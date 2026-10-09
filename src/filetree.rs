//! File tree: selection, inline create/rename, drag & drop moving, importing files,
//! and the file operations behind it (keeping open buffers and \input references in sync).

use crate::app::{truncate, App, Dialog, Tab};
use crate::icons as ic;
use crate::project::FileNode;
use crate::theme::{mix, with_alpha, Palette};
use crate::widgets;
use egui::text::{CCursor, CCursorRange};
use egui::{pos2, vec2, Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum EditKind {
    NewFile,
    NewFolder,
    Rename(String),
}

#[derive(Clone, Debug)]
pub struct TreeEdit {
    pub kind: EditKind,
    pub parent: String,
    pub name: String,
    pub fresh: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct TreeUi {
    pub selected: Option<String>,
    pub edit: Option<TreeEdit>,
    pub hover_dir: Option<String>,
    pub rect: Option<Rect>,
}

const CHAPTER_DIRS: &[&str] = &["content", "kapitel", "chapters", "chapter", "sections", "sektionen", "teile"];

fn parent_of(rel: &str) -> String {
    Path::new(rel).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

fn strip_tex(s: &str) -> &str {
    s.strip_suffix(".tex").unwrap_or(s)
}

fn valid_name(name: &str) -> Result<String, String> {
    let n = name.trim().trim_matches('/').to_string();
    if n.is_empty() {
        return Err("Name fehlt".into());
    }
    if n.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
        return Err("Ungültiger Name".into());
    }
    if n.contains('\\') {
        return Err("Bitte „/“ statt „\\“ verwenden".into());
    }
    Ok(n)
}

impl App {
    fn is_dir_rel(&self, rel: &str) -> bool {
        rel.is_empty() || self.project.root.join(rel).is_dir()
    }

    /// Folder in which "new file" creates things: the selected folder, or the folder of the selected file.
    pub fn target_dir(&self) -> String {
        match &self.tree_ui.selected {
            Some(s) if self.is_dir_rel(s) => s.clone(),
            Some(s) => parent_of(s),
            None => String::new(),
        }
    }

    pub fn begin_new(&mut self, folder: bool, parent: Option<String>) {
        let parent = parent.unwrap_or_else(|| self.target_dir());
        self.collapsed.remove(&parent);
        self.tree_ui.edit = Some(TreeEdit { kind: if folder { EditKind::NewFolder } else { EditKind::NewFile }, parent, name: String::new(), fresh: true, error: None });
    }

    pub fn begin_rename(&mut self, rel: &str) {
        let name = rel.rsplit('/').next().unwrap_or(rel).to_string();
        self.tree_ui.edit = Some(TreeEdit { kind: EditKind::Rename(rel.to_string()), parent: parent_of(rel), name, fresh: true, error: None });
    }

    /// Create a file or folder; returns its relative path.
    pub fn create_entry(&mut self, parent: &str, name: &str, folder: bool) -> Result<String, String> {
        let mut n = valid_name(name)?;
        let last = n.rsplit('/').next().unwrap_or(&n).to_string();
        if !folder && !last.contains('.') {
            n.push_str(".tex");
        }
        let rel = join(parent, &n);
        let abs = self.project.root.join(&rel);
        if abs.exists() {
            return Err(format!("„{n}“ existiert bereits"));
        }
        if folder {
            std::fs::create_dir_all(&abs).map_err(|e| e.to_string())?;
        } else {
            if let Some(d) = abs.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            let dir_name = Path::new(&rel).parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            let content = if rel.ends_with(".tex") && CHAPTER_DIRS.contains(&dir_name.as_str()) {
                let stem = Path::new(&rel).file_stem().unwrap().to_string_lossy().to_string();
                let title = crate::theme::pretty_name(&stem);
                let slug: String = stem.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
                format!("\\chapter{{{title}}}\n\\label{{ch:{slug}}}\n\n")
            } else {
                String::new()
            };
            std::fs::write(&abs, content).map_err(|e| e.to_string())?;
        }
        self.refresh_tree();
        Ok(rel)
    }

    fn tex_sources(&self) -> Vec<String> {
        self.flat.iter().filter(|f| f.ends_with(".tex")).cloned().collect()
    }

    /// Replace text of a project file, through its buffer if it is open.
    fn write_source(&mut self, rel: &str, text: String) {
        if let Some(i) = self.buffer_idx(rel) {
            self.buffers[i].text = text;
            self.buffers[i].last_edit = 0.0;
        } else {
            let _ = std::fs::write(self.project.root.join(rel), text);
        }
    }

    /// Insert `\include{…}`/`\input{…}` for `rel` into the main file, next to its siblings.
    pub fn include_in_main(&mut self, rel: &str) -> Result<String, String> {
        let main = self.project.config.thesis_main.clone();
        if rel == main {
            return Err("Das ist das Hauptdokument".into());
        }
        let text = self.read_source(&main).ok_or("Hauptdokument nicht gefunden")?;
        let target = strip_tex(rel).to_string();
        let re = regex::Regex::new(r"^(\s*)\\(input|include)\{([^}]+)\}").unwrap();
        let lines: Vec<&str> = text.lines().collect();
        if lines.iter().any(|l| re.captures(crate::project::strip_comment(l)).is_some_and(|c| strip_tex(&c[3]) == target)) {
            return Err("Ist bereits eingebunden".into());
        }
        let begin = lines.iter().position(|l| l.contains("\\begin{document}")).unwrap_or(0);
        let dir = parent_of(rel);
        let mut best: Option<(usize, String, String)> = None; // line, indent, cmd
        let mut any: Option<(usize, String, String)> = None;
        for (i, l) in lines.iter().enumerate().skip(begin) {
            if let Some(c) = re.captures(crate::project::strip_comment(l)) {
                let entry = (i, c[1].to_string(), c[2].to_string());
                if parent_of(&c[3]) == dir {
                    best = Some(entry.clone());
                }
                any = Some(entry);
            }
        }
        let (idx, indent, cmd) = match best.or(any) {
            Some(b) => b,
            None => {
                let end = lines.iter().position(|l| l.contains("\\end{document}")).ok_or("Kein \\end{document} gefunden")?;
                (end.saturating_sub(1), String::new(), "input".to_string())
            }
        };
        let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        out.insert(idx + 1, format!("{indent}\\{cmd}{{{target}}}"));
        let mut new = out.join("\n");
        if text.ends_with('\n') {
            new.push('\n');
        }
        self.write_source(&main, new);
        Ok(format!("\\{cmd}{{{target}}} in {main} eingefügt"))
    }

    /// Rename or move a file/folder, keeping buffers, tabs and LaTeX references in sync.
    pub fn rename_path(&mut self, from: &str, to: &str) -> Result<(), String> {
        if from == to {
            return Ok(());
        }
        if to.starts_with(&format!("{from}/")) {
            return Err("Ein Ordner kann nicht in sich selbst verschoben werden".into());
        }
        let root = self.project.root.clone();
        if root.join(to).exists() {
            return Err(format!("„{to}“ existiert bereits"));
        }
        // save first so nothing is lost or written back to the old place
        self.save_all();
        if let Some(d) = root.join(to).parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::rename(root.join(from), root.join(to)).map_err(|e| e.to_string())?;
        let map = |p: &str| -> Option<String> {
            if p == from {
                Some(to.to_string())
            } else {
                p.strip_prefix(&format!("{from}/")).map(|rest| format!("{to}/{rest}"))
            }
        };
        for b in &mut self.buffers {
            if let Some(n) = map(&b.rel) {
                b.rel = n.clone();
                b.abs = root.join(&n);
            }
        }
        for ws in [&mut self.thesis, &mut self.slides] {
            for t in &mut ws.tabs {
                if let Some(n) = map(t) {
                    *t = n;
                }
            }
            if let Some(a) = ws.active.clone() {
                if let Some(n) = map(&a) {
                    ws.active = Some(n);
                }
            }
        }
        self.collapsed = self.collapsed.iter().map(|c| map(c).unwrap_or_else(|| c.clone())).collect();
        if let Some(s) = self.tree_ui.selected.clone() {
            self.tree_ui.selected = Some(map(&s).unwrap_or(s));
        }
        for cfg in [&mut self.project.config.thesis_main, &mut self.project.config.slides_main] {
            if let Some(n) = map(cfg) {
                *cfg = n;
            }
        }
        let _ = self.project.save_config();
        self.refresh_tree();
        self.update_references(from, to);
        Ok(())
    }

    /// Fix `\input`, `\include`, `\includegraphics` arguments after a rename.
    fn update_references(&mut self, from: &str, to: &str) {
        let re = regex::Regex::new(r"\\(input|include|includegraphics|subfile|includepdf)(\[[^\]]*\])?\{([^}]*)\}").unwrap();
        let (from_s, to_s) = (strip_tex(from).to_string(), strip_tex(to).to_string());
        for f in self.tex_sources() {
            let Some(text) = self.read_source(&f) else { continue };
            let mut changed = false;
            let new = re
                .replace_all(&text, |c: &regex::Captures| {
                    let arg = &c[3];
                    let a = arg.trim_start_matches("./");
                    let rep = if a == from || a == from_s {
                        Some(if a.ends_with(".tex") || !from.ends_with(".tex") { to.to_string() } else { to_s.clone() })
                    } else {
                        a.strip_prefix(&format!("{from}/")).map(|rest| format!("{to}/{rest}"))
                    };
                    match rep {
                        Some(r) => {
                            changed = true;
                            format!("\\{}{}{{{}}}", &c[1], c.get(2).map_or("", |m| m.as_str()), r)
                        }
                        None => c[0].to_string(),
                    }
                })
                .to_string();
            if changed {
                self.write_source(&f, new);
            }
        }
    }

    pub fn delete_path(&mut self, rel: &str) {
        let p = self.project.root.join(rel);
        let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        let inside = |r: &str| r == rel || r.starts_with(&format!("{rel}/"));
        let affected: Vec<String> = self.buffers.iter().filter(|b| inside(&b.rel)).map(|b| b.rel.clone()).collect();
        for r in &affected {
            for t in [Tab::Thesis, Tab::Slides] {
                let ws = self.ws_mut(t);
                if let Some(pos) = ws.tabs.iter().position(|x| x == r) {
                    ws.tabs.remove(pos);
                    if ws.active.as_deref() == Some(r.as_str()) {
                        ws.active = ws.tabs.get(pos.min(ws.tabs.len().saturating_sub(1))).cloned();
                    }
                }
            }
        }
        self.buffers.retain(|b| !inside(&b.rel));
        if self.tree_ui.selected.as_deref().is_some_and(inside) {
            self.tree_ui.selected = None;
        }
        self.refresh_tree();
    }

    /// Copy external files into a project folder (name clashes get a counter).
    pub fn import_files(&mut self, files: &[std::path::PathBuf], dir: &str) -> usize {
        let mut n = 0;
        for f in files {
            if !f.is_file() {
                continue;
            }
            let name = f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let (stem, ext) = match name.rsplit_once('.') {
                Some((s, e)) => (s.to_string(), format!(".{e}")),
                None => (name.clone(), String::new()),
            };
            let mut rel = join(dir, &name);
            let mut k = 2;
            while self.project.root.join(&rel).exists() {
                rel = join(dir, &format!("{stem} ({k}){ext}"));
                k += 1;
            }
            let dst = self.project.root.join(&rel);
            if let Some(d) = dst.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if std::fs::copy(f, &dst).is_ok() {
                n += 1;
                self.tree_ui.selected = Some(rel);
            }
        }
        self.collapsed.remove(dir);
        self.refresh_tree();
        n
    }

    pub fn finish_edit_pub(&mut self, t: Tab, now: f64) {
        self.finish_edit(t, now)
    }

    fn finish_edit(&mut self, t: Tab, now: f64) {
        let Some(e) = self.tree_ui.edit.clone() else { return };
        let result = match &e.kind {
            EditKind::NewFile | EditKind::NewFolder => {
                let folder = e.kind == EditKind::NewFolder;
                self.create_entry(&e.parent, &e.name, folder).map(|rel| {
                    self.tree_ui.selected = Some(rel.clone());
                    if folder {
                        self.collapsed.remove(&rel);
                    } else {
                        self.open_file(&rel, t);
                        let chapter_dir = Path::new(&rel).parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                        if rel.ends_with(".tex") && CHAPTER_DIRS.contains(&chapter_dir.as_str()) {
                            if let Ok(msg) = self.include_in_main(&rel) {
                                let g = self.pal.green;
                                self.toast(ic::LINK, msg, g, now);
                            }
                        }
                    }
                })
            }
            EditKind::Rename(from) => match valid_name(&e.name) {
                Ok(n) => {
                    let to = join(&e.parent, &n);
                    self.rename_path(from, &to)
                }
                Err(err) => Err(err),
            },
        };
        match result {
            Ok(()) => self.tree_ui.edit = None,
            Err(err) => {
                if let Some(ed) = &mut self.tree_ui.edit {
                    ed.error = Some(err);
                    ed.fresh = true;
                }
            }
        }
    }
}

// ───────────────────────────── UI ─────────────────────────────

pub fn files_view(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab, now: f64) {
    let ctx = ui.ctx().clone();
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("DATEIEN").font(widgets::ui_font(10.5)).color(pal.dim).extra_letter_spacing(1.2));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if widgets::icon_button_sized(ui, ic::UPLOAD, "Dateien hochladen (oder ins Fenster ziehen)", pal, false, 24.0).clicked() {
                let dir = app.target_dir();
                if let Some(files) = rfd::FileDialog::new().set_title("Dateien zum Projekt hinzufügen").pick_files() {
                    let n = app.import_files(&files, &dir);
                    let g = pal.green;
                    app.toast(ic::UPLOAD, format!("{n} Datei(en) hinzugefügt"), g, now);
                }
            }
            if widgets::icon_button_sized(ui, ic::FOLDER, "Neuer Ordner", pal, matches!(app.tree_ui.edit.as_ref().map(|e| &e.kind), Some(EditKind::NewFolder)), 24.0).clicked() {
                app.begin_new(true, None);
            }
            if widgets::icon_button_sized(ui, ic::PLUS, "Neue Datei (.tex wird ergänzt)", pal, matches!(app.tree_ui.edit.as_ref().map(|e| &e.kind), Some(EditKind::NewFile)), 24.0).clicked() {
                app.begin_new(false, None);
            }
        });
    });
    // where new things go
    let target = app.target_dir();
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(format!("{}  /{}", ic::FOLDER_OPEN, target)).font(widgets::ui_font(11.0)).color(mix(pal.dim, pal.accent, 0.35)))
            .on_hover_text("Neue Dateien und Uploads landen in diesem Ordner. Ordner anklicken, um ihn zu wählen.");
    });
    ui.add_space(4.0);

    let tree = app.tree.clone();
    let active = app.ws(t).active.clone();
    app.tree_ui.hover_dir = None;
    let mut clicked_empty = false;
    let os_drag = ctx.input(|i| !i.raw.hovered_files.is_empty());
    let scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        if app.tree_ui.edit.as_ref().is_some_and(|e| e.parent.is_empty() && !matches!(e.kind, EditKind::Rename(_))) {
            edit_row(app, ui, pal, 0, t, now);
        }
        for n in &tree {
            tree_node(app, ui, pal, n, 0, active.as_deref(), t, now, os_drag);
        }
        // empty area below = project root (click to select root, drop to move to root)
        let rest = ui.available_size().y.max(40.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), rest), Sense::click());
        if let Some(p) = resp.dnd_hover_payload::<String>() {
            if !parent_of(&p).is_empty() {
                ui.painter().rect_stroke(rect.shrink(2.0), 6.0, Stroke::new(1.5, with_alpha(pal.accent, 140)), StrokeKind::Inside);
                ui.painter().text(rect.center_top() + vec2(0.0, 18.0), Align2::CENTER_CENTER, "In Projektordner verschieben", widgets::ui_font(11.5), pal.accent);
            }
        }
        if let Some(p) = resp.dnd_release_payload::<String>() {
            let name = p.rsplit('/').next().unwrap_or(&p).to_string();
            if let Err(e) = app.rename_path(&p, &name) {
                let r = pal.red;
                app.toast(ic::WARN, e, r, now);
            }
        }
        if resp.hovered() {
            app.tree_ui.hover_dir = Some(String::new());
        }
        if resp.clicked() {
            clicked_empty = true;
        }
        resp.context_menu(|ui| {
            if ui.button(format!("{}  Neue Datei", ic::PLUS)).clicked() {
                app.begin_new(false, Some(String::new()));
                ui.close();
            }
            if ui.button(format!("{}  Neuer Ordner", ic::FOLDER)).clicked() {
                app.begin_new(true, Some(String::new()));
                ui.close();
            }
        });
    });
    let tree_rect = scroll.inner_rect;
    app.tree_ui.rect = Some(tree_rect);
    if clicked_empty {
        app.tree_ui.selected = None;
    }
    // OS file drag hint
    if os_drag && ui.rect_contains_pointer(tree_rect) {
        let dir = app.tree_ui.hover_dir.clone().unwrap_or_else(|| app.target_dir());
        let pill = Rect::from_min_size(pos2(tree_rect.min.x + 8.0, tree_rect.max.y - 40.0), vec2(tree_rect.width() - 16.0, 30.0));
        ui.painter().rect_filled(pill, 8.0, pal.accent);
        ui.painter().text(pill.center(), Align2::CENTER_CENTER, format!("{}  Ablegen in /{}", ic::DOWNLOAD, truncate(&dir, 26)), widgets::ui_font(12.0), pal.on_accent);
    }
    // keyboard: F2 rename, Del delete (when the tree is hovered and no text field is active)
    if ui.rect_contains_pointer(tree_rect) && !ctx.egui_wants_keyboard_input() && app.tree_ui.edit.is_none() {
        if let Some(sel) = app.tree_ui.selected.clone() {
            if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
                app.begin_rename(&sel);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                app.dialog = Some(Dialog::Delete { rel: sel });
            }
        }
    }
    // drag ghost
    if let Some(p) = egui::DragAndDrop::payload::<String>(&ctx) {
        if let Some(pos) = ctx.pointer_interact_pos() {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("tree-drag")));
            let name = p.rsplit('/').next().unwrap_or(&p).to_string();
            let g = painter.layout_no_wrap(name, widgets::ui_font(12.5), pal.text);
            let r = Rect::from_min_size(pos + vec2(14.0, 6.0), g.size() + vec2(20.0, 10.0));
            painter.rect_filled(r, 7.0, pal.surface);
            painter.rect_stroke(r, 7.0, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
            painter.galley(r.min + vec2(10.0, 5.0), g, pal.text);
        }
    }
}

