//! Application state, top bar, status bar, theme handling and dialogs.

use crate::compile::{self, CompileJob, CompileSpec, Issue, Level};
use crate::editor::{Buffer, CompItem, Syntax};
use crate::icons as ic;
use crate::pdfview::{PageCache, PdfViewer, Renderer};
use crate::project::{self, FileNode, OutlineItem, Project};
use crate::shelf::{SearchHit, Shelf, ShelfMsg};
use crate::theme::{self, mix, with_alpha, Palette};
use crate::widgets::{self, BtnKind};
use egui::{pos2, vec2, Align2, Color32, Rect, Stroke};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::SystemTime;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `None` = follow the active Omarchy theme.
    pub theme: Option<String>,
    pub font_size: f32,
    pub last_project: Option<PathBuf>,
    pub auto_compile: bool,
    pub dark_pdf: bool,
    pub pdf_frac: f32,
    pub stage_frac: f32,
    pub visual: bool,
    pub doc_width: f32,
    pub update_check: bool,
    /// Editor input: standard or vim keybindings (code mode).
    pub input_vim: bool,
    /// UI language: false = Deutsch (default), true = English.
    pub lang_en: bool,
    /// PDF preview next to the editor (standard view).
    pub show_pdf: bool,
    /// Git diff: side by side instead of inline.
    pub diff_split: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { theme: None, font_size: 14.0, last_project: None, auto_compile: true, dark_pdf: false, pdf_frac: 0.5, stage_frac: 0.52, visual: false, doc_width: 820.0, update_check: true, input_vim: false, lang_en: false, show_pdf: true, diff_split: false }
    }
}

fn settings_path() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("nedit/settings.json")
}

impl Settings {
    fn load() -> Self {
        std::fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        if std::env::var_os("NEDIT_SHOT").is_some() {
            return; // never touch real settings from screenshot runs
        }
        let p = settings_path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_string_pretty(self).unwrap_or_default());
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Thesis,
    Slides,
    Shelf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SideMode {
    Files,
    Outline,
    Papers,
    Git,
}

pub struct Workspace {
    pub tabs: Vec<String>,
    pub active: Option<String>,
    pub viewer: PdfViewer,
    pub job: Option<CompileJob>,
    pub queued: bool,
    pub issues: Vec<Issue>,
    pub last_ok: Option<bool>,
    pub last_secs: f32,
    pub raw_log: String,
    pub show_logs: bool,
    pub show_raw: bool,
    pub sync_line: Option<(String, usize, f64)>,
    pub compiled_once: bool,
    /// Source position to reveal in the PDF once the running compile finishes.
    pub sync_after: Option<(String, usize)>,
}

impl Workspace {
    fn new(kind: Tab) -> Self {
        let mut viewer = PdfViewer::new(if kind == Tab::Thesis { "thesis" } else { "slides" });
        if kind == Tab::Slides {
            viewer.single_page = true;
        }
        Workspace {
            tabs: vec![],
            active: None,
            viewer,
            job: None,
            queued: false,
            issues: vec![],
            last_ok: None,
            last_secs: 0.0,
            raw_log: String::new(),
            show_logs: false,
            show_raw: false,
            sync_line: None,
            compiled_once: false,
            sync_after: None,
        }
    }
    pub fn errors(&self) -> usize {
        self.issues.iter().filter(|i| i.level == Level::Error).count()
    }
    pub fn warnings(&self) -> usize {
        self.issues.iter().filter(|i| i.level == Level::Warning).count()
    }
}

pub struct Toast {
    pub msg: String,
    pub color: Color32,
    pub icon: &'static str,
    pub t0: f64,
}

#[derive(Default)]
pub struct ShelfUi {
    pub input: String,
    pub busy: bool,
    pub status: String,
    pub results: Vec<SearchHit>,
    pub selected: Option<String>,
    pub filter: String,
    pub status_filter: Option<crate::shelf::ReadStatus>,
    pub fav_only: bool,
    pub tag_filter: Option<String>,
    pub tags_edit: String,
    pub bib_edit: Option<String>,
    pub reading: Option<String>,
    pub confirm_delete: Option<String>,
    pub side_filter: String,
}

pub enum Dialog {
    Delete { rel: String },
    NewProject { name: String },
    GitRestore { hash: String, path: String },
    /// Revert changes of these paths (files or folders, "" = everything) to the last commit.
    GitRevert { paths: Vec<String> },
    Update,
}

pub struct FindState {
    pub open: bool,
    pub query: String,
    pub replace: String,
    pub focus: bool,
}

pub struct PresentState {
    pub start: f64,
    pub black: bool,
    pub hud: bool,
    pub paused_at: Option<f64>,
}

pub struct App {
    pub pal: Palette,
    pub syntax: Syntax,
    pub style_rev: u64,
    omarchy_stamp: Option<SystemTime>,
    last_poll: f64,
    pub settings: Settings,
    pub project: Project,
    pub buffers: Vec<Buffer>,
    pub shelf: Shelf,
    pub tab: Tab,
    pub last_doc_tab: Tab,
    pub thesis: Workspace,
    pub slides: Workspace,
    pub renderer: Renderer,
    pub side: SideMode,
    pub tree: Vec<FileNode>,
    pub flat: Vec<String>,
    pub outline: Vec<OutlineItem>,
    pub slide_outline: Vec<OutlineItem>,
    pub labels: Vec<String>,
    pub cites: Vec<CompItem>,
    cites_rev: u64,
    pub word_count: usize,
    wc_hash: u64,
    pub shelf_ui: ShelfUi,
    pub shelf_tx: Sender<ShelfMsg>,
    shelf_rx: Receiver<ShelfMsg>,
    pub reader: PdfViewer,
    pub thumbs: PageCache,
    pub present: Option<PresentState>,
    pub find: FindState,
    pub dialog: Option<Dialog>,
    pub toasts: Vec<Toast>,
    pub collapsed: std::collections::HashSet<String>,
    shot_step: usize,
    shot_wait: bool,
    debug_typed: usize,
    pub doc_mode: bool,
    pub doc_toc: bool,
    pub doc_pdf: bool,
    pub doc_collapsed: std::collections::HashSet<String>,
    pub fullscreen: bool,
    pub focus: bool,
    pub focus_pdf: bool,
    pub pdf_win: Option<egui::Rect>,
    pub pdf_win_gen: u32,
    pub git: crate::git::GitState,
    pub tree_ui: crate::filetree::TreeUi,
    pub updater: crate::updater::Updater,
    update_started: bool,
    pub close_requested: bool,
    pub vim: crate::vim::VimState,
    pub about_open: bool,
    pub help_open: bool,
    pub quick: Option<crate::quickopen::QuickOpen>,
    pub shift_tap: crate::quickopen::ShiftTap,
    /// Most recently opened files (newest first).
    pub recent: Vec<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        let settings = Settings::load();
        crate::i18n::set_english(settings.lang_en || std::env::var("NEDIT_LANG").is_ok_and(|l| l == "en"));
        let pal = load_palette(&settings);
        let project = settings
            .last_project
            .clone()
            .filter(|p| p.join(".nedit/project.json").exists() && p.starts_with(project::projects_dir()))
            .and_then(|p| Project::open(&p).ok())
            .or_else(|| project::list_projects().first().and_then(|p| Project::open(p).ok()))
            .unwrap_or_else(|| Project::create("Masterarbeit", "").expect(tr!("Projektordner kann nicht angelegt werden" | "Cannot create project folder")));
        let shelf = Shelf::load(&project.root);
        let (tx, rx) = channel();
        let mut app = App {
            syntax: Syntax::from_palette(&pal),
            pal,
            style_rev: 1,
            omarchy_stamp: theme::omarchy_stamp(),
            last_poll: 0.0,
            settings,
            project,
            buffers: vec![],
            shelf,
            tab: Tab::Thesis,
            last_doc_tab: Tab::Thesis,
            thesis: Workspace::new(Tab::Thesis),
            slides: Workspace::new(Tab::Slides),
            renderer: Renderer::new(3),
            side: SideMode::Files,
            tree: vec![],
            flat: vec![],
            outline: vec![],
            slide_outline: vec![],
            labels: vec![],
            cites: vec![],
            cites_rev: 0,
            word_count: 0,
            wc_hash: 0,
            shelf_ui: ShelfUi::default(),
            shelf_tx: tx,
            shelf_rx: rx,
            reader: PdfViewer::new("reader"),
            thumbs: PageCache::new("thumbs"),
            present: None,
            find: FindState { open: false, query: String::new(), replace: String::new(), focus: false },
            dialog: None,
            toasts: vec![],
            collapsed: Default::default(),
            shot_step: 0,
            shot_wait: false,
            debug_typed: 0,
            doc_mode: false,
            doc_toc: true,
            doc_pdf: false,
            doc_collapsed: Default::default(),
            fullscreen: false,
            focus: false,
            focus_pdf: false,
            pdf_win: None,
            pdf_win_gen: 0,
            git: Default::default(),
            tree_ui: Default::default(),
            updater: Default::default(),
            update_started: false,
            close_requested: false,
            vim: Default::default(),
            about_open: false,
            help_open: false,
            quick: None,
            shift_tap: Default::default(),
            recent: vec![],
        };
        crate::updater::cleanup();
        app.apply_style(&cc.egui_ctx);
        app.after_project_open(&cc.egui_ctx);
        app
    }

    pub fn after_project_open(&mut self, ctx: &egui::Context) {
        self.settings.last_project = Some(self.project.root.clone());
        self.settings.save();
        self.refresh_tree();
        let main = self.project.config.thesis_main.clone();
        let slides = self.project.config.slides_main.clone();
        self.thesis = Workspace::new(Tab::Thesis);
        self.slides = Workspace::new(Tab::Slides);
        self.buffers.clear();
        self.git = Default::default();
        self.open_file(&main, Tab::Thesis);
        if self.project.has_slides() {
            self.open_file(&slides, Tab::Slides);
        }
        // show existing PDFs immediately, then compile in the background
        for t in [Tab::Thesis, Tab::Slides] {
            let spec = self.spec(t);
            if spec.pdf_path().exists() {
                self.ws_mut(t).viewer.load(&spec.pdf_path());
            }
        }
        self.compile(Tab::Thesis, ctx);
        if self.project.has_slides() {
            self.compile(Tab::Slides, ctx);
        }
        self.rebuild_indexes();
    }

    pub fn apply_style(&mut self, ctx: &egui::Context) {
        ctx.set_visuals(self.pal.visuals());
        ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(10.0, 5.0);
            s.spacing.interact_size = vec2(24.0, 26.0);
            s.spacing.menu_margin = egui::Margin::same(8);
            s.spacing.window_margin = egui::Margin::same(18);
            s.spacing.scroll = egui::style::ScrollStyle::floating();
            s.spacing.scroll.bar_width = 8.0;
            s.interaction.selectable_labels = false;
            s.text_styles.insert(egui::TextStyle::Body, widgets::ui_font(13.5));
            s.text_styles.insert(egui::TextStyle::Button, widgets::ui_font(13.5));
            s.text_styles.insert(egui::TextStyle::Small, widgets::ui_font(11.5));
            s.text_styles.insert(egui::TextStyle::Heading, widgets::display_font(22.0));
            s.text_styles.insert(egui::TextStyle::Monospace, widgets::mono_font(13.0));
        });
        self.syntax = Syntax::from_palette(&self.pal);
        self.style_rev += 1;
        self.thesis.viewer.dark_pages = self.settings.dark_pdf && self.pal.dark;
    }

    pub fn set_theme(&mut self, name: Option<String>, ctx: &egui::Context) {
        self.settings.theme = name;
        self.settings.save();
        self.pal = load_palette(&self.settings);
        self.apply_style(ctx);
    }

    pub fn set_doc_mode(&mut self, on: bool, _ctx: &egui::Context) {
        self.doc_mode = on;
        if on {
            self.rebuild_indexes();
        }
    }

    /// Move keyboard focus to the next/previous file: chapters in document mode,
    /// open tabs otherwise. The cursor position of each file is kept.
    pub fn cycle_file(&mut self, dir: i32) {
        let t = self.tab;
        let list: Vec<String> = if t == Tab::Thesis && self.doc_mode {
            crate::workspace::doc_files(self).into_iter().filter(|f| !self.doc_collapsed.contains(f)).collect()
        } else {
            self.ws(t).tabs.clone()
        };
        if list.is_empty() {
            return;
        }
        let n = list.len() as i32;
        let next = match self.ws(t).active.as_ref().and_then(|a| list.iter().position(|f| f == a)) {
            Some(cur) => list[((cur as i32 + dir).rem_euclid(n)) as usize].clone(),
            // current file isn't part of the list (e.g. main.tex): start at the first/last chapter
            None => if dir > 0 { list[0].clone() } else { list[list.len() - 1].clone() },
        };
        if t == Tab::Thesis && self.doc_mode {
            if let Some(i) = self.buffer_idx(&next) {
                let b = &mut self.buffers[i];
                let (a, z) = (b.sel_end, b.cursor);
                b.select(a, z); // focus + scroll to the remembered cursor
            }
            if !self.ws(t).tabs.contains(&next) {
                self.ws_mut(t).tabs.push(next.clone());
            }
            self.ws_mut(t).active = Some(next);
        } else {
            self.open_file(&next, t);
        }
    }

    /// Full screen with nothing but the text.
    pub fn set_focus(&mut self, on: bool, ctx: &egui::Context) {
        self.focus = on;
        if on != self.fullscreen {
            self.fullscreen = on;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
        }
    }

    pub fn toast(&mut self, icon: &'static str, msg: impl Into<String>, color: Color32, now: f64) {
        self.toasts.push(Toast { msg: msg.into(), color, icon, t0: now });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    // ───────────── files & buffers ─────────────

    pub fn refresh_tree(&mut self) {
        self.tree = project::file_tree(&self.project.root);
        let old = std::mem::take(&mut self.flat);
        project::flat_files(&self.tree, &mut self.flat);
        if old != self.flat {
            self.git.invalidate();
        }
    }

    /// After git changed files on disk: open editors take the disk version (unsaved edits are
    /// discarded on purpose); files that no longer exist are closed.
    pub fn sync_buffers_with_disk(&mut self, paths: &[String]) {
        let mut gone = vec![];
        for p in paths {
            if let Some(i) = self.buffer_idx(p) {
                if self.project.root.join(p).exists() {
                    self.buffers[i].force_reload();
                } else {
                    gone.push(p.clone());
                }
            }
        }
        for p in gone {
            for t in [Tab::Thesis, Tab::Slides] {
                let ws = self.ws_mut(t);
                if let Some(pos) = ws.tabs.iter().position(|x| *x == p) {
                    ws.tabs.remove(pos);
                    if ws.active.as_deref() == Some(p.as_str()) {
                        ws.active = ws.tabs.get(pos.min(ws.tabs.len().saturating_sub(1))).cloned();
                    }
                }
            }
            self.buffers.retain(|b| b.rel != p);
        }
    }

    /// Revert files/folders to the last commit and update everything that shows them.
    pub fn git_revert(&mut self, paths: &[String], now: f64) {
        let root = self.project.root.clone();
        // make sure the change list is current before deciding what to revert
        self.git.refresh_status(&root, now);
        match self.git.revert(&root, paths) {
            Ok(affected) => {
                self.sync_buffers_with_disk(&affected);
                let g = self.pal.green;
                self.toast(ic::UNDO, trf!("{} Datei(en) zurückgesetzt" | "{} file(s) reverted", affected.len()), g, now);
            }
            Err(e) => {
                let r = self.pal.red;
                self.toast(ic::WARN, format!("Git: {}", truncate(&e, 90)), r, now);
            }
        }
        if let Some(crate::git::GitView::WorkingFile(p)) = &self.git.view {
            if !self.git.changes.iter().any(|c| &c.path == p) || paths.iter().any(|x| x.is_empty() || p == x || p.starts_with(&format!("{x}/"))) {
                self.git.view = None;
            }
        }
        self.git.refresh(&root, now);
        self.refresh_tree();
        self.rebuild_indexes();
    }

    /// Create the git repository for this project.
    pub fn git_init(&mut self, now: f64) {
        let root = self.project.root.clone();
        self.save_all();
        match self.git.init(&root) {
            Ok(()) => {
                let g = self.pal.green;
                self.toast(ic::GIT, tr!("Git-Repository angelegt – erste Version gesichert" | "Git repository created – first version saved"), g, now);
            }
            Err(e) => {
                let r = self.pal.red;
                self.toast(ic::WARN, format!("Git: {e}"), r, now);
            }
        }
        self.git.refresh(&root, now);
        self.refresh_tree();
    }

    /// Git status letter for display; open files with unsaved edits show "M" right away.
    pub fn git_letter(&self, rel: &str) -> Option<char> {
        self.git.file_status(rel).or_else(|| {
            (self.git.is_repo && self.buffer_idx(rel).is_some_and(|i| self.buffers[i].dirty())).then_some('M')
        })
    }

    pub fn ws(&self, t: Tab) -> &Workspace {
        if t == Tab::Slides { &self.slides } else { &self.thesis }
    }
    pub fn ws_mut(&mut self, t: Tab) -> &mut Workspace {
        if t == Tab::Slides { &mut self.slides } else { &mut self.thesis }
    }

    pub fn buffer_idx(&self, rel: &str) -> Option<usize> {
        self.buffers.iter().position(|b| b.rel == rel)
    }

    pub fn open_file(&mut self, rel: &str, t: Tab) -> bool {
        if !project::is_text_file(rel) {
            let lower = rel.to_lowercase();
            if lower.ends_with(".pdf") && rel.starts_with("papers/") {
                self.shelf_ui.reading = Some(rel.to_string());
                self.reader.load(&self.project.root.join(rel));
                self.tab = Tab::Shelf;
            } else {
                crate::platform::open_external(self.project.root.join(rel));
            }
            return false;
        }
        if self.buffer_idx(rel).is_none() {
            match Buffer::open(&self.project.root, rel) {
                Ok(b) => self.buffers.push(b),
                Err(_) => return false,
            }
        }
        self.recent.retain(|r| r != rel);
        self.recent.insert(0, rel.to_string());
        self.recent.truncate(20);
        let ws = self.ws_mut(t);
        if !ws.tabs.iter().any(|x| x == rel) {
            ws.tabs.push(rel.to_string());
        }
        ws.active = Some(rel.to_string());
        if let Some(i) = self.buffer_idx(rel) {
            self.buffers[i].request_focus = true;
        }
        true
    }

    pub fn close_tab(&mut self, rel: &str, t: Tab) {
        if let Some(i) = self.buffer_idx(rel) {
            if self.buffers[i].dirty() {
                let _ = self.buffers[i].save();
            }
        }
        let ws = self.ws_mut(t);
        if let Some(pos) = ws.tabs.iter().position(|x| x == rel) {
            ws.tabs.remove(pos);
            if ws.active.as_deref() == Some(rel) {
                ws.active = ws.tabs.get(pos.min(ws.tabs.len().saturating_sub(1))).cloned();
            }
        }
    }

    pub fn jump_to(&mut self, rel: &str, line: usize, t: Tab) {
        if self.open_file(rel, t) {
            if let Some(i) = self.buffer_idx(rel) {
                self.buffers[i].goto_line(line);
            }
        }
    }

    pub fn active_buffer_mut(&mut self, t: Tab) -> Option<&mut Buffer> {
        let rel = self.ws(t).active.clone()?;
        let i = self.buffer_idx(&rel)?;
        Some(&mut self.buffers[i])
    }

    pub fn insert_snippet(&mut self, t: Tab, snippet: &str) -> Option<String> {
        let b = self.active_buffer_mut(t)?;
        b.insert_snippet(snippet);
        b.last_edit = 0.0;
        Some(b.rel.clone())
    }

    pub fn save_all(&mut self) -> bool {
        let mut any = false;
        let mut bib_saved = false;
        for b in &mut self.buffers {
            if b.dirty() {
                if b.save().is_ok() {
                    any = true;
                    if b.rel == "references.bib" {
                        bib_saved = true;
                    }
                }
            }
        }
        if bib_saved {
            self.shelf.reload();
        }
        if any {
            self.git.invalidate();
        }
        any
    }

    // ───────────── compile ─────────────

    pub fn spec(&self, t: Tab) -> CompileSpec {
        let c = &self.project.config;
        let (main, out, engine) = match t {
            Tab::Slides => (c.slides_main.clone(), ".nedit/build/praesentation", c.slides_engine.clone()),
            _ => (c.thesis_main.clone(), ".nedit/build/arbeit", c.engine.clone()),
        };
        CompileSpec { root: self.project.root.clone(), main, outdir: out.into(), engine }
    }

    pub fn compile(&mut self, t: Tab, ctx: &egui::Context) {
        if t == Tab::Shelf {
            return;
        }
        self.save_all();
        let spec = self.spec(t);
        let ws = self.ws_mut(t);
        if ws.job.is_some() {
            ws.queued = true;
            return;
        }
        ws.job = Some(compile::start(spec, ctx.clone()));
    }

    fn poll_jobs(&mut self, ctx: &egui::Context, now: f64) {
        for t in [Tab::Thesis, Tab::Slides] {
            let res = self.ws(t).job.as_ref().and_then(|j| j.rx.try_recv().ok());
            if let Some(r) = res {
                let errors = r.issues.iter().filter(|i| i.level == Level::Error).count();
                let ws = self.ws_mut(t);
                ws.job = None;
                ws.issues = r.issues;
                ws.raw_log = r.raw_log;
                ws.last_ok = Some(r.ok && errors == 0);
                ws.last_secs = r.duration.as_secs_f32();
                ws.compiled_once = true;
                if let Some(pdf) = r.pdf {
                    if ws.viewer.doc.is_none() {
                        ws.viewer.load(&pdf);
                    } else {
                        ws.viewer.reload_if_changed();
                    }
                }
                if errors > 0 {
                    let name = if t == Tab::Slides { tr!("Präsentation" | "Presentation") } else { tr!("Masterarbeit" | "Thesis") };
                    let red = self.pal.red;
                    self.toast(ic::ERROR, trf!("{name}: {errors} Fehler beim Kompilieren" | "{name}: {errors} compile error(s)"), red, now);
                }
                if self.ws(t).queued {
                    self.ws_mut(t).queued = false;
                    self.compile(t, ctx);
                } else if let Some((rel, line)) = self.ws_mut(t).sync_after.take() {
                    self.forward_sync(t, &rel, line, true, now);
                }
                if t == Tab::Slides {
                    self.thumbs.clear();
                }
            }
        }
    }

    pub fn forward_sync(&mut self, t: Tab, rel: &str, line: usize, flash: bool, now: f64) {
        let spec = self.spec(t);
        let pdf = spec.pdf_path();
        if !pdf.exists() {
            return;
        }
        // blank lines / comments have no SyncTeX record: try the nearest lines around
        let candidates = std::iter::once(line).chain((1..=12).flat_map(|d| [line.saturating_sub(d), line + d])).filter(|l| *l >= 1);
        let hit = candidates.take(25).find_map(|l| compile::synctex_view(&self.project.root, rel, l, &pdf));
        if let Some(b) = hit {
            let ws = self.ws_mut(t);
            if ws.viewer.single_page {
                ws.viewer.current_page = b.page;
            } else {
                ws.viewer.go_to(b.page, b.y);
            }
            if flash {
                ws.viewer.flash(b.page, Rect::from_min_size(pos2(b.x, b.y), vec2(b.w, b.h)), now);
            }
        }
    }

    pub fn inverse_sync(&mut self, t: Tab, page: usize, x: f32, y: f32) {
        let pdf = self.spec(t).pdf_path();
        if let Some((rel, line)) = compile::synctex_edit(&self.project.root, page, x, y, &pdf) {
            self.jump_to(&rel, line, t);
        }
    }

    // ───────────── indexes ─────────────

    pub fn read_source(&self, rel: &str) -> Option<String> {
        if let Some(i) = self.buffer_idx(rel) {
            return Some(self.buffers[i].text.clone());
        }
        std::fs::read_to_string(self.project.root.join(rel)).ok()
    }

    pub fn rebuild_indexes(&mut self) {
        let main = self.project.config.thesis_main.clone();
        let slides = self.project.config.slides_main.clone();
        self.outline = project::outline(&main, &|f| self.read_source(f));
        self.slide_outline = project::outline(&slides, &|f| self.read_source(f));
        let texts: Vec<String> = self.flat.iter().filter(|f| f.ends_with(".tex")).filter_map(|f| self.read_source(f)).collect();
        self.labels = project::scan_labels(&texts);
        if self.cites_rev != self.shelf.revision {
            self.cites = self
                .shelf
                .papers
                .iter()
                .map(|p| CompItem {
                    label: p.entry.key.clone(),
                    detail: format!("{} {} — {}", p.entry.authors_short(), p.entry.year(), p.entry.title()),
                    insert: p.entry.key.clone(),
                })
                .collect();
            self.cites_rev = self.shelf.revision;
        }
    }

    fn handle_shelf_msgs(&mut self, now: f64) {
        while let Ok(m) = self.shelf_rx.try_recv() {
            match m {
                ShelfMsg::Status(s) => {
                    self.shelf_ui.status = s;
                    self.shelf_ui.busy = true;
                }
                ShelfMsg::Fetched { entry, pdf } => {
                    let title = entry.title();
                    let key = self.shelf.add(entry, pdf.as_deref());
                    self.shelf_ui.busy = false;
                    self.shelf_ui.status.clear();
                    self.shelf_ui.input.clear();
                    self.shelf_ui.results.clear();
                    self.shelf_ui.selected = Some(key.clone());
                    let green = self.pal.green;
                    self.toast(ic::BOOK, trf!("Hinzugefügt: {} ({key})" | "Added: {} ({key})", truncate(&title, 48)), green, now);
                    self.after_shelf_change();
                }
                ShelfMsg::SearchResults(r) => {
                    self.shelf_ui.busy = false;
                    self.shelf_ui.status = if r.is_empty() { tr!("Keine Treffer" | "No results").into() } else { String::new() };
                    self.shelf_ui.results = r;
                }
                ShelfMsg::Attach { key, path } => {
                    if let Some(i) = self.shelf.papers.iter().position(|p| p.entry.key == key) {
                        self.shelf.attach_pdf(i, &path);
                        let g = self.pal.green;
                        self.toast(ic::PAPERCLIP, tr!("PDF angehängt" | "PDF attached"), g, now);
                    }
                }
                ShelfMsg::Error(e) => {
                    self.shelf_ui.busy = false;
                    self.shelf_ui.status = e.clone();
                    let red = self.pal.red;
                    self.toast(ic::WARN, e, red, now);
                }
            }
        }
    }

    /// Keep an open references.bib buffer in sync with the shelf.
    pub fn after_shelf_change(&mut self) {
        if let Some(i) = self.buffer_idx("references.bib") {
            let txt = std::fs::read_to_string(crate::shelf::Shelf::bib_path(&self.project.root)).unwrap_or_default();
            if !self.buffers[i].dirty() {
                self.buffers[i].set_text_external(txt);
            }
        }
        self.cites_rev = 0;
        self.rebuild_indexes();
        self.refresh_tree();
    }

    // ───────────── periodic work ─────────────

    fn periodic(&mut self, ctx: &egui::Context, now: f64) {
        self.poll_jobs(ctx, now);
        self.handle_shelf_msgs(now);
        if self.shelf_ui.busy || self.thesis.job.is_some() || self.slides.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // auto-save + auto-compile after idle
        let last_edit = self.buffers.iter().filter(|b| b.dirty()).map(|b| b.last_edit).fold(f64::NAN, f64::max);
        if !last_edit.is_nan() {
            if now - last_edit > 1.2 {
                let edited: Vec<String> = self.buffers.iter().filter(|b| b.dirty()).map(|b| b.rel.clone()).collect();
                self.save_all();
                if self.settings.auto_compile {
                    let slides_dir = std::path::Path::new(&self.project.config.slides_main).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                    let touches_slides = edited.iter().any(|r| (!slides_dir.is_empty() && r.starts_with(&slides_dir)) || r.ends_with(".bib"));
                    let touches_thesis = edited.iter().any(|r| slides_dir.is_empty() || !r.starts_with(&slides_dir));
                    if touches_thesis {
                        self.compile(Tab::Thesis, ctx);
                    }
                    if touches_slides && self.project.has_slides() {
                        self.compile(Tab::Slides, ctx);
                    }
                }
                self.rebuild_indexes();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(300));
            }
        }

        if !self.update_started && now > 4.0 && self.settings.update_check && std::env::var_os("NEDIT_SHOT").is_none() {
            self.update_started = true;
            self.updater.check(false, ctx);
        }
        if let Some(tag) = self.updater.poll() {
            let a = self.pal.accent;
            self.toast(ic::DOWNLOAD, trf!("nEdit {tag} ist verfügbar – oben rechts aktualisieren" | "nEdit {tag} is available – update at the top right"), a, now);
        }
        if self.updater.installing || self.updater.checking {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        if let Some(r) = self.git.poll() {
            match r {
                Ok(m) => {
                    let g = self.pal.green;
                    self.git.status = m.clone();
                    self.toast(ic::GIT, m, g, now);
                }
                Err(e) => {
                    let red = self.pal.red;
                    self.git.status = e.clone();
                    self.toast(ic::WARN, format!("Git: {}", truncate(&e, 90)), red, now);
                }
            }
            let root = self.project.root.clone();
            self.git.refresh(&root, now);
        }
        // git status: in the background every second, immediately after file changes
        if self.git.poll_status() && self.side == SideMode::Git {
            let root = self.project.root.clone();
            self.git.refresh_log(&root);
        }
        if now - self.git.last_refresh > 1.0 && !self.git.busy && self.git.git_available {
            let root = self.project.root.clone();
            self.git.request_status(&root, now, ctx);
        }
        if now - self.last_poll > 1.0 {
            self.last_poll = now;
            // Omarchy theme switch?
            if self.settings.theme.is_none() {
                let st = theme::omarchy_stamp();
                if st != self.omarchy_stamp {
                    self.omarchy_stamp = st;
                    self.pal = load_palette(&self.settings);
                    self.apply_style(ctx);
                    let a = self.pal.accent;
                    let name = self.pal.name.clone();
                    self.toast(ic::BRUSH, trf!("Theme: {name}" | "Theme: {name}"), a, now);
                }
            }
            // pick up files created/removed outside nEdit
            self.refresh_tree();
            // external file changes
            let mut changed = false;
            for b in &mut self.buffers {
                changed |= b.reload_if_changed();
            }
            if self.shelf.poll_external_change() {
                changed = true;
            }
            if changed {
                self.rebuild_indexes();
            }
            let wc_src = self.thesis.active.clone().and_then(|r| self.buffer_idx(&r)).map(|i| &self.buffers[i].text);
            if let Some(t) = wc_src {
                let h = t.len() as u64;
                if h != self.wc_hash {
                    self.wc_hash = h;
                    self.word_count = project::word_count(t);
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(1000));
        }
    }

    fn global_keys(&mut self, ctx: &egui::Context, now: f64) {
        use egui::{Key, Modifiers};
        let (save, compile, find, t1, t2, t3, zin, zout) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::Enter),
                i.consume_key(Modifiers::COMMAND, Key::F),
                i.consume_key(Modifiers::ALT, Key::Num1),
                i.consume_key(Modifiers::ALT, Key::Num2),
                i.consume_key(Modifiers::ALT, Key::Num3),
                i.consume_key(Modifiers::COMMAND, Key::Plus) || i.consume_key(Modifiers::COMMAND, Key::Equals),
                i.consume_key(Modifiers::COMMAND, Key::Minus),
            )
        });
        let (f11, ctrl_e, esc, doc_key) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::F11),
                i.consume_key(Modifiers::COMMAND, Key::E),
                i.key_pressed(Key::Escape),
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::D),
            )
        });
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1)) {
            self.help_open = !self.help_open;
        }
        if f11 && self.tab == Tab::Thesis {
            self.set_focus(!self.focus, ctx);
        }
        if doc_key && self.tab == Tab::Thesis {
            self.set_doc_mode(!self.doc_mode, ctx);
        }
        if ctrl_e && self.tab == Tab::Thesis {
            self.settings.visual = !self.settings.visual;
            self.settings.save();
        }
        if esc && self.focus && ctx.memory(|m| m.focused().is_none()) && self.dialog.is_none() {
            self.set_focus(false, ctx);
        }
        // Ctrl+Tab / Ctrl+Shift+Tab: next / previous file (document mode: next chapter)
        let (tab_prev, tab_next) = ctx.input_mut(|i| {
            let prev = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab);
            let next = i.consume_key(Modifiers::COMMAND, Key::Tab);
            (prev, next)
        });
        if (tab_next || tab_prev) && self.tab != Tab::Shelf && self.quick.is_none() {
            self.cycle_file(if tab_next { 1 } else { -1 });
        }
        let doc = if self.tab == Tab::Shelf { self.last_doc_tab } else { self.tab };
        if save || compile {
            let pos = self.ws(doc).active.clone().and_then(|r| self.buffer_idx(&r).map(|i| (r, self.buffers[i].line)));
            self.ws_mut(doc).sync_after = pos;
        }
        if save {
            self.save_all();
            self.compile(doc, ctx);
            self.rebuild_indexes();
        }
        if compile {
            self.compile(doc, ctx);
        }
        if find && self.tab != Tab::Shelf {
            self.find.open = true;
            self.find.focus = true;
            if let Some(b) = self.active_buffer_mut(doc) {
                let (a, z) = (b.cursor.min(b.sel_end), b.cursor.max(b.sel_end));
                if z > a && z - a < 80 {
                    let sel: String = b.text.chars().skip(a).take(z - a).collect();
                    self.find.query = sel;
                }
            }
        }
        if t1 {
            self.tab = Tab::Thesis;
        }
        if t2 {
            self.tab = Tab::Slides;
        }
        if t3 {
            self.tab = Tab::Shelf;
        }
        if zin {
            self.settings.font_size = (self.settings.font_size + 1.0).min(28.0);
            self.settings.save();
        }
        if zout {
            self.settings.font_size = (self.settings.font_size - 1.0).max(9.0);
            self.settings.save();
        }
        let _ = now;
    }

    fn handle_drops(&mut self, ctx: &egui::Context, now: f64) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if dropped.is_empty() {
            return;
        }
        // dropped onto the file tree → copy into the hovered folder
        let over_tree = self.tab != Tab::Shelf
            && self.side == SideMode::Files
            && self.tree_ui.rect.is_some_and(|r| ctx.input(|i| i.pointer.latest_pos()).is_some_and(|p| r.contains(p)));
        if over_tree {
            let dir = self.tree_ui.hover_dir.clone().unwrap_or_else(|| self.target_dir());
            let n = self.import_files(&dropped, &dir);
            let g = self.pal.green;
            self.toast(ic::DOWNLOAD, trf!("{n} Datei(en) nach /{dir} kopiert" | "{n} file(s) copied to /{dir}"), g, now);
            return;
        }
        let pdfs: Vec<PathBuf> = dropped.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))).cloned().collect();
        let images: Vec<PathBuf> = dropped
            .iter()
            .filter(|p| p.extension().is_some_and(|e| ["png", "jpg", "jpeg", "svg", "eps"].contains(&e.to_string_lossy().to_lowercase().as_str())))
            .cloned()
            .collect();
        if !pdfs.is_empty() && (self.tab == Tab::Shelf || images.is_empty()) {
            self.tab = Tab::Shelf;
            self.shelf_ui.busy = true;
            crate::shelf::import_pdfs(pdfs, self.shelf_tx.clone(), ctx.clone());
        }
        for img in images {
            let name = img.file_name().unwrap().to_string_lossy().to_string();
            let dst_rel = format!("abbildungen/{name}");
            let _ = std::fs::create_dir_all(self.project.root.join("abbildungen"));
            if std::fs::copy(&img, self.project.root.join(&dst_rel)).is_ok() {
                let stem = img.file_stem().unwrap().to_string_lossy().to_string();
                let t = if self.tab == Tab::Slides { Tab::Slides } else { Tab::Thesis };
                let snip = if t == Tab::Slides {
                    format!("\\begin{{frame}}{{$0{stem}}}\n  \\centering\n  \\includegraphics[width=0.8\\textwidth,height=0.7\\textheight,keepaspectratio]{{../{dst_rel}}}\n\\end{{frame}}\n")
                } else {
                    format!("\\begin{{figure}}[htbp]\n  \\centering\n  \\includegraphics[width=0.8\\linewidth]{{{name}}}\n  \\caption{{$0}}\n  \\label{{fig:{stem}}}\n\\end{{figure}}\n")
                };
                self.insert_snippet(t, &snip);
                let g = self.pal.green;
                self.toast(ic::IMAGE, trf!("Abbildung {name} eingefügt" | "Figure {name} inserted"), g, now);
            }
        }
        self.refresh_tree();
    }
}