pub fn file_icon(name: &str, pal: &Palette) -> (&'static str, Color32) {
    let l = name.to_lowercase();
    if l.ends_with(".tex") {
        (ic::FILE_TEXT, pal.accent)
    } else if l.ends_with(".bib") {
        (ic::BOOK, pal.yellow)
    } else if l.ends_with(".pdf") {
        (ic::FILE_PDF, pal.red)
    } else if l.ends_with(".png") || l.ends_with(".jpg") || l.ends_with(".jpeg") || l.ends_with(".svg") || l.ends_with(".eps") {
        (ic::FILE_IMAGE, pal.green)
    } else if l.ends_with(".sty") || l.ends_with(".cls") {
        (ic::FILE_CODE, pal.magenta)
    } else {
        (ic::FILE, pal.dim)
    }
}

fn is_image(rel: &str) -> bool {
    let l = rel.to_lowercase();
    [".png", ".jpg", ".jpeg", ".pdf", ".eps", ".svg"].iter().any(|e| l.ends_with(e)) && !l.starts_with("papers/")
}

/// Inline text field used for "new file/folder" and "rename".
fn edit_row(app: &mut App, ui: &mut Ui, pal: &Palette, depth: usize, t: Tab, now: f64) {
    let Some(e) = app.tree_ui.edit.clone() else { return };
    let (icon, hint) = match e.kind {
        EditKind::NewFolder => (ic::FOLDER, "Ordnername"),
        EditKind::NewFile => (ic::FILE_TEXT, "name.tex"),
        EditKind::Rename(_) => (ic::PENCIL, ""),
    };
    let row_h = 28.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
    let x = rect.min.x + 8.0 + depth as f32 * 14.0;
    ui.painter().text(pos2(x + 20.0, rect.center().y), Align2::CENTER_CENTER, icon, widgets::ui_font(12.0), pal.accent);
    let field = Rect::from_min_max(pos2(x + 32.0, rect.min.y + 2.0), pos2(rect.max.x - 4.0, rect.max.y - 2.0));
    ui.painter().rect_filled(field, 5.0, pal.base);
    ui.painter().rect_stroke(field, 5.0, Stroke::new(1.0, if e.error.is_some() { pal.red } else { pal.accent }), StrokeKind::Inside);
    let id = egui::Id::new("tree-edit-field");
    let mut name = e.name.clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let resp = child.add(egui::TextEdit::singleline(&mut name).id(id).frame(egui::Frame::NONE).hint_text(egui::RichText::new(hint).color(pal.dim)).font(widgets::ui_font(13.0)).desired_width(f32::INFINITY));
    if e.fresh {
        resp.request_focus();
        // pre-select the name without extension (like a file manager)
        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
            let stem_len = match name.rfind('.') {
                Some(p) if p > 0 && matches!(e.kind, EditKind::Rename(_)) => name[..p].chars().count(),
                _ => name.chars().count(),
            };
            st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(stem_len))));
            st.store(ui.ctx(), id);
        }
    }
    let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    if let Some(ed) = &mut app.tree_ui.edit {
        ed.name = name.clone();
        ed.fresh = false;
        if resp.changed() {
            ed.error = None;
        }
    }
    if esc {
        app.tree_ui.edit = None;
        return;
    }
    if resp.lost_focus() {
        let unchanged = match &e.kind {
            EditKind::Rename(from) => from.rsplit('/').next() == Some(name.trim()),
            _ => false,
        };
        if name.trim().is_empty() || unchanged {
            app.tree_ui.edit = None;
        } else {
            // Enter or clicking elsewhere both confirm
            let _ = enter;
            app.finish_edit(t, now);
        }
    }
    if let Some(err) = app.tree_ui.edit.as_ref().and_then(|e| e.error.clone()) {
        ui.horizontal(|ui| {
            ui.add_space(x - rect.min.x + 32.0);
            ui.label(egui::RichText::new(err).font(widgets::ui_font(11.0)).color(pal.red));
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn tree_node(app: &mut App, ui: &mut Ui, pal: &Palette, n: &FileNode, depth: usize, active: Option<&str>, t: Tab, now: f64, os_drag: bool) {
    // inline rename replaces the row
    if matches!(app.tree_ui.edit.as_ref().map(|e| &e.kind), Some(EditKind::Rename(r)) if *r == n.rel) {
        edit_row(app, ui, pal, depth, t, now);
        if n.is_dir && !app.collapsed.contains(&n.rel) {
            for c in &n.children {
                tree_node(app, ui, pal, c, depth + 1, active, t, now, os_drag);
            }
        }
        return;
    }
    let collapsed = app.collapsed.contains(&n.rel);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 27.0), Sense::click_and_drag());
    let is_active = active == Some(n.rel.as_str());
    let is_sel = app.tree_ui.selected.as_deref() == Some(n.rel.as_str());
    let drop_dir = if n.is_dir { n.rel.clone() } else { parent_of(&n.rel) };

    // drag source
    if resp.dragged() {
        resp.dnd_set_drag_payload(n.rel.clone());
    }
    // drop target (folders, or the folder of a file)
    let mut drop_hover = false;
    if let Some(p) = resp.dnd_hover_payload::<String>() {
        let p = p.as_str();
        drop_hover = n.is_dir && p != n.rel && !n.rel.starts_with(&format!("{p}/")) && parent_of(p) != n.rel;
    }
    if let Some(p) = resp.dnd_release_payload::<String>() {
        let p = p.as_str().to_string();
        if p != drop_dir && !drop_dir.starts_with(&format!("{p}/")) && parent_of(&p) != drop_dir {
            let name = p.rsplit('/').next().unwrap_or(&p).to_string();
            match app.rename_path(&p, &join(&drop_dir, &name)) {
                Ok(()) => {
                    app.collapsed.remove(&drop_dir);
                    let g = pal.green;
                    app.toast(ic::FOLDER_OPEN, format!("{name} → /{drop_dir}"), g, now);
                }
                Err(e) => {
                    let r = pal.red;
                    app.toast(ic::WARN, e, r, now);
                }
            }
        }
    }
    if resp.hovered() || (os_drag && ui.rect_contains_pointer(rect)) {
        app.tree_ui.hover_dir = Some(drop_dir.clone());
    }

    let p = ui.painter();
    if drop_hover || (os_drag && n.is_dir && ui.rect_contains_pointer(rect)) {
        p.rect_filled(rect, 6.0, with_alpha(pal.accent, 45));
        p.rect_stroke(rect, 6.0, Stroke::new(1.5, pal.accent), StrokeKind::Inside);
    } else if is_active {
        p.rect_filled(rect, 6.0, with_alpha(pal.accent, 30));
    } else if is_sel {
        p.rect_filled(rect, 6.0, with_alpha(pal.text, 16));
    } else if resp.hovered() {
        p.rect_filled(rect, 6.0, with_alpha(pal.text, 9));
    }
    if is_sel && n.is_dir {
        p.rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(2.5, 15.0)), 1.0, pal.accent);
    }
    let x = rect.min.x + 8.0 + depth as f32 * 14.0;
    let cy = rect.center().y;
    // indent guides
    for d in 0..depth {
        let gx = rect.min.x + 12.0 + d as f32 * 14.0;
        p.line_segment([pos2(gx, rect.min.y), pos2(gx, rect.max.y)], Stroke::new(1.0, with_alpha(pal.border, 90)));
    }
    if n.is_dir {
        p.text(pos2(x + 4.0, cy), Align2::CENTER_CENTER, if collapsed { ic::CHEVRON_RIGHT } else { ic::CHEVRON_DOWN }, widgets::ui_font(8.5), pal.dim);
        p.text(pos2(x + 20.0, cy), Align2::CENTER_CENTER, if collapsed { ic::FOLDER } else { ic::FOLDER_OPEN }, widgets::ui_font(12.5), mix(pal.accent, pal.subtext, 0.4));
    } else {
        let (icon, col) = file_icon(&n.name, pal);
        p.text(pos2(x + 20.0, cy), Align2::CENTER_CENTER, icon, widgets::ui_font(12.0), col);
    }
    let dirty = app.buffer_idx(&n.rel).is_some_and(|i| app.buffers[i].dirty());
    let maxc = ((rect.max.x - x - 62.0) / 7.0).max(4.0) as usize;
    let gs = if n.is_dir { None } else { app.git_letter(&n.rel) };
    let name_col = match gs {
        Some(c) => mix(widgets::git_color(c, pal), pal.text, 0.25),
        None if is_active => pal.bright,
        None => pal.text,
    };
    p.text(pos2(x + 34.0, cy), Align2::LEFT_CENTER, truncate(&n.name, maxc), widgets::ui_font(13.0), name_col);
    let hovering_dir = n.is_dir && resp.hovered();
    if let Some(c) = gs {
        p.text(pos2(rect.max.x - 12.0, cy), Align2::CENTER_CENTER, c.to_string(), widgets::mono_font(11.5), widgets::git_color(c, pal));
    } else if n.is_dir && !hovering_dir && (app.git.dir_has_changes(&n.rel) || (app.git.is_repo && app.buffers.iter().any(|b| b.dirty() && b.rel.starts_with(&format!("{}/", n.rel))))) {
        p.circle_filled(pos2(rect.max.x - 12.0, cy), 3.0, with_alpha(widgets::git_color('M', pal), 200));
    }
    if dirty {
        p.circle_filled(pos2(rect.max.x - if gs.is_some() { 26.0 } else { 12.0 }, cy), 3.5, pal.yellow);
    }
    // inline "+" on hovered folders
    if n.is_dir && resp.hovered() && !resp.dragged() {
        let pr = Rect::from_center_size(pos2(rect.max.x - 14.0, cy), vec2(20.0, 20.0));
        let presp = ui.interact(pr, ui.id().with(("tree-plus", &n.rel)), Sense::click());
        ui.painter().rect_filled(pr, 5.0, if presp.hovered() { with_alpha(pal.accent, 50) } else { with_alpha(pal.text, 14) });
        ui.painter().text(pr.center(), Align2::CENTER_CENTER, ic::PLUS, widgets::ui_font(10.0), pal.text);
        if presp.on_hover_text("Neue Datei in diesem Ordner").clicked() {
            app.tree_ui.selected = Some(n.rel.clone());
            app.begin_new(false, Some(n.rel.clone()));
        }
    }

    if resp.clicked() {
        app.tree_ui.selected = Some(n.rel.clone());
        if n.is_dir {
            if collapsed {
                app.collapsed.remove(&n.rel);
            } else {
                app.collapsed.insert(n.rel.clone());
            }
        } else {
            app.open_file(&n.rel, t);
        }
    }
    if resp.double_clicked() && !n.is_dir && !crate::project::is_text_file(&n.rel) {
        crate::platform::open_external(app.project.root.join(&n.rel));
    }
    resp.context_menu(|ui| {
        app.tree_ui.selected = Some(n.rel.clone());
        if n.is_dir {
            if ui.button(format!("{}  Neue Datei", ic::PLUS)).clicked() {
                app.begin_new(false, Some(n.rel.clone()));
                ui.close();
            }
            if ui.button(format!("{}  Neuer Ordner", ic::FOLDER)).clicked() {
                app.begin_new(true, Some(n.rel.clone()));
                ui.close();
            }
            if ui.button(format!("{}  Dateien hierher hochladen", ic::UPLOAD)).clicked() {
                if let Some(files) = rfd::FileDialog::new().pick_files() {
                    let k = app.import_files(&files, &n.rel);
                    let g = pal.green;
                    app.toast(ic::UPLOAD, format!("{k} Datei(en) hinzugefügt"), g, now);
                }
                ui.close();
            }
            ui.separator();
        } else {
            if n.rel.ends_with(".tex") && n.rel != app.project.config.thesis_main && ui.button(format!("{}  In Hauptdokument einbinden", ic::LINK)).clicked() {
                match app.include_in_main(&n.rel) {
                    Ok(m) => {
                        let g = pal.green;
                        app.toast(ic::LINK, m, g, now);
                    }
                    Err(e) => {
                        let y = pal.yellow;
                        app.toast(ic::WARN, e, y, now);
                    }
                }
                ui.close();
            }
            if is_image(&n.rel) && ui.button(format!("{}  Als Abbildung einfügen", ic::IMAGE)).clicked() {
                let stem = Path::new(&n.rel).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let snip = if t == Tab::Slides {
                    format!("\\begin{{frame}}{{$0{stem}}}\n  \\centering\n  \\includegraphics[width=0.8\\textwidth,height=0.7\\textheight,keepaspectratio]{{{}}}\n\\end{{frame}}\n", n.rel)
                } else {
                    format!("\\begin{{figure}}[htbp]\n  \\centering\n  \\includegraphics[width=0.8\\linewidth]{{{}}}\n  \\caption{{$0}}\n  \\label{{fig:{stem}}}\n\\end{{figure}}\n", n.rel)
                };
                app.insert_snippet(t, &snip);
                ui.close();
            }
            if ui.button(format!("{}  Pfad kopieren", ic::COPY)).clicked() {
                ui.ctx().copy_text(n.rel.clone());
                ui.close();
            }
        }
        if ui.button(format!("{}  Umbenennen   F2", ic::PENCIL)).clicked() {
            app.begin_rename(&n.rel);
            ui.close();
        }
        if ui.button(format!("{}  Im Dateimanager zeigen", ic::FOLDER_OPEN)).clicked() {
            let p = app.project.root.join(&n.rel);
            let dir = if n.is_dir { p } else { p.parent().unwrap().to_path_buf() };
            crate::platform::open_external(dir);
            ui.close();
        }
        ui.separator();
        if ui.button(egui::RichText::new(format!("{}  Löschen   Entf", ic::TRASH)).color(pal.red)).clicked() {
            app.dialog = Some(Dialog::Delete { rel: n.rel.clone() });
            ui.close();
        }
    });
    if n.is_dir && !collapsed {
        if let Some(e) = &app.tree_ui.edit {
            if e.parent == n.rel && !matches!(e.kind, EditKind::Rename(_)) {
                edit_row(app, ui, pal, depth + 1, t, now);
            }
        }
        for c in &n.children {
            tree_node(app, ui, pal, c, depth + 1, active, t, now, os_drag);
        }
    }
}