fn load_palette(s: &Settings) -> Palette {
    // NEDIT_THEME=<omarchy theme name | nedit> overrides the setting (handy for screenshots)
    let env_theme = std::env::var("NEDIT_THEME").ok();
    match env_theme.as_ref().or(s.theme.as_ref()) {
        Some(name) if name == "nedit" => Palette::default_dark(),
        Some(name) => theme::load_named(name).unwrap_or_else(Palette::default_dark),
        None => theme::load_omarchy_current().unwrap_or_else(Palette::default_dark),
    }
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

// ───────────────────────────── frame ─────────────────────────────

impl eframe::App for App {
    /// egui drops keyboard focus on a plain Esc before any widget sees it. In vim mode we
    /// tag that Esc (harmless modifier) so the editor keeps focus and gets Normal mode.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // screenshot runs: a step named "...rawesc..." sends a real, unmodified Esc once
        if let Ok(spec) = std::env::var("NEDIT_SHOT") {
            if let Some((_, steps)) = spec.split_once(':') {
                if let Some((name, _)) = steps.split(',').nth(self.shot_step).and_then(|s| s.split_once('@')) {
                    if name.contains("tabdrag") {
                        // drag the first tab to the far right over ~14 frames (raw input, so egui sees real pointer events)
                        let rects: Vec<(String, egui::Rect)> = ctx.data(|d| d.get_temp(egui::Id::new("dbg-tabrects"))).unwrap_or_default();
                        if self.debug_typed < 4000 {
                            self.debug_typed = 4000;
                            eprintln!("TABDRAG before {:?}", self.thesis.tabs);
                        }
                        let k = self.debug_typed - 4000;
                        let fixed_id = egui::Id::new("dbg-tabdrag-fixed");
                        if k == 0 {
                            if let (Some(first), Some(last)) = (rects.first(), rects.last()) {
                                let start = first.1.center();
                                ctx.data_mut(|d| d.insert_temp(fixed_id, (start, egui::pos2(last.1.max.x - 10.0, start.y))));
                            }
                        }
                        let fixed: Option<(egui::Pos2, egui::Pos2)> = ctx.data(|d| d.get_temp(fixed_id));
                        if let Some((start, end)) = fixed {
                            let p = |f: f32| start + (end - start) * f;
                            match k {
                                0 => raw.events.push(egui::Event::PointerMoved(start)),
                                1 => raw.events.push(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }),
                                2..=13 => raw.events.push(egui::Event::PointerMoved(p((k - 1) as f32 / 12.0))),
                                14 => raw.events.push(egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }),
                                16 => eprintln!("TABDRAG after {:?}", self.thesis.tabs),
                                _ => {}
                            }
                            if k < 17 {
                                self.debug_typed += 1;
                            }
                        }
                    }
                    if name.contains("rawesc") && self.debug_typed != 1000 + self.shot_step {
                        self.debug_typed = 1000 + self.shot_step;
                        raw.events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
                    }
                }
            }
        }
        if !self.settings.input_vim {
            return;
        }
        let focused = ctx.memory(|m| m.focused());
        if !focused.is_some_and(|f| self.buffers.iter().any(|b| b.id == f)) {
            return;
        }
        for e in &mut raw.events {
            if let egui::Event::Key { key: egui::Key::Escape, modifiers, .. } = e {
                if modifiers.is_none() {
                    modifiers.mac_cmd = true;
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if self.close_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Ok(spec) = std::env::var("NEDIT_SHOT") {
            self.debug_shots(&ctx, now, &spec);
        }
        self.periodic(&ctx, now);
        self.handle_drops(&ctx, now);

        if self.present.is_some() {
            crate::workspace::present_ui(self, ui, now);
            return;
        }
        self.quick_shortcut(&ctx, now);
        self.global_keys(&ctx, now);
        let pal = self.pal.clone();
        if self.focus && self.tab == Tab::Thesis {
            crate::workspace::focus_ui(self, ui, now);
            self.dialogs(&ctx, &pal, now);
            self.about_window(&ctx, &pal);
            self.help_window(&ctx, &pal);
            self.quick_ui(&ctx, now);
            self.draw_toasts(&ctx, &pal, now);
            return;
        }

        self.top_bar(ui, &pal, now);
        self.status_bar(ui, &pal);

        match self.tab {
            Tab::Thesis if self.doc_mode => crate::workspace::document_ui(self, ui, now),
            Tab::Thesis => crate::workspace::thesis_ui(self, ui, now),
            Tab::Slides => crate::workspace::slides_ui(self, ui, now),
            Tab::Shelf => crate::shelf_ui::shelf_ui(self, ui, now),
        }
        if self.tab != Tab::Shelf {
            self.last_doc_tab = self.tab;
        }
        self.dialogs(&ctx, &pal, now);
        self.about_window(&ctx, &pal);
        self.help_window(&ctx, &pal);
        self.quick_ui(&ctx, now);
        self.draw_toasts(&ctx, &pal, now);
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui, pal: &Palette, now: f64) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("topbar")
            .exact_size(56.0)
            .frame(egui::Frame::new().fill(pal.mantle).inner_margin(egui::Margin::symmetric(14, 0)))
            .show_separator_line(false)
            .show(ui, |ui| {
                let full = ui.max_rect();
                ui.painter().line_segment([pos2(full.min.x - 14.0, full.max.y), pos2(full.max.x + 14.0, full.max.y)], Stroke::new(1.0, with_alpha(pal.border, 150)));
                ui.horizontal_centered(|ui| {
                    // logo
                    let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 9.0, pal.accent);
                    ui.painter().text(r.center() + vec2(0.0, -1.0), Align2::CENTER_CENTER, "∂", widgets::display_font(21.0), pal.on_accent);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.add_space(9.0);
                        ui.label(egui::RichText::new("nEdit").font(widgets::bold_font(14.0)).color(pal.bright));
                        ui.label(egui::RichText::new("LaTeX Studio").font(widgets::ui_font(10.5)).color(pal.dim));
                    });
                    ui.add_space(14.0);
                    // project switcher
                    let name = self.project.config.name.clone();
                    let resp = ui.add(egui::Button::new(egui::RichText::new(format!("{}   {name}   {}", ic::GRADUATION, ic::CHEVRON_DOWN)).font(widgets::ui_font(13.5)).color(pal.text)).fill(pal.surface).corner_radius(8));
                    egui::Popup::menu(&resp).show(|ui| {
                        ui.set_min_width(240.0);
                        widgets::section_label(ui, tr!("Projekte" | "Projects"), pal);
                        for p in project::list_projects() {
                            let n = p.file_name().unwrap().to_string_lossy().to_string();
                            let cur = p == self.project.root;
                            if ui.selectable_label(cur, format!("{}  {n}", if cur { ic::CHECK } else { ic::FOLDER })).clicked() && !cur {
                                self.save_all();
                                if let Ok(pr) = Project::open(&p) {
                                    self.project = pr;
                                    self.shelf = Shelf::load(&self.project.root);
                                    self.cites_rev = 0;
                                    self.after_project_open(&ctx);
                                }
                            }
                        }
                        ui.separator();
                        if ui.button(trf!("{}  Neues Projekt …" | "{}  New project …", ic::PLUS)).clicked() {
                            self.dialog = Some(Dialog::NewProject { name: String::new() });
                        }
                        if ui.button(trf!("{}  Ordner öffnen" | "{}  Open folder", ic::FOLDER_OPEN)).clicked() {
                            crate::platform::open_external(&self.project.root);
                        }
                    });

                    // center: tabs
                    let seg_w = 470.0;
                    let center_x = full.center().x - seg_w / 2.0;
                    let cur_x = ui.cursor().min.x;
                    if center_x > cur_x + 10.0 {
                        ui.add_space(center_x - cur_x);
                    } else {
                        ui.add_space(10.0);
                    }
                    let mut sel = match self.tab {
                        Tab::Thesis => 0,
                        Tab::Slides => 1,
                        Tab::Shelf => 2,
                    };
                    let n_papers = self.shelf.papers.len();
                    let items = [
                        (ic::FILE_TEXT, tr!("Masterarbeit" | "Thesis"), None),
                        (ic::TV, tr!("Präsentation" | "Presentation"), None),
                        (ic::BOOK, tr!("Bibliothek" | "Library"), Some(n_papers.to_string())),
                    ];
                    if widgets::segmented(ui, "maintabs", &items, &mut sel, pal) {
                        self.tab = [Tab::Thesis, Tab::Slides, Tab::Shelf][sel];
                    }

                    // right side
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // settings
                        let resp = widgets::icon_button(ui, ic::COG, tr!("Einstellungen" | "Settings"), pal, false);
                        egui::Popup::menu(&resp).show(|ui| {
                            ui.set_min_width(260.0);
                            widgets::section_label(ui, tr!("Sprache" | "Language"), pal);
                            ui.horizontal(|ui| {
                                let mut ch = false;
                                ch |= ui.selectable_value(&mut self.settings.lang_en, false, "Deutsch").changed();
                                ch |= ui.selectable_value(&mut self.settings.lang_en, true, "English").changed();
                                if ch {
                                    crate::i18n::set_english(self.settings.lang_en);
                                    self.settings.save();
                                }
                            });
                            widgets::section_label(ui, "Editor", pal);
                            ui.horizontal(|ui| {
                                ui.label(tr!("Eingabe" | "Input"));
                                let mut ch = false;
                                ch |= ui.selectable_value(&mut self.settings.input_vim, false, "Standard").changed();
                                ch |= ui.selectable_value(&mut self.settings.input_vim, true, format!("{}  Vim", ic::TERMINAL)).changed();
                                if ch {
                                    self.settings.save();
                                    self.vim = Default::default();
                                }
                            });
                            if self.settings.input_vim {
                                ui.label(egui::RichText::new(tr!("Vim gilt im Code-Modus; im visuellen Modus bleibt die Standardeingabe." | "Vim applies in code mode; visual mode keeps the standard input.")).font(widgets::ui_font(11.0)).color(pal.dim));
                            }
                            ui.horizontal(|ui| {
                                ui.label(tr!("Schriftgröße" | "Font size"));
                                if widgets::fancy_slider(ui, &mut self.settings.font_size, 10.0, 24.0, 0.5, "pt", pal).changed() {
                                    self.settings.save();
                                }
                            });
                            if ui.checkbox(&mut self.settings.auto_compile, tr!("Automatisch kompilieren" | "Compile automatically")).changed() {
                                self.settings.save();
                            }
                            if ui.checkbox(&mut self.settings.dark_pdf, tr!("PDF im Dark-Mode abdunkeln" | "Dim PDF in dark mode")).changed() {
                                self.settings.save();
                                self.thesis.viewer.dark_pages = self.settings.dark_pdf && pal.dark;
                            }
                            widgets::section_label(ui, "Compiler", pal);
                            let mut changed = false;
                            ui.horizontal(|ui| {
                                ui.label(tr!("Arbeit" | "Thesis"));
                                for e in ["pdflatex", "xelatex", "lualatex"] {
                                    changed |= ui.selectable_value(&mut self.project.config.engine, e.to_string(), e).changed();
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label(tr!("Folien" | "Slides"));
                                for e in ["pdflatex", "xelatex", "lualatex"] {
                                    changed |= ui.selectable_value(&mut self.project.config.slides_engine, e.to_string(), e).changed();
                                }
                            });
                            if ui.button(format!("{}  {}", ic::BOOKMARK, tr!("Hilfe & Tastenkürzel …   F1" | "Help & shortcuts …   F1"))).clicked() {
                                self.help_open = true;
                                ui.close();
                            }
                            if ui.button(trf!("{}  Über nEdit …" | "{}  About nEdit …", ic::GRADUATION)).clicked() {
                                self.about_open = true;
                                ui.close();
                            }
                            widgets::section_label(ui, "Updates", pal);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("Version {}", crate::updater::VERSION)).color(pal.subtext));
                                if ui.add_enabled(!self.updater.checking, egui::Button::new(trf!("{}  Nach Updates suchen" | "{}  Check for updates", ic::REFRESH))).clicked() {
                                    self.updater.check(true, &ctx);
                                }
                            });
                            if ui.checkbox(&mut self.settings.update_check, tr!("Beim Start nach Updates suchen" | "Check for updates on start")).changed() {
                                self.settings.save();
                            }
                            if !self.updater.status.is_empty() {
                                ui.label(egui::RichText::new(&self.updater.status).font(widgets::ui_font(11.5)).color(pal.dim));
                            }
                            if self.updater.available.is_some() && ui.button(trf!("{}  Update anzeigen" | "{}  Show update", ic::DOWNLOAD)).clicked() {
                                self.dialog = Some(Dialog::Update);
                            }
                            widgets::section_label(ui, tr!("Präsentation" | "Presentation"), pal);
                            ui.horizontal(|ui| {
                                ui.label(tr!("Redezeit" | "Talk length"));
                                changed |= ui.add(egui::DragValue::new(&mut self.project.config.talk_minutes).range(1..=120).suffix(" min")).changed();
                            });
                            if changed {
                                let _ = self.project.save_config();
                            }
                        });
                        // theme picker
                        let resp = widgets::icon_button(ui, ic::BRUSH, "Theme", pal, false);
                        egui::Popup::menu(&resp).show(|ui| {
                            ui.set_min_width(250.0);
                            widgets::section_label(ui, "Theme", pal);
                            let follow = self.settings.theme.is_none();
                            let cur = theme::current_omarchy_name().map(|n| theme::pretty_name(&n)).unwrap_or_default();
                            if ui.selectable_label(follow, trf!("{}  Omarchy folgen  ·  {cur}" | "{}  Follow Omarchy  ·  {cur}", ic::MAGIC)).clicked() {
                                self.set_theme(None, &ctx);
                            }
                            if ui.selectable_label(self.settings.theme.as_deref() == Some("nedit"), format!("{}  nEdit Ink", ic::MOON)).clicked() {
                                self.set_theme(Some("nedit".into()), &ctx);
                            }
                            ui.separator();
                            egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                                for (name, path) in theme::list_themes() {
                                    let sel = self.settings.theme.as_deref() == Some(name.as_str());
                                    let p = theme::Palette::load_dir(&path, &name);
                                    ui.horizontal(|ui| {
                                        if let Some(p) = &p {
                                            let (r, _) = ui.allocate_exact_size(vec2(44.0, 16.0), egui::Sense::hover());
                                            let cols = [p.base, p.accent, p.green, p.magenta];
                                            for (k, c) in cols.iter().enumerate() {
                                                let rr = Rect::from_min_size(pos2(r.min.x + k as f32 * 11.0, r.min.y), vec2(11.0, 16.0));
                                                ui.painter().rect_filled(rr, 2.0, *c);
                                            }
                                            ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Outside);
                                        }
                                        if ui.selectable_label(sel, theme::pretty_name(&name)).clicked() {
                                            self.set_theme(Some(name.clone()), &ctx);
                                        }
                                    });
                                }
                            });
                        });
                        if self.updater.installed {
                            if widgets::button(ui, ic::REFRESH, tr!("Neu starten" | "Restart"), pal, BtnKind::Primary).on_hover_text(tr!("Update installiert – nEdit neu starten" | "Update installed – restart nEdit")).clicked() {
                                self.save_all();
                                match crate::updater::restart() {
                                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                                    Err(e) => {
                                        let r = pal.red;
                                        self.toast(ic::WARN, e, r, now);
                                    }
                                }
                            }
                        } else if let Some(rel) = &self.updater.available {
                            let label = format!("Update {}", rel.tag);
                            if widgets::button(ui, ic::DOWNLOAD, &label, pal, BtnKind::Secondary).on_hover_text(tr!("Neue nEdit-Version verfügbar" | "New nEdit version available")).clicked() {
                                self.dialog = Some(Dialog::Update);
                            }
                        }
                        ui.add_space(6.0);
                        if self.tab != Tab::Shelf {
                            let t = self.tab;
                            let running = self.ws(t).job.is_some();
                            let label = if running { tr!("Kompiliert …" | "Compiling …") } else { tr!("Kompilieren" | "Compile") };
                            let r = widgets::button(ui, if running { "" } else { ic::PLAY }, label, pal, BtnKind::Primary);
                            if running {
                                widgets::draw_spinner(ui, pos2(r.rect.min.x + 18.0, r.rect.center().y), 6.0, pal.on_accent);
                            }
                            if r.on_hover_text(tr!("Strg+Enter" | "Ctrl+Enter")).clicked() {
                                self.compile(t, &ctx);
                            }
                            if t == Tab::Slides {
                                ui.add_space(4.0);
                                if widgets::button(ui, ic::PLAY, tr!("Präsentieren" | "Present"), pal, BtnKind::Secondary).on_hover_text("F5").clicked() {
                                    crate::workspace::start_presentation(self, &ctx, now);
                                }
                            }
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(egui::Frame::new().fill(pal.mantle).inner_margin(egui::Margin::symmetric(14, 0)))
            .show_separator_line(false)
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment([pos2(r.min.x - 14.0, r.min.y), pos2(r.max.x + 14.0, r.min.y)], Stroke::new(1.0, with_alpha(pal.border, 150)));
                ui.horizontal_centered(|ui| {
                    let small = |t: String, c: Color32| egui::RichText::new(t).font(widgets::ui_font(11.5)).color(c);
                    let doc = if self.tab == Tab::Shelf { self.last_doc_tab } else { self.tab };
                    if let Some(rel) = self.ws(doc).active.clone() {
                        if let Some(i) = self.buffer_idx(&rel) {
                            let b = &self.buffers[i];
                            ui.label(small(format!("{}  {}", ic::FILE_TEXT, b.rel), pal.subtext));
                            ui.label(small(trf!("Zeile {}, Spalte {}" | "Line {}, column {}", b.line, b.col), pal.dim));
                            if b.dirty() {
                                ui.label(small(trf!("{} ungespeichert" | "{} unsaved", ic::CIRCLE), pal.yellow));
                            }
                        }
                    }
                    let vim_active = self.settings.input_vim && self.tab != Tab::Shelf && !(doc == Tab::Thesis && self.settings.visual);
                    if vim_active {
                        let (mode, info) = self.vim.status();
                        let col = match self.vim.mode {
                            crate::vim::Mode::Normal => pal.accent,
                            crate::vim::Mode::Insert => pal.green,
                            _ => pal.magenta,
                        };
                        let g = ui.painter().layout_no_wrap(mode.clone(), widgets::bold_font(11.0), pal.on_accent);
                        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 14.0, 18.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 4.0, col);
                        ui.painter().galley(pos2(r.min.x + 7.0, r.center().y - g.size().y / 2.0), g, pal.on_accent);
                        if !info.is_empty() {
                            ui.label(small(info, pal.subtext));
                        }
                    }
                    if doc == Tab::Thesis {
                        ui.label(small(trf!("{} Wörter" | "{} words", fmt_thousands(self.word_count)), pal.dim));
                    }
                    if self.git.is_repo {
                        let n = self.git.changes.len();
                        let txt = if n == 0 { format!("{}  {}  ✓", ic::BRANCH, self.git.branch) } else { trf!("{}  {}  ·  {n} geändert" | "{}  {}  ·  {n} changed", ic::BRANCH, self.git.branch) };
                        let col = if n == 0 { pal.dim } else { widgets::git_color('M', pal) };
                        let r = ui.add(egui::Label::new(small(txt, col)).sense(egui::Sense::click())).on_hover_text(tr!("Git-Panel öffnen" | "Open Git panel"));
                        if r.clicked() {
                            self.side = SideMode::Git;
                            if self.tab == Tab::Shelf {
                                self.tab = Tab::Thesis;
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(small(format!("{}  {}", ic::BRUSH, self.pal.name), pal.dim));
                        ui.add_space(10.0);
                        let ws = self.ws(doc);
                        if ws.job.is_some() {
                            ui.label(small(trf!("{} Kompiliere …" | "{} Compiling …", ic::REFRESH), pal.accent));
                        } else if let Some(ok) = ws.last_ok {
                            let (e, w) = (ws.errors(), ws.warnings());
                            if ok {
                                ui.label(small(trf!("{} Kompiliert in {:.1} s" | "{} Compiled in {:.1} s", ic::CHECK_CIRCLE, ws.last_secs), pal.green));
                            } else {
                                ui.label(small(trf!("{} {e} Fehler" | "{} {e} error(s)", ic::ERROR), pal.red));
                            }
                            if w > 0 {
                                ui.label(small(format!("{} {w}", ic::WARN), pal.yellow));
                            }
                        }
                    });
                });
            });
    }

    fn dialogs(&mut self, ctx: &egui::Context, pal: &Palette, now: f64) {
        let Some(dialog) = &mut self.dialog else { return };
        let mut close = false;
        let mut action: Option<Box<dyn FnOnce(&mut App)>> = None;
        let modal = egui::Modal::new(egui::Id::new("dialog")).backdrop_color(with_alpha(Color32::BLACK, 120)).show(ctx, |ui| {
            ui.set_width(380.0);
            match dialog {
                Dialog::Delete { rel } => {
                    ui.label(egui::RichText::new(tr!("Löschen?" | "Delete?")).font(widgets::display_font(20.0)).color(pal.bright));
                    ui.add_space(6.0);
                    ui.label(trf!("„{rel}“ wird endgültig gelöscht." | "“{rel}” will be deleted permanently."));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if widgets::button(ui, ic::TRASH, tr!("Löschen" | "Delete"), pal, BtnKind::Danger).clicked() {
                            let rel = rel.clone();
                            action = Some(Box::new(move |app: &mut App| app.delete_path(&rel)));
                            close = true;
                        }
                        if widgets::button(ui, "", tr!("Abbrechen" | "Cancel"), pal, BtnKind::Ghost).clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::GitRestore { hash, path } => {
                    ui.label(egui::RichText::new(tr!("Datei wiederherstellen?" | "Restore file?")).font(widgets::display_font(20.0)).color(pal.bright));
                    ui.add_space(6.0);
                    ui.label(trf!("„{path}“ wird auf den Stand von Commit {} zurückgesetzt. Nicht committete Änderungen an dieser Datei gehen verloren." | "“{path}” will be reset to commit {}. Uncommitted changes to this file will be lost.", &hash[..hash.len().min(7)]));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if widgets::button(ui, ic::UNDO, tr!("Wiederherstellen" | "Restore"), pal, BtnKind::Danger).clicked() {
                            let (hash, path) = (hash.clone(), path.clone());
                            action = Some(Box::new(move |app: &mut App| {
                                let root = app.project.root.clone();
                                let now = 0.0;
                                match crate::git::GitState::restore_file(&root, &hash, &path) {
                                    Ok(()) => {
                                        let g = app.pal.green;
                                        app.toast(ic::UNDO, trf!("{path} wiederhergestellt" | "{path} restored"), g, now);
                                    }
                                    Err(e) => {
                                        let r = app.pal.red;
                                        app.toast(ic::WARN, e, r, now);
                                    }
                                }
                                app.sync_buffers_with_disk(std::slice::from_ref(&path));
                                app.git.refresh(&root, now);
                            }));
                            close = true;
                        }
                        if widgets::button(ui, "", tr!("Abbrechen" | "Cancel"), pal, BtnKind::Ghost).clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::GitRevert { paths } => {
                    let files: Vec<crate::git::Change> = {
                        let mut v: Vec<crate::git::Change> = paths.iter().flat_map(|p| self.git.changes_under(p)).collect();
                        v.sort_by(|a, b| a.path.cmp(&b.path));
                        v.dedup_by(|a, b| a.path == b.path);
                        v
                    };
                    ui.label(egui::RichText::new(tr!("Änderungen verwerfen?" | "Discard changes?")).font(widgets::display_font(20.0)).color(pal.bright));
                    ui.add_space(6.0);
                    if files.is_empty() {
                        ui.label(egui::RichText::new(tr!("Hier gibt es keine Änderungen seit dem letzten Commit." | "There are no changes since the last commit.")).color(pal.subtext));
                        ui.add_space(10.0);
                        if widgets::button(ui, "", tr!("Schließen" | "Close"), pal, BtnKind::Ghost).clicked() {
                            close = true;
                        }
                    } else {
                        ui.label(trf!("{} Datei(en) werden auf den Stand des letzten Commits zurückgesetzt. Neue Dateien werden gelöscht – auch in geöffneten Editoren." | "{} file(s) will be reset to the last commit. New files will be deleted – also in open editors.", files.len()));
                        ui.add_space(8.0);
                        egui::Frame::new().fill(pal.base).corner_radius(8).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                                for c in &files {
                                    let (label, letter) = c.label();
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(letter.to_string()).font(widgets::mono_font(12.0)).color(widgets::git_color(letter, pal)));
                                        ui.label(egui::RichText::new(&c.path).font(widgets::ui_font(12.5)).color(pal.text));
                                        ui.label(egui::RichText::new(label).font(widgets::ui_font(11.0)).color(pal.dim));
                                    });
                                }
                            });
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if widgets::button(ui, ic::UNDO, tr!("Verwerfen" | "Discard"), pal, BtnKind::Danger).clicked() {
                                let paths = paths.clone();
                                action = Some(Box::new(move |app: &mut App| app.git_revert(&paths, now)));
                                close = true;
                            }
                            if widgets::button(ui, "", tr!("Abbrechen" | "Cancel"), pal, BtnKind::Ghost).clicked() {
                                close = true;
                            }
                        });
                    }
                }
                Dialog::Update => {
                    let up = &mut self.updater;
                    let tag = up.available.as_ref().map(|r| r.tag.clone()).unwrap_or_default();
                    ui.set_width(460.0);
                    ui.label(egui::RichText::new(trf!("Update auf {tag}" | "Update to {tag}")).font(widgets::display_font(22.0)).color(pal.bright));
                    ui.label(egui::RichText::new(trf!("Installiert: {}" | "Installed: {}", crate::updater::VERSION)).color(pal.dim));
                    ui.add_space(8.0);
                    if let Some(rel) = &up.available {
                        if !rel.notes.trim().is_empty() {
                            egui::Frame::new().fill(pal.base).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                    ui.label(egui::RichText::new(rel.notes.replace("**", "").replace("## ", "").replace("### ", "")).font(widgets::ui_font(12.5)).color(pal.text));
                                });
                            });
                            ui.add_space(8.0);
                        }
                    }
                    if let Some((done, total)) = up.progress {
                        let f = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                        ui.add(egui::ProgressBar::new(f).text(format!("{:.1} / {:.1} MB", done as f32 / 1e6, total as f32 / 1e6)));
                        ui.add_space(6.0);
                    }
                    if !up.status.is_empty() {
                        ui.label(egui::RichText::new(&up.status).font(widgets::ui_font(12.0)).color(pal.subtext));
                        ui.add_space(6.0);
                    }
                    if let Some(src) = crate::updater::source_checkout() {
                        ui.label(egui::RichText::new(tr!("nEdit läuft aus einem Quellcode-Checkout. Aktualisieren mit:" | "nEdit runs from a source checkout. Update with:")).color(pal.subtext));
                        let cmd = format!("cd {} && git pull && ./install.sh", src.display());
                        ui.label(egui::RichText::new(&cmd).font(widgets::mono_font(12.0)).color(pal.accent));
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if widgets::button(ui, ic::COPY, tr!("Befehl kopieren" | "Copy command"), pal, BtnKind::Primary).clicked() {
                                ui.ctx().copy_text(cmd.clone());
                            }
                            if widgets::button(ui, "", tr!("Schließen" | "Close"), pal, BtnKind::Ghost).clicked() {
                                close = true;
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            if up.installed {
                                if widgets::button(ui, ic::REFRESH, tr!("Jetzt neu starten" | "Restart now"), pal, BtnKind::Primary).clicked() {
                                    action = Some(Box::new(|app: &mut App| {
                                        app.save_all();
                                        match crate::updater::restart() {
                                            Ok(()) => app.close_requested = true,
                                            Err(e) => app.updater.status = e,
                                        }
                                    }));
                                }
                            } else if up.installing {
                                let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
                                widgets::draw_spinner(ui, r.center(), 7.0, pal.accent);
                            } else if widgets::button(ui, ic::DOWNLOAD, tr!("Jetzt aktualisieren" | "Update now"), pal, BtnKind::Primary).clicked() {
                                up.install(ctx);
                            }
                            if let Some(rel) = &up.available {
                                if widgets::button(ui, ic::GLOBE, tr!("Auf GitHub" | "On GitHub"), pal, BtnKind::Ghost).clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(rel.html_url.clone()));
                                }
                            }
                            if !up.installing && widgets::button(ui, "", tr!("Später" | "Later"), pal, BtnKind::Ghost).clicked() {
                                close = true;
                            }
                        });
                    }
                }
                Dialog::NewProject { name } => {
                    ui.label(egui::RichText::new(tr!("Neues Projekt" | "New project")).font(widgets::display_font(20.0)).color(pal.bright));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(tr!("Mit Vorlage für Arbeit, TU-Graz-Präsentation und Bibliothek." | "With template for thesis, TU Graz presentation and library.")).color(pal.dim));
                    ui.add_space(8.0);
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text(tr!("Projektname" | "Project name")).desired_width(f32::INFINITY));
                    r.request_focus();
                    ui.add_space(10.0);
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        if (widgets::button(ui, ic::PLUS, tr!("Erstellen" | "Create"), pal, BtnKind::Primary).clicked() || enter) && !name.trim().is_empty() {
                            let name = name.trim().to_string();
                            let ctx2 = ctx.clone();
                            action = Some(Box::new(move |app: &mut App| {
                                app.save_all();
                                if let Ok(p) = Project::create(&name, "") {
                                    app.project = p;
                                    app.shelf = Shelf::load(&app.project.root);
                                    app.cites_rev = 0;
                                    app.after_project_open(&ctx2);
                                }
                            }));
                            close = true;
                        }
                        if widgets::button(ui, "", tr!("Abbrechen" | "Cancel"), pal, BtnKind::Ghost).clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(a) = action {
            a(self);
        }
        if close {
            self.dialog = None;
        }
        let _ = now;
    }

    /// Movable tr!("Über nEdit" | "About nEdit") window.
    fn about_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.about_open {
            return;
        }
        let mut open = true;
        let screen = ctx.content_rect();
        egui::Window::new(egui::RichText::new(tr!("Über nEdit" | "About nEdit")).font(widgets::ui_font(13.0)).color(pal.text))
            .id(egui::Id::new("about-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .movable(true)
            .pivot(Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .frame(
                egui::Frame::new()
                    .fill(pal.surface)
                    .stroke(Stroke::new(1.0, pal.border))
                    .corner_radius(14)
                    .inner_margin(egui::Margin::same(22))
                    .shadow(egui::Shadow { offset: [0, 14], blur: 40, spread: 0, color: with_alpha(Color32::BLACK, 120) }),
            )
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(84.0, 84.0), egui::Sense::hover());
                    ui.painter().rect_filled(r.translate(vec2(0.0, 4.0)), 22.0, with_alpha(pal.accent, 50));
                    ui.painter().rect_filled(r, 22.0, pal.accent);
                    ui.painter().text(r.center() + vec2(0.0, -3.0), Align2::CENTER_CENTER, "∂", widgets::display_font(58.0), pal.on_accent);
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new("nEdit").font(widgets::display_font(30.0)).color(pal.bright));
                    ui.label(egui::RichText::new(trf!("LaTeX Studio  ·  Version {}" | "LaTeX Studio  ·  version {}", crate::updater::VERSION)).font(widgets::ui_font(12.5)).color(pal.dim));
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new(tr!("Erstellt von Norbert Winter – zur Motivation, die Masterarbeit endlich fertigzustellen." | "Created by Norbert Winter – as motivation to finally finish the master's thesis."))
                            .font(widgets::FontIdExt::serif(17.0))
                            .italics()
                            .color(pal.text),
                    );
                    ui.add_space(16.0);
                    ui.painter().line_segment(
                        [pos2(ui.max_rect().center().x - 30.0, ui.cursor().min.y), pos2(ui.max_rect().center().x + 30.0, ui.cursor().min.y)],
                        Stroke::new(1.5, pal.accent),
                    );
                    ui.add_space(14.0);
                    ui.hyperlink_to(egui::RichText::new(format!("{}  github.com/jwm3000/nedit", ic::GLOBE)).font(widgets::ui_font(12.5)), "https://github.com/jwm3000/nedit");
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(tr!("MIT-Lizenz  ·  Rust & egui  ·  Icons: Nerd Fonts" | "MIT license  ·  Rust & egui  ·  Icons: Nerd Fonts")).font(widgets::ui_font(11.0)).color(pal.dim));
                });
            });
        if !open {
            self.about_open = false;
        }
    }

    fn draw_toasts(&mut self, ctx: &egui::Context, pal: &Palette, now: f64) {
        self.toasts.retain(|t| now - t.t0 < 4.5);
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint();
        let screen = ctx.content_rect();
        let mut y = screen.max.y - 44.0;
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("toasts")));
        for t in self.toasts.iter().rev() {
            let age = (now - t.t0) as f32;
            let a = (age * 6.0).min(1.0).min((4.5 - age) * 2.0).clamp(0.0, 1.0);
            let galley = painter.layout_no_wrap(t.msg.clone(), widgets::ui_font(13.0), with_alpha(pal.text, (a * 255.0) as u8));
            let w = galley.size().x + 56.0;
            let slide = (1.0 - (age * 5.0).min(1.0)) * 20.0;
            let r = Rect::from_min_size(pos2(screen.max.x - w - 18.0 + slide, y - 40.0), vec2(w, 40.0));
            painter.rect_filled(r.translate(vec2(0.0, 4.0)).expand(2.0), 12.0, with_alpha(Color32::BLACK, (a * 50.0) as u8));
            painter.rect_filled(r, 10.0, with_alpha(pal.surface, (a * 250.0) as u8));
            painter.rect_stroke(r, 10.0, Stroke::new(1.0, with_alpha(mix(pal.border, t.color, 0.4), (a * 255.0) as u8)), egui::StrokeKind::Inside);
            painter.rect_filled(Rect::from_min_size(r.min + vec2(0.0, 10.0), vec2(3.0, 20.0)), 2.0, with_alpha(t.color, (a * 255.0) as u8));
            painter.text(pos2(r.min.x + 22.0, r.center().y), Align2::CENTER_CENTER, t.icon, widgets::ui_font(14.0), with_alpha(t.color, (a * 255.0) as u8));
            painter.galley(pos2(r.min.x + 40.0, r.center().y - galley.size().y / 2.0), galley, pal.text);
            y -= 48.0;
        }
    }
}

impl App {
    /// Developer aid: `NEDIT_SHOT="/tmp/x:thesis@8,slides@12"` renders the given
    /// views and stores screenshots as `/tmp/x-<view>.png`, then quits.
    fn debug_shots(&mut self, ctx: &egui::Context, now: f64, spec: &str) {
        ctx.request_repaint();
        let Some((prefix, steps)) = spec.split_once(':') else { return };
        let steps: Vec<(&str, f64)> = steps.split(',').filter_map(|s| s.split_once('@').map(|(n, t)| (n, t.parse().unwrap_or(5.0)))).collect();
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None })
        });
        if let Some(img) = shot {
            let (name, _) = steps[self.shot_step];
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                let _ = buf.save(format!("{prefix}-{name}.png"));
            }
            self.shot_wait = false;
            self.shot_step += 1;
        }
        if self.shot_step >= steps.len() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let (name, at) = steps[self.shot_step];
        match name.split('-').next().unwrap_or("") {
            "thesis" => self.tab = Tab::Thesis,
            "slides" => self.tab = Tab::Slides,
            "shelf" => self.tab = Tab::Shelf,
            "present" => {
                if self.present.is_none() {
                    self.tab = Tab::Slides;
                    self.present = Some(PresentState { start: now - 312.0, black: false, hud: true, paused_at: None });
                }
            }
            _ => {}
        }
        if name.contains("outline") {
            self.side = SideMode::Outline;
        }
        if name.contains("papers") {
            self.side = SideMode::Papers;
        }
        if name.contains("git") {
            self.side = SideMode::Git;
            if name.contains("hist") && self.git.view.is_none() && !self.git.log.is_empty() {
                let root = self.project.root.clone();
                let h = self.git.log[0].hash.clone();
                self.git.open_commit(&root, &h);
            }
        }
        if name.contains("logs") {
            self.thesis.show_logs = true;
        }
        if name.contains("sel") && self.shelf_ui.selected.is_none() {
            self.shelf_ui.selected = self.shelf.papers.first().map(|p| p.entry.key.clone());
        }
        if name.contains("add") && self.shelf.papers.len() == 2 && !self.shelf_ui.busy {
            self.shelf_ui.busy = true;
            crate::shelf::run_query("arXiv:1706.03762".into(), self.shelf_tx.clone(), ctx.clone());
        }
        if name.contains("intro") && self.thesis.active.as_deref() == Some(self.project.config.thesis_main.as_str()) {
            let f = if self.project.root.join("content/introduction.tex").exists() { "content/introduction.tex" } else { "kapitel/einleitung.tex" };
            self.open_file(f, Tab::Thesis);
        }
        if name.contains("visual") {
            self.settings.visual = true;
        }
        if !name.contains("comp") {
            if let Some(b) = self.active_buffer_mut(Tab::Thesis) {
                b.completion = None;
            }
        }
        if name.contains("sync") && self.thesis.sync_after.is_none() && self.thesis.job.is_none() && self.thesis.viewer.doc.is_some() {
            if let Some(rel) = self.thesis.active.clone() {
                self.thesis.sync_after = Some((rel.clone(), 0));
                self.forward_sync(Tab::Thesis, &rel, 14, true, now);
            }
        }
        if name.contains("vim") && !self.settings.input_vim {
            self.settings.input_vim = true;
            if let Some(b) = self.active_buffer_mut(Tab::Thesis) {
                b.request_focus = true;
            }
        }
        // type keys into the focused editor once per step: name like "vimtype=jjwve"
        if let Some(keys) = name.split("type=").nth(1) {
            if self.debug_typed != self.shot_step + 1 && now > at - 2.0 {
                self.debug_typed = self.shot_step + 1;
                let keys = keys.replace('_', " ");
                ctx.input_mut(|i| i.events.push(egui::Event::Text(keys.clone())));
            }
        }
        if name.contains("newedit") && self.tree_ui.edit.is_none() {
            self.tree_ui.selected = Some("content".into());
            self.begin_new(false, None);
            if let Some(e) = &mut self.tree_ui.edit {
                e.name = "methoden".into();
            }
        }
        if name.contains("ftest") && !self.flat.iter().any(|f| f.starts_with("bilder/")) {
            let log = |m: String| eprintln!("FTEST {m}");
            self.tree_ui.selected = Some("content".into());
            self.begin_new(false, None);
            if let Some(e) = &mut self.tree_ui.edit {
                e.name = "neues-kapitel".into();
            }
            self.finish_edit_pub(Tab::Thesis, now);
            log(format!("created exists={} active={:?}", self.project.root.join("content/neues-kapitel.tex").exists(), self.thesis.active));
            log(format!("main includes={}", self.read_source("main.tex").unwrap_or_default().contains("{content/neues-kapitel}")));
            log(format!("rename: {:?}", self.rename_path("content/neues-kapitel.tex", "content/umbenannt.tex")));
            log(format!("main updated={}", self.read_source("main.tex").unwrap_or_default().contains("{content/umbenannt}")));
            log(format!("mkdir: {:?}", self.create_entry("", "bilder", true)));
            log(format!("move: {:?}", self.rename_path("content/umbenannt.tex", "bilder/umbenannt.tex")));
            log(format!("buffer moved={} tabs={:?}", self.buffers.iter().any(|b| b.rel == "bilder/umbenannt.tex"), self.thesis.tabs));
            log(format!("dup: {:?}", self.create_entry("bilder", "umbenannt", false)));
            log(format!("bad: {:?}", self.create_entry("", "../x", false)));
            self.save_all();
            log(format!("old dir file recreated={}", self.project.root.join("content/umbenannt.tex").exists()));
            self.tree_ui.selected = Some("bilder".into());
        }
        if name.contains("tabs") && self.thesis.tabs.len() < 4 {
            let fs: Vec<String> = self.flat.iter().filter(|f| f.ends_with(".tex")).take(8).cloned().collect();
            for f in fs {
                self.open_file(&f, Tab::Thesis);
            }
        }
        if name.contains("doc") && !self.doc_mode {
            self.doc_mode = true;
        }
        if name.contains("slider") {
            let id = egui::Id::new("debug-slider");
            egui::Area::new(id).fixed_pos(pos2(200.0, 200.0)).order(egui::Order::Foreground).show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Schriftgröße");
                        let mut v = self.settings.font_size;
                        widgets::fancy_slider(ui, &mut v, 10.0, 24.0, 0.5, "pt", &self.pal.clone());
                    });
                });
            });
        }
        if name.contains("revtest") && self.debug_typed != 777 {
            self.debug_typed = 777;
            let rel = "kapitel/fazit.tex".to_string();
            self.open_file(&rel, Tab::Thesis);
            let disk = std::fs::read_to_string(self.project.root.join(&rel)).unwrap_or_default();
            if let Some(i) = self.buffer_idx(&rel) {
                self.buffers[i].text.push_str("\nUNGESPEICHERTE ÄNDERUNG\n");
                eprintln!("REVTEST dirty before={}", self.buffers[i].dirty());
            }
            // also a saved change so git sees the file as modified
            let _ = std::fs::write(self.project.root.join(&rel), format!("{disk}\nGESPEICHERTE ÄNDERUNG\n"));
            self.git_revert(&[rel.clone()], now);
            if let Some(i) = self.buffer_idx(&rel) {
                let b = &self.buffers[i];
                eprintln!("REVTEST after: dirty={} has_unsaved={} has_saved={} equals_head={}", b.dirty(), b.text.contains("UNGESPEICHERTE"), b.text.contains("GESPEICHERTE ÄNDERUNG"), b.text == disk);
            }
        }
        if let Some(q) = name.split("quick=").nth(1) {
            if self.quick.is_none() {
                self.open_quick(now - 1.0);
                if let Some(qo) = &mut self.quick {
                    qo.query = q.replace('_', " ");
                }
            }
        } else if name.contains("quick") && self.quick.is_none() {
            self.open_quick(now - 1.0);
        }
        if name.contains("split") {
            self.settings.diff_split = true;
        }
        if name.contains("vblock") && self.debug_typed != 2000 + self.shot_step {
            self.debug_typed = 2000 + self.shot_step;
            ctx.input_mut(|i| {
                i.events.push(egui::Event::Paste(String::new()));
                i.events.push(egui::Event::Text("jjjjlll".into()));
            });
        }
        if name.contains("ctrltab") && self.debug_typed != 3000 + self.shot_step {
            self.debug_typed = 3000 + self.shot_step;
            ctx.input_mut(|i| i.events.push(egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }));
        }
        if name.contains("help") {
            self.help_open = true;
        }
        if name.contains("about") {
            self.about_open = true;
        }
        if name.contains("narrow") {
            self.settings.doc_width = 520.0;
        }
        if name.contains("wide") {
            self.settings.doc_width = 1400.0;
        }
        if name.contains("focus") && !self.focus {
            self.focus = true;
        }
        if name.contains("fpdf") && !self.focus_pdf {
            self.focus_pdf = true;
            self.pdf_win_gen += 1;
        }
        if name.contains("find") && !self.find.open {
            self.find.open = true;
            self.find.query = "hefe".into();
            if let Some(b) = self.active_buffer_mut(Tab::Thesis) {
                if let Some(m) = crate::editor::find_matches(&b.text, "Hefe").first().copied() {
                    b.select(m.0, m.1);
                }
            }
        }
        if name.contains("comp") {
            let cites = self.cites.clone();
            if let Some(b) = self.active_buffer_mut(Tab::Thesis) {
                if b.completion.is_none() {
                    let pos = b.text.find("\\citep{").map(|p| crate::editor::byte_to_char(&b.text, p + 7)).unwrap_or(0);
                    b.completion = Some(crate::editor::Completion {
                        kind: crate::editor::CompKind::Cite,
                        start: pos,
                        items: cites,
                        selected: 0,
                    });
                    b.select(pos, pos);
                }
            }
        }
        if now >= at && !self.shot_wait {
            self.shot_wait = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }
}

pub fn fmt_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out
}
