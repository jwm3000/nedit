//! The two document workspaces: "Masterarbeit" (Overleaf-style editor + PDF)
//! and "Präsentation" (filmstrip, editor, stage, fullscreen presenting).

use crate::app::{truncate, App, PresentState, SideMode, Tab};
use crate::compile::Level;
use crate::editor::{self, CompletionSources, EditorStyle};
use crate::icons as ic;
use crate::pdfview::{self, PdfViewer, Zoom};
use crate::theme::{mix, with_alpha, Palette};
use crate::widgets::{self, BtnKind};
use egui::{pos2, vec2, Align2, Color32, Frame, Margin, Rect, Sense, Stroke, StrokeKind, Ui};
use std::collections::HashMap;

// ───────────────────────────── layouts ─────────────────────────────

pub fn thesis_ui(app: &mut App, ui: &mut Ui, now: f64) {
    let pal = app.pal.clone();
    egui::Panel::left("rail")
        .exact_size(52.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().fill(pal.mantle).inner_margin(Margin::symmetric(11, 12)))
        .show(ui, |ui| rail(app, ui, &pal));
    egui::Panel::left("sidebar")
        .resizable(true)
        .default_size(260.0)
        .size_range(190.0..=480.0)
        .frame(Frame::new().fill(pal.mantle).inner_margin(Margin { left: 6, right: 10, top: 12, bottom: 8 }))
        .show(ui, |ui| sidebar(app, ui, &pal, Tab::Thesis, now));
    let w = ui.available_width();
    let pw = (w * app.settings.pdf_frac).clamp(280.0, (w - 340.0).max(280.0));
    let panel = egui::Panel::right("pdf-thesis")
        .resizable(false)
        .exact_size(pw)
        .show_separator_line(false)
        .frame(Frame::new().fill(pal.crust))
        .show(ui, |ui| pdf_panel(app, ui, &pal, Tab::Thesis, now));
    egui::CentralPanel::default().frame(Frame::new().fill(pal.base)).show(ui, |ui| editor_area(app, ui, &pal, Tab::Thesis, now));
    if let Some(f) = splitter(ui, panel.response.rect, w, &pal, "split-thesis") {
        app.settings.pdf_frac = (app.settings.pdf_frac + f).clamp(0.2, 0.8);
        if f == 0.0 {
            app.settings.save();
        }
    }
}

/// A draggable vertical divider on the left edge of `panel`. Returns the change of the
/// right panel's width fraction.
fn splitter(ui: &mut Ui, panel: Rect, total_w: f32, pal: &Palette, id: &str) -> Option<f32> {
    let r = Rect::from_min_max(pos2(panel.min.x - 4.0, panel.min.y), pos2(panel.min.x + 4.0, panel.max.y));
    let resp = ui.interact(r, egui::Id::new(id), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    let active = resp.hovered() || resp.dragged();
    let col = if active { pal.accent } else { with_alpha(pal.border, 160) };
    ui.painter().line_segment([pos2(panel.min.x, panel.min.y), pos2(panel.min.x, panel.max.y)], Stroke::new(if active { 2.0 } else { 1.0 }, col));
    if resp.dragged() {
        let dx = resp.drag_delta().x;
        if dx != 0.0 {
            return Some(-dx / total_w.max(1.0));
        }
    }
    if resp.drag_stopped() {
        return Some(0.0);
    }
    None
}

pub fn slides_ui(app: &mut App, ui: &mut Ui, now: f64) {
    let pal = app.pal.clone();
    let ctx = ui.ctx().clone();
    if !app.project.has_slides() {
        egui::CentralPanel::default().frame(Frame::new().fill(pal.base)).show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new(format!("Keine Präsentation gefunden: {}", app.project.config.slides_main)).color(pal.dim));
            });
        });
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
        start_presentation(app, &ctx, now);
    }
    egui::Panel::left("filmstrip")
        .resizable(true)
        .default_size(230.0)
        .size_range(170.0..=360.0)
        .frame(Frame::new().fill(pal.mantle).inner_margin(Margin { left: 10, right: 10, top: 12, bottom: 8 }))
        .show(ui, |ui| filmstrip(app, ui, &pal, now));
    let w = ui.available_width();
    let sw = (w * app.settings.stage_frac).clamp(300.0, (w - 340.0).max(300.0));
    let panel = egui::Panel::right("stage")
        .resizable(false)
        .exact_size(sw)
        .show_separator_line(false)
        .frame(Frame::new().fill(pal.crust))
        .show(ui, |ui| stage(app, ui, &pal, now));
    egui::CentralPanel::default().frame(Frame::new().fill(pal.base)).show(ui, |ui| editor_area(app, ui, &pal, Tab::Slides, now));
    if let Some(f) = splitter(ui, panel.response.rect, w, &pal, "split-stage") {
        app.settings.stage_frac = (app.settings.stage_frac + f).clamp(0.2, 0.8);
        if f == 0.0 {
            app.settings.save();
        }
    }

    // editor cursor → show the slide under the cursor
    if let Some(rel) = app.slides.active.clone() {
        if let Some(i) = app.buffer_idx(&rel) {
            let b = &app.buffers[i];
            if b.cursor_moved {
                app.slides.sync_line = Some((rel.clone(), b.line, now));
            }
        }
    }
    if let Some((rel, line, t0)) = app.slides.sync_line.clone() {
        if now - t0 > 0.25 {
            app.slides.sync_line = None;
            app.forward_sync(Tab::Slides, &rel, line, false, now);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(260));
        }
    }
}

// ───────────────────────────── sidebar ─────────────────────────────

fn rail(app: &mut App, ui: &mut Ui, pal: &Palette) {
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        for (mode, icon, tip) in [
            (SideMode::Files, ic::FOLDER, "Dateien"),
            (SideMode::Outline, ic::LIST, "Gliederung"),
            (SideMode::Papers, ic::BOOKMARK, "Literatur zitieren"),
            (SideMode::Git, ic::GIT, "Git – Versionen & Verlauf"),
        ] {
            let active = app.side == mode;
            let r = widgets::icon_button(ui, icon, tip, pal, active);
            if active {
                ui.painter().rect_filled(Rect::from_min_size(pos2(r.rect.min.x - 11.0, r.rect.min.y + 7.0), vec2(3.0, 16.0)), 2.0, pal.accent);
            }
            if r.clicked() {
                app.side = mode;
            }
        }
    });
}

fn sidebar(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab, now: f64) {
    match app.side {
        SideMode::Files => crate::filetree::files_view(app, ui, pal, t, now),
        SideMode::Outline => outline_view(app, ui, pal, t),
        SideMode::Papers => papers_quick(app, ui, pal, t, now),
        SideMode::Git => git_panel(app, ui, pal, t, now),
    }
}

fn header_row(ui: &mut Ui, title: &str, pal: &Palette, add_right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(title.to_uppercase()).font(widgets::ui_font(10.5)).color(pal.dim).extra_letter_spacing(1.2));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_right);
    });
    ui.add_space(6.0);
}

fn outline_view(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab) {
    header_row(ui, "Gliederung", pal, |_| {});
    let items = if t == Tab::Slides { app.slide_outline.clone() } else { app.outline.clone() };
    // current position
    let (cur_file, cur_line) = app
        .ws(t)
        .active
        .clone()
        .and_then(|r| app.buffer_idx(&r).map(|i| (r, app.buffers[i].line)))
        .unwrap_or_default();
    let current = items.iter().rposition(|it| it.file == cur_file && it.line <= cur_line);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        if items.is_empty() {
            ui.label(egui::RichText::new("Noch keine Kapitel.").color(pal.dim));
        }
        for (k, it) in items.iter().enumerate() {
            let h = if it.level <= 1 { 30.0 } else { 25.0 };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
            let is_cur = current == Some(k);
            if is_cur {
                ui.painter().rect_filled(rect, 6.0, with_alpha(pal.accent, 26));
                ui.painter().rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(2.5, h - 12.0)), 1.0, pal.accent);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, with_alpha(pal.text, 10));
            }
            let indent = it.level.saturating_sub(1) as f32 * 14.0;
            let (font, col) = match it.level {
                0 | 1 => (widgets::bold_font(13.0), pal.bright),
                2 => (widgets::ui_font(13.0), pal.text),
                _ => (widgets::ui_font(12.5), pal.subtext),
            };
            if it.level >= 2 {
                ui.painter().circle_filled(pos2(rect.min.x + 12.0 + indent, rect.center().y), 2.0, if is_cur { pal.accent } else { pal.dim });
            }
            let tx = rect.min.x + 22.0 + indent;
            let maxw = rect.max.x - tx - 4.0;
            let txt = truncate(&it.title, (maxw / 7.0).max(8.0) as usize);
            ui.painter().text(pos2(tx, rect.center().y), Align2::LEFT_CENTER, txt, font, col);
            if resp.clicked() {
                let (f, l) = (it.file.clone(), it.line);
                app.jump_to(&f, l, t);
            }
        }
    });
}

fn papers_quick(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab, now: f64) {
    header_row(ui, "Literatur", pal, |ui| {
        if widgets::icon_button_sized(ui, ic::EXTERNAL, "Bibliothek öffnen", pal, false, 24.0).clicked() {
            app.tab = Tab::Shelf;
        }
    });
    let w = ui.available_width();
    let mut f = std::mem::take(&mut app.shelf_ui.side_filter);
    widgets::search_field(ui, &mut f, "Filtern …", ic::SEARCH, pal, w, "side-filter");
    app.shelf_ui.side_filter = f;
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Klick fügt \\citep{…} an der Cursorposition ein.").font(widgets::ui_font(11.0)).color(pal.dim));
    ui.add_space(4.0);
    let q = app.shelf_ui.side_filter.to_lowercase();
    let papers: Vec<(String, String, String, Color32)> = app
        .shelf
        .papers
        .iter()
        .filter(|p| q.is_empty() || format!("{} {} {}", p.entry.key, p.entry.title(), p.entry.authors_full()).to_lowercase().contains(&q))
        .map(|p| (p.entry.key.clone(), format!("{} {}", p.entry.authors_short(), p.entry.year()), p.entry.title(), widgets::color_for(&p.entry.key, pal)))
        .collect();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (key, head, title, col) in papers {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, 8.0, with_alpha(pal.text, 12));
            }
            ui.painter().rect_filled(Rect::from_min_size(rect.min + vec2(4.0, 9.0), vec2(3.0, 32.0)), 2.0, col);
            ui.painter().text(pos2(rect.min.x + 16.0, rect.min.y + 16.0), Align2::LEFT_CENTER, &head, widgets::bold_font(12.5), pal.bright);
            let maxc = ((rect.width() - 24.0) / 6.4) as usize;
            ui.painter().text(pos2(rect.min.x + 16.0, rect.min.y + 34.0), Align2::LEFT_CENTER, truncate(&title, maxc.max(10)), widgets::ui_font(11.5), pal.subtext);
            let resp = resp.on_hover_text(format!("{title}\n\n\\citep{{{key}}}"));
            if resp.clicked() {
                if let Some(rel) = app.insert_snippet(t, &format!("\\citep{{{key}}}$0")) {
                    let g = app.pal.green;
                    app.toast(ic::QUOTE, format!("\\citep{{{key}}} → {rel}"), g, now);
                }
            }
        }
    });
}

// ───────────────────────────── editor area ─────────────────────────────

const THESIS_SNIPPETS: &[(&str, &str, &str)] = &[
    (ic::HEADER, "Abschnitt", "\\section{$0}\n"),
    (ic::BOLD, "Fett (Strg+B)", "\\textbf{$SEL$0}"),
    (ic::ITALIC, "Kursiv (Strg+I)", "\\textit{$SEL$0}"),
    (ic::LIST_UL, "Aufzählung", "\\begin{itemize}\n  \\item $0\n\\end{itemize}\n"),
    (ic::LIST_OL, "Nummerierung", "\\begin{enumerate}\n  \\item $0\n\\end{enumerate}\n"),
    (ic::IMAGE, "Abbildung", "\\begin{figure}[htbp]\n  \\centering\n  \\includegraphics[width=0.8\\linewidth]{$0}\n  \\caption{}\n  \\label{fig:}\n\\end{figure}\n"),
    (ic::TABLE, "Tabelle", "\\begin{table}[htbp]\n  \\centering\n  \\caption{$0}\n  \\label{tab:}\n  \\begin{tabular}{lrr}\n    \\toprule\n    A & B & C \\\\\n    \\midrule\n    1 & 2 & 3 \\\\\n    \\bottomrule\n  \\end{tabular}\n\\end{table}\n"),
    ("∑", "Gleichung", "\\begin{equation}\n  $0\n  \\label{eq:}\n\\end{equation}\n"),
    (ic::FILE_CODE, "Fußnote", "\\footnote{$0}"),
];

const SLIDE_SNIPPETS: &[(&str, &str, &str)] = &[
    (ic::SQUARE, "Neue Folie", "\\begin{frame}{$0Titel}\n  \\begin{itemize}\n    \\item \n  \\end{itemize}\n\\end{frame}\n\n"),
    (ic::COLUMNS, "Zwei Spalten", "\\begin{frame}{$0Titel}\n  \\begin{columns}[T]\n    \\begin{column}{0.48\\textwidth}\n      Links\n    \\end{column}\n    \\begin{column}{0.48\\textwidth}\n      Rechts\n    \\end{column}\n  \\end{columns}\n\\end{frame}\n\n"),
    (ic::IMAGE, "Bildfolie", "\\begin{frame}{$0Titel}\n  \\centering\n  \\includegraphics[width=0.8\\textwidth,height=0.7\\textheight,keepaspectratio]{}\n\\end{frame}\n\n"),
    (ic::STEP, "Schrittweise", "\\begin{itemize}\n  \\item<1-> $0\n  \\item<2-> \n  \\item<3-> \n\\end{itemize}\n"),
    (ic::STICKY, "Block", "\\begin{block}{$0Titel}\n  \n\\end{block}\n"),
    ("∑", "Formel", "\\[\n  $0\n\\]\n"),
    (ic::TABLE, "Tabelle", "\\begin{tabular}{lrr}\n  \\toprule\n  $0A & B & C \\\\\n  \\midrule\n  1 & 2 & 3 \\\\\n  \\bottomrule\n\\end{tabular}\n"),
    (ic::HEADER, "Abschnitt", "\\section{$0}\n\n"),
    (ic::BOLD, "Hervorheben", "\\alert{$SEL$0}"),
];

fn editor_area(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab, now: f64) {
    // ── tab strip ──
    let tabs = app.ws(t).tabs.clone();
    let active = app.ws(t).active.clone();
    let strip = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover()).0;
    ui.painter().rect_filled(strip, 0.0, pal.mantle);
    ui.painter().line_segment([strip.left_bottom(), strip.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 150)));
    let mut x = strip.min.x + 6.0;
    let mut close: Option<String> = None;
    let mut activate: Option<String> = None;
    // tabs shrink (and names get truncated) when they don't fit
    let natural: Vec<f32> = tabs
        .iter()
        .map(|rel| ui.painter().layout_no_wrap(rel.rsplit('/').next().unwrap_or(rel).to_string(), widgets::ui_font(13.0), pal.text).size().x + 58.0)
        .collect();
    let avail = strip.width() - 12.0;
    let total: f32 = natural.iter().sum::<f32>() + 2.0 * tabs.len() as f32;
    let cap = if total > avail && !tabs.is_empty() { (avail / tabs.len() as f32 - 2.0).max(64.0) } else { f32::INFINITY };
    for (ti, rel) in tabs.iter().enumerate() {
        let full = rel.rsplit('/').next().unwrap_or(rel).to_string();
        let w = natural[ti].min(cap);
        let name = if w < natural[ti] { truncate(&full, (((w - 58.0) / 7.2).max(2.0)) as usize) } else { full.clone() };
        let g = ui.painter().layout_no_wrap(name.clone(), widgets::ui_font(13.0), pal.text);
        let r = Rect::from_min_size(pos2(x, strip.min.y + 6.0), vec2(w, 34.0));
        let resp = ui.interact(r, ui.id().with(("tab", t as u8, rel)), Sense::click());
        let is_active = active.as_deref() == Some(rel.as_str());
        let p = ui.painter();
        if is_active {
            p.rect_filled(r, egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, pal.base);
            p.rect_filled(Rect::from_min_size(r.min + vec2(10.0, 0.0), vec2(w - 20.0, 2.0)), 1.0, pal.accent);
        } else if resp.hovered() {
            p.rect_filled(r.shrink2(vec2(0.0, 3.0)), 7.0, with_alpha(pal.text, 10));
        }
        let (icon, col) = crate::filetree::file_icon(&full, pal);
        p.text(pos2(r.min.x + 16.0, r.center().y), Align2::CENTER_CENTER, icon, widgets::ui_font(11.5), if is_active { col } else { pal.dim });
        let gs = app.git_letter(rel);
        let name_col = match gs {
            Some(c) => mix(widgets::git_color(c, pal), if is_active { pal.bright } else { pal.subtext }, 0.35),
            None if is_active => pal.bright,
            None => pal.subtext,
        };
        p.galley(pos2(r.min.x + 28.0, r.center().y - g.size().y / 2.0), g, name_col);
        let cr = Rect::from_center_size(pos2(r.max.x - 15.0, r.center().y), vec2(18.0, 18.0));
        let cresp = ui.interact(cr, ui.id().with(("tabclose", t as u8, rel)), Sense::click());
        let dirty = app.buffer_idx(rel).is_some_and(|i| app.buffers[i].dirty());
        if cresp.hovered() || (resp.hovered() && !dirty) {
            if cresp.hovered() {
                ui.painter().rect_filled(cr, 4.0, with_alpha(pal.text, 20));
            }
            ui.painter().text(cr.center(), Align2::CENTER_CENTER, ic::TIMES, widgets::ui_font(10.0), pal.subtext);
        } else if dirty {
            ui.painter().circle_filled(cr.center(), 3.5, pal.yellow);
        }
        let resp = resp.on_hover_text(rel.as_str());
        if cresp.clicked() || resp.middle_clicked() {
            close = Some(rel.clone());
        } else if resp.clicked() {
            activate = Some(rel.clone());
        }
        x += w + 2.0;
    }
    if let Some(r) = close {
        app.close_tab(&r, t);
    }
    if let Some(r) = activate {
        app.open_file(&r, t);
    }
    if t == Tab::Thesis && app.git.view.is_some() {
        git_view(app, ui, pal, t);
        return;
    }

    editor_toolbar(app, ui, pal, t);

    // ── find bar ──
    if app.find.open {
        find_bar(app, ui, pal, t);
    }

    editor_body(app, ui, pal, t, now);
}

fn editor_body(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab, now: f64) {
    let Some(rel) = app.ws(t).active.clone() else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("Wähle links eine Datei aus.").color(pal.dim));
        });
        return;
    };
    let Some(bi) = app.buffer_idx(&rel) else { return };
    let mut issues: HashMap<usize, (Level, String)> = HashMap::new();
    for i in &app.ws(t).issues {
        if i.file.as_deref().map(|f| f.trim_start_matches("./")) == Some(rel.as_str()) {
            if let Some(l) = i.line {
                issues.entry(l).or_insert((i.level, i.message.clone()));
            }
        }
    }
    let visual = t == Tab::Thesis && app.settings.visual;
    let root = app.project.root.clone();
    let marks = app.git.line_marks(&root, &rel, app.buffers[bi].disk_stamp);
    let style = EditorStyle { pal: &app.pal, syntax: &app.syntax, font_size: app.settings.font_size, style_rev: app.style_rev, issues: &issues, visual, embedded: false, git_marks: &marks, search: app.find.open.then_some(app.find.query.as_str()) };
    let src = CompletionSources { cites: &app.cites, labels: &app.labels, files: &app.flat };
    let use_vim = app.settings.input_vim && !visual;
    let out = editor::editor_ui(ui, &mut app.buffers[bi], &style, &src, if use_vim { Some(&mut app.vim) } else { None });
    if out.changed {
        app.buffers[bi].last_edit = now;
    }
    if let Some(line) = out.ctrl_click_line {
        app.forward_sync(t, &rel, line, true, now);
    }
    if let Some(vo) = out.vim {
        apply_vim(app, ui.ctx(), vo, t, &rel, now);
    }
}

/// Act on vim ex commands / yanks.
fn apply_vim(app: &mut App, ctx: &egui::Context, vo: crate::vim::VimOut, t: Tab, rel: &str, now: f64) {
    if let Some(y) = vo.yanked {
        ctx.copy_text(y);
    }
    if let Some(q) = vo.search {
        app.find.query = q;
        app.find.open = true;
    }
    if vo.noh {
        app.find.open = false;
    }
    if vo.save {
        if let Some(i) = app.buffer_idx(rel) {
            let line = app.buffers[i].line;
            app.ws_mut(t).sync_after = Some((rel.to_string(), line));
        }
        app.save_all();
        app.compile(t, ctx);
        let g = app.pal.green;
        app.toast(ic::SAVE, format!("{rel} gespeichert"), g, now);
    }
    if vo.close {
        app.close_tab(rel, t);
    }
}


/// Code / Visuell / Dokument switch (right-to-left layout expected).
fn view_switch(app: &mut App, ui: &mut Ui, pal: &Palette, ctx: &egui::Context, compact: bool) {
    view_switch_ordered(app, ui, pal, ctx, compact, false)
}

/// `ltr`: the layout of `ui` is left-to-right (otherwise right-to-left).
fn view_switch_ordered(app: &mut App, ui: &mut Ui, pal: &Palette, ctx: &egui::Context, compact: bool, ltr: bool) {
    ui.spacing_mut().item_spacing.x = 4.0;
    let l = |s: &'static str| if compact { "" } else { s };
    let vis = app.settings.visual;
    let dm = app.doc_mode;
    let mut items: Vec<u8> = vec![0, 1, 2, 3]; // code, visual, separator, document
    if !ltr {
        items.reverse();
    }
    for it in items {
        match it {
            0 => {
                if widgets::chip(ui, ic::CODE, l("Code"), !vis, pal.accent, pal).on_hover_text("Code: LaTeX-Quelltext (Strg+E)").clicked() && vis {
                    app.settings.visual = false;
                    app.settings.save();
                }
            }
            1 => {
                if widgets::chip(ui, ic::EYE, l("Visuell"), vis, pal.accent, pal).on_hover_text("Visuell: LaTeX-Befehle ausblenden (Strg+E)").clicked() && !vis {
                    app.settings.visual = true;
                    app.settings.save();
                }
            }
            2 => {
                let r = ui.allocate_exact_size(vec2(6.0, 18.0), Sense::hover()).0;
                ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, pal.border));
            }
            _ => {
                if widgets::chip(ui, ic::BOOK, l("Dokument"), dm, pal.accent, pal).on_hover_text("Dokument: alle Kapitel zusammenhängend (Strg+Umschalt+D)").clicked() {
                    app.set_doc_mode(!dm, ctx);
                }
            }
        }
    }
}

fn editor_toolbar(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab) {
    // ── snippet toolbar ──
    let bar = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover()).0;
    ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 90)));
    let inner = bar.shrink2(vec2(10.0, 4.0));
    let ctx = ui.ctx().clone();
    // right side first, so the view controls are always visible
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 2.0;
    if t == Tab::Thesis {
        let f = app.focus;
        if widgets::icon_button_sized(&mut right, ic::EXPAND, "Vollbild – nur der Text (F11)", pal, f, 28.0).clicked() {
            app.set_focus(!f, &ctx);
        }
        if app.doc_mode {
            if widgets::icon_button_sized(&mut right, ic::FILE_PDF, "PDF-Vorschau ein/aus", pal, app.doc_pdf, 28.0).clicked() {
                app.doc_pdf = !app.doc_pdf;
            }
            if widgets::icon_button_sized(&mut right, ic::LIST, "Inhaltsverzeichnis ein/aus", pal, app.doc_toc, 28.0).clicked() {
                app.doc_toc = !app.doc_toc;
            }
        }
    }
    if widgets::icon_button_sized(&mut right, ic::SEARCH, "Suchen & Ersetzen (Strg+F)", pal, app.find.open, 28.0).clicked() {
        app.find.open = !app.find.open;
        app.find.focus = app.find.open;
    }
    if t == Tab::Thesis {
        let r = right.allocate_exact_size(vec2(10.0, 20.0), Sense::hover()).0;
        right.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, pal.border));
        view_switch(app, &mut right, pal, &ctx, inner.width() < 640.0);
    }
    let left_edge = right.min_rect().min.x - 8.0;
    let left_rect = Rect::from_min_max(inner.min, pos2(left_edge.max(inner.min.x), inner.max.y));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(left_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.set_clip_rect(left_rect.expand2(vec2(0.0, 4.0)).intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing.x = 2.0;
    let snippets = if t == Tab::Slides { SLIDE_SNIPPETS } else { THESIS_SNIPPETS };
    for (k, (icon, tip, snip)) in snippets.iter().enumerate() {
        if t == Tab::Thesis && (k == 1 || k == 5) || t == Tab::Slides && (k == 3 || k == 7) {
            let r = child.allocate_exact_size(vec2(10.0, 20.0), Sense::hover()).0;
            child.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, pal.border));
        }
        if widgets::icon_button_sized(&mut child, icon, tip, pal, false, 28.0).clicked() {
            app.insert_snippet(t, snip);
        }
    }
    // cite picker
    let r = child.allocate_exact_size(vec2(10.0, 20.0), Sense::hover()).0;
    child.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, pal.border));
    let resp = widgets::icon_button_sized(&mut child, ic::QUOTE, "Zitieren", pal, false, 28.0);
    let mut insert: Option<String> = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(320.0);
        widgets::section_label(ui, "Zitieren aus der Bibliothek", pal);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for p in &app.shelf.papers {
                let label = format!("{} {}  ·  {}", p.entry.authors_short(), p.entry.year(), truncate(&p.entry.title(), 40));
                if ui.button(label).on_hover_text(&p.entry.key).clicked() {
                    insert = Some(format!("\\citep{{{}}}$0", p.entry.key));
                }
            }
            if app.shelf.papers.is_empty() {
                ui.label(egui::RichText::new("Die Bibliothek ist leer.").color(pal.dim));
            }
        });
    });
    if let Some(s) = insert {
        app.insert_snippet(t, &s);
    }
    let resp = widgets::icon_button_sized(&mut child, ic::LINK, "Querverweis", pal, false, 28.0);
    let mut insert: Option<String> = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(260.0);
        widgets::section_label(ui, "Querverweis (\\cref)", pal);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for l in &app.labels {
                if ui.button(egui::RichText::new(l).font(widgets::mono_font(12.5))).clicked() {
                    insert = Some(format!("\\cref{{{l}}}$0"));
                }
            }
        });
    });
    if let Some(s) = insert {
        app.insert_snippet(t, &s);
    }

}

fn find_bar(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab) {
    let bar = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover()).0;
    ui.painter().rect_filled(bar, 0.0, pal.mantle);
    ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 120)));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(12.0, 6.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let enter = child.input(|i| i.key_pressed(egui::Key::Enter));
    let shift = child.input(|i| i.modifiers.shift);
    let esc = child.input(|i| i.key_pressed(egui::Key::Escape));
    let mut q = std::mem::take(&mut app.find.query);
    let r = widgets::search_field(&mut child, &mut q, "Suchen", ic::SEARCH, pal, 220.0, "find-q");
    if app.find.focus {
        r.request_focus();
        app.find.focus = false;
    }
    let q_focused = r.has_focus();
    let q_changed = r.changed();
    let q_lost_enter = r.lost_focus() && enter;
    app.find.query = q;
    let mut rep = std::mem::take(&mut app.find.replace);
    widgets::search_field(&mut child, &mut rep, "Ersetzen", ic::PENCIL, pal, 180.0, "find-r");
    app.find.replace = rep;

    let query = app.find.query.clone();
    let replace = app.find.replace.clone();
    let Some(b) = app.active_buffer_mut(t) else { return };
    let matches = editor::find_matches(&b.text, &query);
    let cur = b.cursor.min(b.sel_end);
    let idx = matches.iter().position(|m| m.0 == cur && m.1 == b.cursor.max(b.sel_end));
    child.label(egui::RichText::new(if query.is_empty() { String::new() } else if matches.is_empty() { "Keine Treffer".into() } else { format!("{} / {}", idx.map(|i| i + 1).unwrap_or(0), matches.len()) }).font(widgets::ui_font(12.0)).color(pal.dim));
    let mut go: Option<bool> = None;
    if widgets::icon_button_sized(&mut child, "\u{f077}", "Vorheriger", pal, false, 26.0).clicked() {
        go = Some(false);
    }
    if widgets::icon_button_sized(&mut child, ic::CHEVRON_DOWN, "Nächster", pal, false, 26.0).clicked() {
        go = Some(true);
    }
    if q_changed && !matches.is_empty() {
        // jump to first match at/after cursor while typing
        let m = matches.iter().find(|m| m.0 >= cur).or(matches.first()).copied().unwrap();
        b.select(m.0, m.1);
        b.request_focus = false;
    }
    if q_lost_enter || (q_focused && enter) {
        go = Some(!shift);
    }
    if let Some(fwd) = go {
        if !matches.is_empty() {
            let m = if fwd {
                matches.iter().find(|m| m.0 > cur).or(matches.first()).copied().unwrap()
            } else {
                matches.iter().rev().find(|m| m.0 < cur).or(matches.last()).copied().unwrap()
            };
            b.select(m.0, m.1);
            b.request_focus = false;
        }
    }
    if widgets::button(&mut child, "", "Ersetzen", pal, BtnKind::Secondary).clicked() {
        if let Some(i) = idx {
            let (a, z) = matches[i];
            let ba = editor::char_to_byte(&b.text, a);
            let bz = editor::char_to_byte(&b.text, z);
            b.text.replace_range(ba..bz, &replace);
            b.last_edit = 0.0;
            let next = a + replace.chars().count();
            b.select(next, next);
        } else if let Some(m) = matches.iter().find(|m| m.0 >= cur).or(matches.first()) {
            b.select(m.0, m.1);
        }
    }
    if widgets::button(&mut child, "", "Alle", pal, BtnKind::Secondary).clicked() && !matches.is_empty() {
        for (a, z) in matches.iter().rev() {
            let ba = editor::char_to_byte(&b.text, *a);
            let bz = editor::char_to_byte(&b.text, *z);
            b.text.replace_range(ba..bz, &replace);
        }
        b.last_edit = 0.0;
    }
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::icon_button_sized(ui, ic::TIMES, "Schließen (Esc)", pal, false, 26.0).clicked() {
            app.find.open = false;
        }
    });
    if esc {
        app.find.open = false;
    }
}

// ───────────────────────────── PDF panel ─────────────────────────────

fn pdf_toolbar(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab) {
    let bar = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover()).0;
    ui.painter().rect_filled(bar, 0.0, pal.mantle);
    ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 150)));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(10.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    c.spacing_mut().item_spacing.x = 4.0;
    let narrow = bar.width() < 520.0;
    let ws = app.ws_mut(t);
    let (e, w) = (ws.errors(), ws.warnings());
    // logs toggle with badges
    let tip = if ws.show_logs { "Zurück zum PDF" } else { "Protokoll anzeigen" };
    let icon = if ws.show_logs { ic::FILE_PDF } else { ic::TERMINAL };
    if widgets::icon_button_sized(&mut c, icon, tip, pal, ws.show_logs, 28.0).clicked() {
        ws.show_logs = !ws.show_logs;
    }
    if e > 0 && widgets::badge(&mut c, &format!("{} {e}", ic::ERROR), pal.red).interact(Sense::click()).clicked() {
        ws.show_logs = true;
    }
    if w > 0 && widgets::badge(&mut c, &format!("{} {w}", ic::WARN), pal.yellow).interact(Sense::click()).clicked() {
        ws.show_logs = true;
    }
    let n = ws.viewer.page_count();
    c.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if widgets::icon_button_sized(ui, ic::EXTERNAL, "In externem Viewer öffnen", pal, false, 28.0).clicked() {
            if let Some(d) = &ws.viewer.doc {
                crate::platform::open_external(&d.path);
            }
        }
        if t == Tab::Thesis {
            if widgets::icon_button_sized(ui, ic::EXPAND, "Seite einpassen", pal, ws.viewer.zoom == Zoom::FitPage, 28.0).clicked() {
                ws.viewer.zoom = Zoom::FitPage;
            }
            if widgets::icon_button_sized(ui, ic::COLUMNS, "Breite einpassen", pal, ws.viewer.zoom == Zoom::FitWidth, 28.0).clicked() {
                ws.viewer.zoom = Zoom::FitWidth;
            }
            if widgets::icon_button_sized(ui, ic::ZOOM_IN, "Vergrößern", pal, false, 28.0).clicked() {
                ws.viewer.zoom_by(1.15);
            }
            if !narrow {
                let pct = format!("{:.0} %", ws.viewer.scale() * 100.0);
                ui.label(egui::RichText::new(pct).font(widgets::ui_font(12.0)).color(pal.subtext));
            }
            if widgets::icon_button_sized(ui, ic::ZOOM_OUT, "Verkleinern", pal, false, 28.0).clicked() {
                ws.viewer.zoom_by(1.0 / 1.15);
            }
            ui.add_space(8.0);
        }
        if n > 0 {
            ui.label(egui::RichText::new(format!("{} / {n}", ws.viewer.current_page + 1)).font(widgets::ui_font(12.5)).color(pal.text));
        }
    });
}

fn pdf_panel(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab, now: f64) {
    pdf_toolbar(app, ui, pal, t);
    if app.ws(t).show_logs {
        logs_view(app, ui, pal, t);
        return;
    }
    if app.ws(t).viewer.doc.is_none() {
        let r = ui.available_rect_before_wrap();
        if app.ws(t).job.is_some() {
            widgets::draw_spinner(ui, r.center() - vec2(0.0, 20.0), 14.0, pal.accent);
            ui.painter().text(r.center() + vec2(0.0, 16.0), Align2::CENTER_CENTER, "Erstes Kompilieren …", widgets::ui_font(13.0), pal.subtext);
        }
    }
    let renderer = app.renderer.clone();
    let resp = app.ws_mut(t).viewer.show(ui, &renderer, pal);
    if let Some((page, x, y)) = resp.double_click {
        app.inverse_sync(t, page, x, y);
    }
    // compiling indicator overlay
    if app.ws(t).job.is_some() && app.ws(t).viewer.doc.is_some() {
        let r = ui.max_rect();
        let pill = Rect::from_center_size(pos2(r.center().x, r.min.y + 62.0), vec2(150.0, 30.0));
        let p = ui.painter();
        p.rect_filled(pill.translate(vec2(0.0, 3.0)), 15.0, with_alpha(Color32::BLACK, 50));
        p.rect_filled(pill, 15.0, pal.surface);
        p.rect_stroke(pill, 15.0, Stroke::new(1.0, pal.border), StrokeKind::Inside);
        widgets::draw_spinner(ui, pos2(pill.min.x + 20.0, pill.center().y), 6.0, pal.accent);
        ui.painter().text(pos2(pill.min.x + 36.0, pill.center().y), Align2::LEFT_CENTER, "Kompiliert …", widgets::ui_font(12.5), pal.text);
    }
    let _ = now;
}

fn logs_view(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab) {
    let mut jump: Option<(String, usize)> = None;
    let ws = app.ws_mut(t);
    Frame::new().inner_margin(Margin::same(14)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Protokoll").font(widgets::display_font(19.0)).color(pal.bright));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::chip(ui, ic::TERMINAL, "Rohes Log", ws.show_raw, pal.accent, pal).clicked() {
                    ws.show_raw = !ws.show_raw;
                }
            });
        });
        ui.add_space(8.0);
        if ws.show_raw {
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.label(egui::RichText::new(&ws.raw_log).font(widgets::mono_font(11.5)).color(pal.subtext));
            });
            return;
        }
        if ws.issues.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(ic::CHECK_CIRCLE).font(widgets::ui_font(34.0)).color(pal.green));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(if ws.compiled_once { "Keine Fehler oder Warnungen" } else { "Noch nicht kompiliert" }).color(pal.subtext));
            });
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for is in &ws.issues {
                let (col, label) = match is.level {
                    Level::Error => (pal.red, "Fehler"),
                    Level::Warning => (pal.yellow, "Warnung"),
                    Level::BadBox => (pal.dim, "Satz"),
                };
                let resp = Frame::new()
                    .fill(mix(pal.surface, col, 0.06))
                    .stroke(Stroke::new(1.0, with_alpha(col, 70)))
                    .corner_radius(10)
                    .inner_margin(Margin { left: 16, right: 12, top: 10, bottom: 10 })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            widgets::badge(ui, label, col);
                            if let Some(f) = &is.file {
                                let loc = match is.line {
                                    Some(l) => format!("{f}:{l}"),
                                    None => f.clone(),
                                };
                                ui.label(egui::RichText::new(loc).font(widgets::mono_font(12.0)).color(pal.subtext));
                            }
                        });
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(&is.message).color(pal.text));
                        if !is.context.is_empty() {
                            ui.label(egui::RichText::new(&is.context).font(widgets::mono_font(11.5)).color(pal.dim));
                        }
                    })
                    .response;
                let r = resp.rect;
                ui.painter().rect_filled(Rect::from_min_size(r.min + vec2(0.0, 10.0), vec2(3.0, r.height() - 20.0)), 2.0, col);
                let resp = ui.interact(r, ui.id().with(("issue", &is.message, is.line)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    if let (Some(f), Some(l)) = (&is.file, is.line) {
                        jump = Some((f.clone(), l));
                    }
                }
                ui.add_space(6.0);
            }
        });
    });
    if let Some((f, l)) = jump {
        app.jump_to(&f, l, t);
    }
}

// ───────────────────────────── slides ─────────────────────────────

fn filmstrip(app: &mut App, ui: &mut Ui, pal: &Palette, now: f64) {
    header_row(ui, "Folien", pal, |ui| {
        let n = app.slides.viewer.page_count();
        ui.label(egui::RichText::new(n.to_string()).font(widgets::ui_font(11.0)).color(pal.dim));
    });
    let Some(doc) = app.slides.viewer.doc.clone() else {
        ui.label(egui::RichText::new("Wird kompiliert …").color(pal.dim));
        return;
    };
    let renderer = app.renderer.clone();
    app.thumbs.poll(ui.ctx());
    let cur = app.slides.viewer.current_page;
    let mut clicked: Option<usize> = None;
    let scroll_to_cur = app.slides.sync_line.is_none() && ui.ctx().animate_bool(ui.id().with("x"), true) > 0.0;
    let _ = scroll_to_cur;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let w = ui.available_width();
        for i in 0..doc.pages.len() {
            let p = doc.pages[i];
            let tw = w - 30.0;
            let th = tw * p[1] / p[0];
            let (rect, resp) = ui.allocate_exact_size(vec2(w, th + 14.0), Sense::click());
            let sel = i == cur;
            let thumb = Rect::from_min_size(pos2(rect.min.x + 26.0, rect.min.y + 4.0), vec2(tw - 2.0, th));
            ui.painter().text(pos2(rect.min.x + 10.0, thumb.min.y + 8.0), Align2::CENTER_CENTER, (i + 1).to_string(), widgets::ui_font(11.0), if sel { pal.accent } else { pal.dim });
            if ui.is_rect_visible(rect) {
                if sel {
                    ui.painter().rect_stroke(thumb.expand(3.0), 6.0, Stroke::new(2.0, pal.accent), StrokeKind::Outside);
                } else if resp.hovered() {
                    ui.painter().rect_stroke(thumb.expand(3.0), 6.0, Stroke::new(1.5, with_alpha(pal.text, 60)), StrokeKind::Outside);
                }
                pdfview::thumbnail(ui, &mut app.thumbs, &doc, i, thumb, &renderer);
            }
            if sel && app.slides.sync_line.is_some() {
                ui.scroll_to_rect(rect, None);
            }
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    if let Some(i) = clicked {
        app.slides.viewer.current_page = i;
        let p = doc.pages[i];
        app.inverse_sync(Tab::Slides, i, p[0] * 0.5, p[1] * 0.45);
        app.slides.sync_line = None;
    }
    let _ = now;
}

fn stage(app: &mut App, ui: &mut Ui, pal: &Palette, now: f64) {
    pdf_toolbar(app, ui, pal, Tab::Slides);
    if app.slides.show_logs {
        logs_view(app, ui, pal, Tab::Slides);
        return;
    }
    // bottom navigation
    let full = ui.available_rect_before_wrap();
    let nav_h = 64.0;
    let view_rect = Rect::from_min_max(full.min, pos2(full.max.x, full.max.y - nav_h));
    let nav_rect = Rect::from_min_max(pos2(full.min.x, full.max.y - nav_h), full.max);
    let renderer = app.renderer.clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(view_rect.shrink(10.0)));
    let resp = app.slides.viewer.show(&mut child, &renderer, pal);
    if let Some((page, x, y)) = resp.double_click {
        app.inverse_sync(Tab::Slides, page, x, y);
    }
    // keyboard nav when pointer is over the stage
    if ui.rect_contains_pointer(view_rect) {
        let (l, r) = ui.input(|i| (i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp), i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::PageDown)));
        if !ui.ctx().egui_wants_keyboard_input() {
            if l {
                app.slides.viewer.prev_page();
            }
            if r {
                app.slides.viewer.next_page();
            }
        }
    }
    let mut nav = ui.new_child(egui::UiBuilder::new().max_rect(nav_rect.shrink2(vec2(16.0, 12.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let n = app.slides.viewer.page_count();
    let cur = app.slides.viewer.current_page;
    if widgets::icon_button_sized(&mut nav, ic::ARROW_LEFT, "Vorherige Folie", pal, false, 34.0).clicked() {
        app.slides.viewer.prev_page();
    }
    // progress dots / bar
    let bar_w = (nav.available_width() - 260.0).max(60.0);
    let (br, _) = nav.allocate_exact_size(vec2(bar_w, 20.0), Sense::hover());
    let track = Rect::from_center_size(br.center(), vec2(bar_w - 16.0, 4.0));
    nav.painter().rect_filled(track, 2.0, with_alpha(pal.text, 25));
    if n > 1 {
        let f = cur as f32 / (n - 1) as f32;
        let fill = Rect::from_min_size(track.min, vec2(track.width() * f, 4.0));
        nav.painter().rect_filled(fill, 2.0, pal.accent);
        nav.painter().circle_filled(pos2(fill.max.x, track.center().y), 6.0, pal.accent);
    }
    if widgets::icon_button_sized(&mut nav, ic::ARROW_RIGHT, "Nächste Folie", pal, false, 34.0).clicked() {
        app.slides.viewer.next_page();
    }
    nav.add_space(8.0);
    nav.label(egui::RichText::new(format!("{} / {}", (cur + 1).min(n), n)).font(widgets::bold_font(13.0)).color(pal.text));
    nav.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let mins = app.project.config.talk_minutes;
        widgets::badge(ui, &format!("{} {mins} min", ic::CLOCK), pal.subtext);
    });
    let _ = now;
}

pub fn start_presentation(app: &mut App, ctx: &egui::Context, now: f64) {
    if app.slides.viewer.doc.is_none() {
        return;
    }
    app.present = Some(PresentState { start: now, black: false, hud: true, paused_at: None });
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
}

fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

pub fn present_ui(app: &mut App, ui: &mut Ui, now: f64) {
    let ctx = ui.ctx().clone();
    let pal = app.pal.clone();
    use egui::Key;
    let (next, prev, exit, black, hud, home, end, pause) = ctx.input(|i| {
        (
            i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::Space) || i.key_pressed(Key::PageDown) || i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::Enter),
            i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::PageUp) || i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::Backspace),
            i.key_pressed(Key::Escape) || i.key_pressed(Key::Q),
            i.key_pressed(Key::B) || i.key_pressed(Key::Period),
            i.key_pressed(Key::T),
            i.key_pressed(Key::Home),
            i.key_pressed(Key::End),
            i.key_pressed(Key::P),
        )
    });
    let n = app.slides.viewer.page_count();
    {
        let st = app.present.as_mut().unwrap();
        if black {
            st.black = !st.black;
        }
        if hud {
            st.hud = !st.hud;
        }
        if pause {
            match st.paused_at.take() {
                Some(p) => st.start += now - p,
                None => st.paused_at = Some(now),
            }
        }
    }
    if next {
        app.slides.viewer.current_page = (app.slides.viewer.current_page + 1).min(n.saturating_sub(1));
    }
    if prev {
        app.slides.viewer.current_page = app.slides.viewer.current_page.saturating_sub(1);
    }
    if home {
        app.slides.viewer.current_page = 0;
    }
    if end {
        app.slides.viewer.current_page = n.saturating_sub(1);
    }
    if exit {
        app.present = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        return;
    }
    let st = app.present.as_ref().unwrap();
    let (is_black, show_hud, start, paused_at) = (st.black, st.hud, st.start, st.paused_at);

    egui::CentralPanel::default().frame(Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
        let rect = ui.max_rect();
        let resp = ui.allocate_rect(rect, Sense::click());
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if p.x > rect.center().x {
                    app.slides.viewer.current_page = (app.slides.viewer.current_page + 1).min(n.saturating_sub(1));
                } else {
                    app.slides.viewer.current_page = app.slides.viewer.current_page.saturating_sub(1);
                }
            }
        }
        let Some(doc) = app.slides.viewer.doc.clone() else { return };
        app.slides.viewer.cache.poll(&ctx);
        let cur = app.slides.viewer.current_page.min(doc.pages.len().saturating_sub(1));
        if !is_black {
            let p = doc.pages[cur];
            let scale = (rect.width() / p[0]).min(rect.height() / p[1]);
            let pr = Rect::from_center_size(rect.center(), vec2(p[0] * scale, p[1] * scale));
            let dpi = PdfViewer::dpi_for(scale, ctx.pixels_per_point());
            let renderer = app.renderer.clone();
            if let Some(tex) = app.slides.viewer.cache.get(&doc, cur, dpi, &renderer, &ctx) {
                ui.painter().image(tex, pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                ui.painter().rect_filled(pr, 0.0, Color32::WHITE);
            }
            if cur + 1 < doc.pages.len() {
                app.slides.viewer.cache.get(&doc, cur + 1, dpi, &renderer, &ctx);
            }
        }
        // HUD: appears on mouse movement or when pinned (T)
        let moved = ctx.input(|i| i.pointer.velocity().length() > 1.0 || i.pointer.any_down());
        let last_move_id = egui::Id::new("present-last-move");
        if moved {
            ctx.data_mut(|d| d.insert_temp(last_move_id, now));
        }
        let last_move: f64 = ctx.data(|d| d.get_temp(last_move_id)).unwrap_or(now);
        let vis = if show_hud { 1.0 } else { (1.0 - ((now - last_move) as f32 - 1.5).max(0.0) * 2.0).clamp(0.0, 1.0) };
        if vis > 0.0 {
            let elapsed = paused_at.unwrap_or(now) - start;
            let target = app.project.config.talk_minutes as f64 * 60.0;
            let frac = (elapsed / target) as f32;
            let col = if frac > 1.0 { pal.red } else if frac > 0.8 { pal.yellow } else { pal.accent };
            let pill = Rect::from_center_size(pos2(rect.center().x, rect.max.y - 40.0), vec2(420.0, 44.0));
            let a = |c: Color32, f: f32| with_alpha(c, (f * vis * 255.0) as u8);
            let p = ui.painter();
            p.rect_filled(pill, 22.0, a(Color32::from_rgb(18, 18, 22), 0.82));
            p.text(pos2(pill.min.x + 26.0, pill.center().y), Align2::LEFT_CENTER, format!("{} / {}", cur + 1, doc.pages.len()), widgets::bold_font(15.0), a(Color32::WHITE, 0.95));
            let tr = Rect::from_min_size(pos2(pill.min.x + 110.0, pill.center().y - 2.0), vec2(170.0, 4.0));
            p.rect_filled(tr, 2.0, a(Color32::WHITE, 0.15));
            p.rect_filled(Rect::from_min_size(tr.min, vec2(tr.width() * frac.min(1.0), 4.0)), 2.0, a(col, 1.0));
            let ttxt = if paused_at.is_some() { format!("{} {}", ic::STOP, fmt_time(elapsed)) } else { fmt_time(elapsed) };
            p.text(pos2(pill.max.x - 76.0, pill.center().y), Align2::CENTER_CENTER, ttxt, widgets::mono_font(15.0), a(col, 1.0));
            p.text(pos2(pill.max.x - 26.0, pill.center().y), Align2::CENTER_CENTER, format!("{:.0}′", (target / 60.0)), widgets::ui_font(12.0), a(Color32::WHITE, 0.5));
            if now - start < 6.0 || !show_hud {
                let help = "←/→ blättern  ·  B schwarz  ·  P Pause  ·  T Leiste fixieren  ·  Esc beenden";
                let hr = Rect::from_center_size(pos2(rect.center().x, pill.min.y - 22.0), vec2(470.0, 26.0));
                p.rect_filled(hr, 13.0, a(Color32::from_rgb(18, 18, 22), 0.7));
                p.text(hr.center(), Align2::CENTER_CENTER, help, widgets::ui_font(11.5), a(Color32::WHITE, 0.75));
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    });
}

// ───────────────────────────── document mode ─────────────────────────────

fn doc_files(app: &mut App) -> Vec<String> {
    let main = app.project.config.thesis_main.clone();
    let files = crate::project::document_files(&main, &|f| app.read_source(f));
    for f in &files {
        if app.buffer_idx(f).is_none() {
            if let Ok(b) = crate::editor::Buffer::open(&app.project.root, f) {
                app.buffers.push(b);
            }
        }
    }
    files
}

/// Distraction-free: only the text (single file or whole document), code or visual.
pub fn focus_ui(app: &mut App, ui: &mut Ui, now: f64) {
    let pal = app.pal.clone();
    let ctx = ui.ctx().clone();
    let files = if app.doc_mode { doc_files(app) } else { vec![] };
    egui::CentralPanel::default().frame(Frame::new().fill(if app.doc_mode { pal.crust } else { pal.base })).show(ui, |ui| {
        if app.find.open {
            find_bar(app, ui, &pal, Tab::Thesis);
        }
        if app.doc_mode {
            document_body(app, ui, &pal, &files, now);
        } else {
            editor_body(app, ui, &pal, Tab::Thesis, now);
        }
    });
    if app.focus_pdf {
        floating_pdf(app, &ctx, &pal, now);
    }
    // floating controls, shown while the mouse moves near the top
    let screen = ctx.content_rect();
    let moved = ctx.input(|i| i.pointer.velocity().length() > 1.0);
    let id = egui::Id::new("focus-last-move");
    if moved {
        ctx.data_mut(|d| d.insert_temp(id, now));
    }
    let last: f64 = ctx.data(|d| d.get_temp(id)).unwrap_or(now);
    let near = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| p.y < screen.min.y + 90.0 && p.x > screen.max.x - 460.0);
    let vis = if near { 1.0 } else { (1.0 - ((now - last) as f32 - 1.6).max(0.0) * 2.0).clamp(0.0, 1.0) };
    if vis > 0.01 {
        egui::Area::new(egui::Id::new("focus-pill"))
            .order(egui::Order::Foreground)
            .anchor(Align2::RIGHT_TOP, vec2(-18.0, 14.0))
            .show(&ctx, |ui| {
                ui.set_opacity(vis);
                Frame::new()
                    .fill(pal.surface)
                    .stroke(Stroke::new(1.0, pal.border))
                    .corner_radius(12)
                    .inner_margin(Margin::symmetric(8, 6))
                    .shadow(egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: with_alpha(Color32::BLACK, 70) })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            view_switch_ordered(app, ui, &pal, &ctx, false, true);
                            let r = ui.allocate_exact_size(vec2(8.0, 18.0), Sense::hover()).0;
                            ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, pal.border));
                            let on = app.focus_pdf;
                            if widgets::chip(ui, ic::FILE_PDF, "PDF", on, pal.accent, &pal).on_hover_text("Schwebende PDF-Vorschau ein/aus").clicked() {
                                app.focus_pdf = !on;
                                app.pdf_win_gen += 1;
                                if !on {
                                    reveal_cursor_in_pdf(app, now);
                                }
                            }
                            let r = ui.allocate_exact_size(vec2(8.0, 18.0), Sense::hover()).0;
                            ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, pal.border));
                            if widgets::icon_button_sized(ui, ic::COMPRESS, "Vollbild beenden (F11 / Esc)", &pal, false, 28.0).clicked() {
                                app.set_focus(false, &ctx);
                            }
                        });
                    });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}

/// Scroll the thesis PDF to the source position of the editor cursor.
fn reveal_cursor_in_pdf(app: &mut App, now: f64) {
    let spec = app.spec(Tab::Thesis);
    if app.thesis.viewer.doc.is_none() && spec.pdf_path().exists() {
        app.thesis.viewer.load(&spec.pdf_path());
    }
    if let Some(rel) = app.thesis.active.clone() {
        if let Some(i) = app.buffer_idx(&rel) {
            let line = app.buffers[i].line;
            app.forward_sync(Tab::Thesis, &rel, line, true, now);
        }
    }
}

/// Movable, resizable PDF preview window used in full-screen mode.
fn floating_pdf(app: &mut App, ctx: &egui::Context, pal: &Palette, now: f64) {
    let screen = ctx.content_rect();
    let mut open = true;
    let mut double: Option<(usize, f32, f32)> = None;
    let renderer = app.renderer.clone();
    let compiling = app.thesis.job.is_some();
    // place below the floating controls; reuse the last rect, clamped to the screen
    let w = (screen.width() * 0.34).clamp(360.0, 720.0);
    let mut want = app.pdf_win.unwrap_or(Rect::from_min_size(pos2(screen.max.x - w - 24.0, screen.min.y + 78.0), vec2(w, screen.height() - 110.0)));
    let top = screen.min.y + 78.0;
    let h = want.height().min(screen.max.y - top - 12.0).max(240.0);
    let ww = want.width().min(screen.width() - 40.0).max(260.0);
    let x = want.min.x.clamp(screen.min.x + 8.0, (screen.max.x - ww - 8.0).max(screen.min.x + 8.0));
    let y = want.min.y.clamp(top, (screen.max.y - h - 8.0).max(top));
    want = Rect::from_min_size(pos2(x, y), vec2(ww, h));
    let window = egui::Window::new(egui::RichText::new(format!("{}  Vorschau", ic::FILE_PDF)).font(widgets::ui_font(13.0)).color(pal.text))
        .id(egui::Id::new(("focus-pdf-window", app.pdf_win_gen)))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .movable(true)
        .default_size(want.size())
        .default_pos(want.min)
        .min_size(vec2(260.0, 240.0))
        .frame(
            Frame::new()
                .fill(pal.crust)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(12)
                .inner_margin(Margin::same(6))
                .shadow(egui::Shadow { offset: [0, 12], blur: 36, spread: 0, color: with_alpha(Color32::BLACK, 110) }),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let v = &mut app.thesis.viewer;
                let n = v.page_count();
                ui.label(egui::RichText::new(format!("{} / {n}", (v.current_page + 1).min(n))).font(widgets::ui_font(12.0)).color(pal.subtext));
                if compiling {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    widgets::draw_spinner(ui, r.center(), 5.0, pal.accent);
                    ui.label(egui::RichText::new("Kompiliert …").font(widgets::ui_font(11.5)).color(pal.dim));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if widgets::icon_button_sized(ui, ic::COLUMNS, "Breite einpassen", pal, v.zoom == Zoom::FitWidth, 24.0).clicked() {
                        v.zoom = Zoom::FitWidth;
                    }
                    if widgets::icon_button_sized(ui, ic::ZOOM_IN, "Vergrößern", pal, false, 24.0).clicked() {
                        v.zoom_by(1.15);
                    }
                    if widgets::icon_button_sized(ui, ic::ZOOM_OUT, "Verkleinern", pal, false, 24.0).clicked() {
                        v.zoom_by(1.0 / 1.15);
                    }
                    if widgets::icon_button_sized(ui, ic::BOLT, "Zur Cursorposition", pal, false, 24.0).clicked() {
                        double = Some((usize::MAX, 0.0, 0.0));
                    }
                });
            });
            let resp = app.thesis.viewer.show(ui, &renderer, pal);
            if let Some(d) = resp.double_click {
                double = Some(d);
            }
        });
    if let Some(r) = &window {
        app.pdf_win = Some(r.response.rect);
    }
    match double {
        Some((usize::MAX, _, _)) => reveal_cursor_in_pdf(app, now),
        Some((page, x, y)) => app.inverse_sync(Tab::Thesis, page, x, y),
        None => {}
    }
    if !open {
        app.focus_pdf = false;
    }
}

pub fn document_ui(app: &mut App, ui: &mut Ui, now: f64) {
    let pal = app.pal.clone();
    let files = doc_files(app);

    if app.doc_toc {
        egui::Panel::left("doc-toc")
            .resizable(false)
            .exact_size(250.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(pal.mantle).inner_margin(Margin { left: 10, right: 10, top: 16, bottom: 8 }))
            .show(ui, |ui| doc_toc(app, ui, &pal, &files));
    }
    if app.doc_pdf {
        let w = ui.available_width();
        let pw = (w * app.settings.pdf_frac).clamp(280.0, (w - 420.0).max(280.0));
        egui::Panel::right("doc-pdf")
            .resizable(false)
            .exact_size(pw)
            .show_separator_line(false)
            .frame(Frame::new().fill(pal.crust))
            .show(ui, |ui| pdf_panel(app, ui, &pal, Tab::Thesis, now));
    }
    egui::CentralPanel::default().frame(Frame::new().fill(pal.crust)).show(ui, |ui| {
        let bar = ui.allocate_exact_size(vec2(ui.available_width(), 0.0), Sense::hover()).0;
        let _ = bar;
        Frame::new().fill(pal.mantle).show(ui, |ui| {
            ui.set_width(ui.available_width());
            editor_toolbar(app, ui, &pal, Tab::Thesis);
        });
        if app.find.open {
            find_bar(app, ui, &pal, Tab::Thesis);
        }
        document_body(app, ui, &pal, &files, now);
    });
}

fn doc_toc(app: &mut App, ui: &mut Ui, pal: &Palette, files: &[String]) {
    ui.label(egui::RichText::new(app.project.config.name.clone()).font(widgets::display_font(20.0)).color(pal.bright));
    let words: usize = files.iter().filter_map(|f| app.buffer_idx(f)).map(|i| crate::project::word_count(&app.buffers[i].text)).sum();
    ui.label(egui::RichText::new(format!("{} Wörter  ·  {} Dateien", crate::app::fmt_thousands(words), files.len())).font(widgets::ui_font(11.5)).color(pal.dim));
    ui.add_space(14.0);
    widgets::section_label(ui, "Inhalt", pal);
    let items = app.outline.clone();
    let active = app.thesis.active.clone().unwrap_or_default();
    let cur_line = app.buffer_idx(&active).map(|i| app.buffers[i].line).unwrap_or(0);
    let current = items.iter().rposition(|it| it.file == active && it.line <= cur_line);
    let mut jump = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        for (k, it) in items.iter().enumerate() {
            let h = if it.level <= 1 { 30.0 } else { 24.0 };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
            let is_cur = current == Some(k);
            if is_cur {
                ui.painter().rect_filled(rect, 6.0, with_alpha(pal.accent, 26));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, with_alpha(pal.text, 10));
            }
            let indent = it.level.saturating_sub(1) as f32 * 13.0;
            let font = if it.level <= 1 { widgets::FontIdExt::serif_bold(15.0) } else { widgets::FontIdExt::serif(14.0) };
            let col = if it.level <= 1 { pal.bright } else if is_cur { pal.text } else { pal.subtext };
            let maxc = ((rect.width() - indent - 16.0) / 6.8).max(8.0) as usize;
            ui.painter().text(pos2(rect.min.x + 8.0 + indent, rect.center().y), Align2::LEFT_CENTER, truncate(&it.title, maxc), font, col);
            if resp.clicked() {
                jump = Some((it.file.clone(), it.line));
            }
        }
    });
    if let Some((f, l)) = jump {
        app.doc_collapsed.remove(&f);
        if let Some(i) = app.buffer_idx(&f) {
            app.buffers[i].goto_line(l);
        }
        app.thesis.active = Some(f);
    }
}

fn document_body(app: &mut App, ui: &mut Ui, pal: &Palette, files: &[String], now: f64) {
    let visual = app.settings.visual;
    let mut focused: Option<String> = None;
    let mut sync: Option<(String, usize)> = None;
    let mut open_code: Option<String> = None;
    let mut toggle: Option<String> = None;
    let mut vim_actions: Vec<(String, crate::vim::VimOut)> = vec![];
    let mut new_width: Option<(f32, bool)> = None; // (width, drag finished)
    let width_setting = app.settings.doc_width;
    let font_size = app.settings.font_size;
    egui::ScrollArea::vertical().id_salt("doc-scroll").auto_shrink([false, false]).show(ui, |ui| {
        let avail_w = ui.available_width();
        let max_col = (avail_w - 140.0).max(320.0);
        let col = width_setting.clamp(320.0, max_col);
        let pad = ((avail_w - col) / 2.0).max(0.0);
        let clip = ui.clip_rect();
        let x0 = ui.min_rect().min.x;
        let paper = Rect::from_min_max(pos2(x0 + pad - 56.0, clip.min.y - 1.0), pos2(x0 + pad + col + 56.0, clip.max.y + 1.0));
        ui.painter().rect_filled(paper, 0.0, pal.base);
        // drag handles on both paper edges change the text width symmetrically
        for (side, edge_x) in [(-1.0f32, paper.min.x), (1.0f32, paper.max.x)] {
            let hr = Rect::from_min_max(pos2(edge_x - 5.0, clip.min.y), pos2(edge_x + 5.0, clip.max.y));
            let resp = ui.interact(hr, ui.id().with(("doc-width", side as i32)), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            let active = resp.hovered() || resp.dragged();
            if active {
                ui.painter().line_segment([pos2(edge_x, clip.min.y), pos2(edge_x, clip.max.y)], Stroke::new(2.0, with_alpha(pal.accent, 160)));
                // grip in the middle of the visible edge
                let gy = clip.center().y;
                ui.painter().rect_filled(Rect::from_center_size(pos2(edge_x, gy), vec2(6.0, 44.0)), 3.0, pal.accent);
            }
            if resp.dragged() {
                let w = (col + 2.0 * side * resp.drag_delta().x).clamp(320.0, max_col);
                new_width = Some((w, false));
                // live readout: approx. characters per line
                let chars = (w / (font_size + 3.0) / 0.47).round() as i32;
                let label = format!("{:.0} px  ·  ≈ {chars} Zeichen/Zeile", w);
                let pill = Rect::from_center_size(pos2(paper.center().x, clip.min.y + 26.0), vec2(250.0, 28.0));
                ui.painter().rect_filled(pill, 14.0, pal.surface);
                ui.painter().rect_stroke(pill, 14.0, Stroke::new(1.0, pal.border), StrokeKind::Inside);
                ui.painter().text(pill.center(), Align2::CENTER_CENTER, label, widgets::ui_font(12.5), pal.text);
            }
            if resp.drag_stopped() {
                new_width = Some((new_width.map(|n| n.0).unwrap_or(col), true));
            }
            if resp.double_clicked() {
                new_width = Some((if visual { 780.0 } else { 980.0 }, true));
            }
            let _ = resp.on_hover_text("Ziehen: Breite ändern · Doppelklick: Standard");
        }
        for k in 1..=3 {
            let k = k as f32;
            ui.painter().rect_filled(Rect::from_min_max(pos2(paper.min.x - k * 2.0, paper.min.y), pos2(paper.min.x, paper.max.y)), 0.0, with_alpha(Color32::BLACK, (14.0 / k) as u8));
            ui.painter().rect_filled(Rect::from_min_max(pos2(paper.max.x, paper.min.y), pos2(paper.max.x + k * 2.0, paper.max.y)), 0.0, with_alpha(Color32::BLACK, (14.0 / k) as u8));
        }
        ui.horizontal_top(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(col);
                ui.add_space(40.0);
                for f in files {
                    let Some(bi) = app.buffer_idx(f) else { continue };
                    // divider
                    let (dr, dresp) = ui.allocate_exact_size(vec2(col, 30.0), Sense::click());
                    let collapsed = app.doc_collapsed.contains(f);
                    let p = ui.painter();
                    p.line_segment([pos2(dr.min.x, dr.center().y), pos2(dr.max.x, dr.center().y)], Stroke::new(1.0, with_alpha(pal.border, if dresp.hovered() { 200 } else { 90 })));
                    let label = match app.git_letter(f) {
                        Some(c) => format!("{}  {f}  · {c}", if collapsed { ic::CHEVRON_RIGHT } else { ic::CHEVRON_DOWN }),
                        None => format!("{}  {f}", if collapsed { ic::CHEVRON_RIGHT } else { ic::CHEVRON_DOWN }),
                    };
                    let g = p.layout_no_wrap(label, widgets::mono_font(11.0), if dresp.hovered() { pal.subtext } else { pal.dim });
                    let lr = Rect::from_min_size(pos2(dr.min.x, dr.center().y - 9.0), vec2(g.size().x + 16.0, 18.0));
                    p.rect_filled(lr, 9.0, pal.base);
                    p.galley(pos2(lr.min.x + 2.0, lr.center().y - g.size().y / 2.0), g, pal.dim);
                    let cr = Rect::from_min_size(pos2(dr.max.x - 74.0, dr.center().y - 9.0), vec2(74.0, 18.0));
                    let cresp = ui.interact(cr, ui.id().with(("open-code", f)), Sense::click());
                    if dresp.hovered() || cresp.hovered() {
                        ui.painter().rect_filled(cr, 9.0, pal.surface);
                        ui.painter().text(cr.center(), Align2::CENTER_CENTER, format!("{}  Code", ic::CODE), widgets::ui_font(11.0), if cresp.hovered() { pal.accent } else { pal.subtext });
                    }
                    if cresp.clicked() {
                        open_code = Some(f.clone());
                    } else if dresp.clicked() {
                        toggle = Some(f.clone());
                    }
                    if collapsed {
                        continue;
                    }
                    // culling of off-screen chapters
                    let top = ui.cursor().min.y;
                    let est = app.buffers[bi].doc_height.max(60.0);
                    let has_focus = ui.ctx().memory(|m| m.has_focus(app.buffers[bi].id));
                    let needed = Rect::from_min_size(pos2(dr.min.x, top), vec2(col, est)).intersects(clip.expand(600.0))
                        || app.buffers[bi].has_pending_select()
                        || has_focus
                        || app.buffers[bi].doc_height == 0.0;
                    if !needed {
                        ui.allocate_space(vec2(col, est));
                        continue;
                    }
                    let mut issues: HashMap<usize, (Level, String)> = HashMap::new();
                    for i in &app.thesis.issues {
                        if i.file.as_deref() == Some(f.as_str()) {
                            if let Some(l) = i.line {
                                issues.entry(l).or_insert((i.level, i.message.clone()));
                            }
                        }
                    }
                    let root = app.project.root.clone();
                    let marks = app.git.line_marks(&root, f, app.buffers[bi].disk_stamp);
                    let style = EditorStyle { pal: &app.pal, syntax: &app.syntax, font_size: app.settings.font_size, style_rev: app.style_rev, issues: &issues, visual, embedded: true, git_marks: &marks, search: app.find.open.then_some(app.find.query.as_str()) };
                    let src = CompletionSources { cites: &app.cites, labels: &app.labels, files: &app.flat };
                    let out = editor::editor_ui(ui, &mut app.buffers[bi], &style, &src, if app.settings.input_vim && !visual { Some(&mut app.vim) } else { None });
                    if let Some(vo) = out.vim.as_ref() {
                        if vo.save || vo.close || vo.search.is_some() || vo.noh || vo.yanked.is_some() {
                            vim_actions.push((f.clone(), out.vim.clone().unwrap()));
                        }
                    }
                    let b = &mut app.buffers[bi];
                    b.doc_height = ui.cursor().min.y - top;
                    if out.changed {
                        b.last_edit = now;
                    }
                    if ui.ctx().memory(|m| m.has_focus(b.id)) {
                        focused = Some(f.clone());
                    }
                    if let Some(l) = out.ctrl_click_line {
                        sync = Some((f.clone(), l));
                    }
                    ui.add_space(18.0);
                }
                ui.add_space(clip.height() * 0.5);
            });
        });
    });
    for (f, vo) in vim_actions {
        apply_vim(app, ui.ctx(), vo, Tab::Thesis, &f, now);
    }
    if let Some((w, done)) = new_width {
        app.settings.doc_width = w;
        if done {
            app.settings.save();
        }
    }
    if let Some(f) = focused {
        if !app.thesis.tabs.contains(&f) {
            app.thesis.tabs.push(f.clone());
        }
        app.thesis.active = Some(f);
    }
    if let Some(f) = toggle {
        if !app.doc_collapsed.remove(&f) {
            app.doc_collapsed.insert(f);
        }
    }
    if let Some(f) = open_code {
        app.doc_mode = false;
        if app.focus {
            app.set_focus(false, ui.ctx());
        }
        app.settings.visual = false;
        app.open_file(&f, Tab::Thesis);
    }
    if let Some((f, l)) = sync {
        app.doc_pdf = true;
        app.forward_sync(Tab::Thesis, &f, l, true, now);
    }
}

// ───────────────────────────── git ─────────────────────────────

fn git_panel(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab, now: f64) {
    let root = app.project.root.clone();
    let ctx = ui.ctx().clone();
    if app.git.last_refresh < 0.0 {
        app.git.refresh(&root, now);
    }
    header_row(ui, "Git", pal, |ui| {
        if widgets::icon_button_sized(ui, ic::REFRESH, "Aktualisieren", pal, false, 24.0).clicked() {
            app.git.refresh(&root, now);
        }
    });
    if !app.git.git_available {
        ui.label(egui::RichText::new("git ist nicht installiert.").color(pal.dim));
        return;
    }
    if !app.git.is_repo {
        Frame::new().fill(pal.surface).corner_radius(10).inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(ic::GIT).font(widgets::ui_font(26.0)).color(pal.accent));
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Noch nicht versioniert").font(widgets::bold_font(14.0)).color(pal.bright));
            ui.label(egui::RichText::new("Mit Git sicherst du jeden Stand deiner Arbeit und kannst jederzeit zu früheren Versionen zurück.").font(widgets::ui_font(12.0)).color(pal.subtext));
            ui.add_space(10.0);
            if widgets::button(ui, ic::PLUS, "Repository anlegen", pal, BtnKind::Primary).clicked() {
                app.git_init(now);
            }
        });
        return;
    }

    // branch / remote
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        widgets::badge(ui, &format!("{}  {}", ic::BRANCH, app.git.branch), pal.accent);
        if app.git.remote.is_some() {
            if app.git.ahead > 0 {
                widgets::badge(ui, &format!("{} {}", ic::ARROW_UP, app.git.ahead), pal.green);
            }
            if app.git.behind > 0 {
                widgets::badge(ui, &format!("{} {}", ic::ARROW_DOWN, app.git.behind), pal.yellow);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if app.git.busy {
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    widgets::draw_spinner(ui, r.center(), 6.0, pal.accent);
                } else {
                    if widgets::icon_button_sized(ui, ic::UPLOAD, "Push – hochladen", pal, false, 24.0).clicked() {
                        app.git.sync(&root, false, ctx.clone());
                    }
                    if widgets::icon_button_sized(ui, ic::DOWNLOAD, "Pull – Änderungen holen", pal, false, 24.0).clicked() {
                        app.git.sync(&root, true, ctx.clone());
                    }
                }
            });
        }
    });
    if app.git.remote.is_none() {
        ui.add_space(4.0);
        ui.collapsing(egui::RichText::new("Remote verbinden (GitHub, GitLab …)").font(widgets::ui_font(11.5)).color(pal.dim), |ui| {
            let id = egui::Id::new("git-remote-url");
            let mut url: String = ui.data(|d| d.get_temp(id)).unwrap_or_default();
            ui.add(egui::TextEdit::singleline(&mut url).hint_text("git@github.com:name/masterarbeit.git").desired_width(f32::INFINITY));
            ui.data_mut(|d| d.insert_temp(id, url.clone()));
            if widgets::button(ui, ic::LINK, "Verbinden", pal, BtnKind::Secondary).clicked() && !url.trim().is_empty() {
                let r = crate::platform::cmd("git").current_dir(&root).args(["remote", "add", "origin", url.trim()]).output();
                if r.map(|o| o.status.success()).unwrap_or(false) {
                    app.git.refresh(&root, now);
                    app.git.sync(&root, false, ctx.clone());
                }
            }
        });
    }
    ui.add_space(8.0);

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // changes
        let n = app.git.changes.len();
        let n_sel = n - app.git.excluded.len().min(n);
        ui.horizontal(|ui| {
            widgets::section_label(ui, &format!("Änderungen ({n})"), pal);
            if n > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button_sized(ui, ic::UNDO, "Alle Änderungen verwerfen …", pal, false, 22.0).clicked() {
                        app.dialog = Some(crate::app::Dialog::GitRevert { paths: vec![String::new()] });
                    }
                    if n < 2 {
                        return;
                    }
                    let all = app.git.excluded.is_empty();
                    if ui.add(egui::Label::new(egui::RichText::new(if all { "Alle abwählen" } else { "Alle auswählen" }).font(widgets::ui_font(11.0)).color(pal.accent)).sense(Sense::click())).clicked() {
                        if all {
                            app.git.excluded = app.git.changes.iter().map(|c| c.path.clone()).collect();
                        } else {
                            app.git.excluded.clear();
                        }
                    }
                });
            }
        });
        if n == 0 {
            ui.label(egui::RichText::new(format!("{}  Alles gesichert", ic::CHECK)).font(widgets::ui_font(12.0)).color(pal.green));
        }
        let mut open_diff: Option<String> = None;
        let selected = match &app.git.view {
            Some(crate::git::GitView::WorkingFile(p)) => Some(p.clone()),
            _ => None,
        };
        for c in app.git.changes.clone() {
            let (label, letter) = c.label();
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
            if selected.as_deref() == Some(c.path.as_str()) {
                ui.painter().rect_filled(rect, 6.0, with_alpha(pal.accent, 30));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, with_alpha(pal.text, 10));
            }
            let col = widgets::git_color(letter, pal);
            // include-in-commit checkbox
            let included = !app.git.excluded.contains(&c.path);
            let cb = Rect::from_center_size(pos2(rect.min.x + 12.0, rect.center().y), vec2(14.0, 14.0));
            let cresp = ui.interact(cb.expand(3.0), ui.id().with(("git-inc", &c.path)), Sense::click()).on_hover_text("Im nächsten Commit enthalten");
            ui.painter().rect_stroke(cb, 3.0, Stroke::new(1.2, if included { pal.accent } else { pal.dim }), StrokeKind::Inside);
            if included {
                ui.painter().rect_filled(cb.shrink(1.0), 2.0, pal.accent);
                ui.painter().text(cb.center(), Align2::CENTER_CENTER, ic::CHECK, widgets::ui_font(8.5), pal.on_accent);
            }
            if cresp.clicked() {
                if included {
                    app.git.excluded.insert(c.path.clone());
                } else {
                    app.git.excluded.remove(&c.path);
                }
            }
            let name = c.path.rsplit('/').next().unwrap_or(&c.path).to_string();
            let dir = c.path.strip_suffix(&name).unwrap_or("").trim_end_matches('/').to_string();
            let g = ui.painter().layout_no_wrap(name, widgets::ui_font(12.5), if included { mix(col, pal.text, 0.45) } else { pal.dim });
            let nw = g.size().x;
            ui.painter().galley(pos2(rect.min.x + 28.0, rect.center().y - g.size().y / 2.0), g, pal.text);
            if !dir.is_empty() {
                ui.painter().text(pos2(rect.min.x + 34.0 + nw, rect.center().y), Align2::LEFT_CENTER, truncate(&dir, 22), widgets::ui_font(11.0), pal.dim);
            }
            ui.painter().text(pos2(rect.max.x - 10.0, rect.center().y), Align2::RIGHT_CENTER, letter.to_string(), widgets::mono_font(11.5), col);
            let resp = resp.on_hover_text(format!("{label}: {}", c.path));
            if resp.clicked() {
                open_diff = Some(c.path.clone());
            }
            resp.context_menu(|ui| {
                if crate::project::is_text_file(&c.path) && ui.button(format!("{}  Datei öffnen", ic::FILE_TEXT)).clicked() {
                    app.git.view = None;
                    app.open_file(&c.path, t);
                    ui.close();
                }
                if ui.button(egui::RichText::new(format!("{}  Änderungen verwerfen", ic::UNDO)).color(pal.red)).clicked() {
                    app.dialog = Some(crate::app::Dialog::GitRevert { paths: vec![c.path.clone()] });
                    ui.close();
                }
            });
        }
        if let Some(p) = open_diff {
            app.save_all();
            app.git.open_working_diff(&root, &p);
        }
        ui.add_space(8.0);
        ui.add(egui::TextEdit::multiline(&mut app.git.message).hint_text("Was hast du geändert? (optional)").desired_rows(2).desired_width(f32::INFINITY));
        ui.add_space(6.0);
        ui.add_enabled_ui(n_sel > 0, |ui| {
            let label = if n_sel == n { "Commit – Stand sichern".to_string() } else { format!("Commit – {n_sel} von {n} Dateien") };
            if widgets::button(ui, ic::CHECK, &label, pal, BtnKind::Primary).clicked() {
                app.save_all();
                match app.git.commit(&root) {
                    Ok(()) => {
                        let g = pal.green;
                        app.toast(ic::GIT, "Neuer Stand gesichert", g, now);
                        app.git.view = None;
                    }
                    Err(e) => {
                        let r = pal.red;
                        app.toast(ic::WARN, format!("Git: {}", truncate(&e, 90)), r, now);
                    }
                }
                app.git.refresh(&root, now);
            }
        });
        ui.add_space(14.0);

        // history
        widgets::section_label(ui, &format!("Verlauf ({})", app.git.log.len()), pal);
        let current = match &app.git.view {
            Some(crate::git::GitView::Commit(h)) => Some(h.clone()),
            _ => None,
        };
        let mut open: Option<String> = None;
        let log = app.git.log.clone();
        for (i, c) in log.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
            let sel = current.as_deref() == Some(c.hash.as_str());
            if sel {
                ui.painter().rect_filled(rect, 7.0, with_alpha(pal.accent, 30));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 7.0, with_alpha(pal.text, 10));
            }
            // timeline
            let x = rect.min.x + 12.0;
            if i + 1 < log.len() {
                ui.painter().line_segment([pos2(x, rect.center().y), pos2(x, rect.max.y + 4.0)], Stroke::new(1.5, with_alpha(pal.border, 200)));
            }
            if i > 0 {
                ui.painter().line_segment([pos2(x, rect.min.y - 4.0), pos2(x, rect.center().y)], Stroke::new(1.5, with_alpha(pal.border, 200)));
            }
            ui.painter().circle_filled(pos2(x, rect.center().y), if i == 0 { 5.0 } else { 3.5 }, if i == 0 { pal.accent } else { mix(pal.accent, pal.dim, 0.5) });
            let maxc = ((rect.width() - 34.0) / 6.8).max(8.0) as usize;
            ui.painter().text(pos2(rect.min.x + 26.0, rect.min.y + 14.0), Align2::LEFT_CENTER, truncate(&c.subject, maxc), widgets::bold_font(12.5), if sel { pal.bright } else { pal.text });
            ui.painter().text(
                pos2(rect.min.x + 26.0, rect.min.y + 31.0),
                Align2::LEFT_CENTER,
                format!("{}  ·  {}  ·  {}", crate::git::relative_time(c.time), truncate(&c.author, 16), c.short),
                widgets::ui_font(11.0),
                pal.dim,
            );
            if resp.clicked() {
                open = Some(c.hash.clone());
            }
        }
        if let Some(h) = open {
            app.git.open_commit(&root, &h);
        }
    });
}

fn git_view(app: &mut App, ui: &mut Ui, pal: &Palette, t: Tab) {
    let root = app.project.root.clone();
    let Some(view) = app.git.view.clone() else { return };
    // header
    let bar = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover()).0;
    ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 120)));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(16.0, 6.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let (icon, sub) = match &view {
        crate::git::GitView::WorkingFile(_) => (ic::PENCIL, "Ungesicherte Änderungen seit dem letzten Commit".to_string()),
        crate::git::GitView::Commit(h) => {
            let info = app.git.log.iter().find(|x| &x.hash == h).map(|x| format!("{}  ·  {}  ·  {}", x.short, x.author, crate::git::relative_time(x.time))).unwrap_or_default();
            (ic::HISTORY, info)
        }
    };
    c.label(egui::RichText::new(icon).font(widgets::ui_font(16.0)).color(pal.accent));
    c.vertical(|ui| {
        ui.label(egui::RichText::new(truncate(&app.git.view_title, 80)).font(widgets::bold_font(14.5)).color(pal.bright));
        ui.label(egui::RichText::new(sub).font(widgets::ui_font(11.5)).color(pal.dim));
    });
    c.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::icon_button_sized(ui, ic::TIMES, "Schließen", pal, false, 28.0).clicked() {
            app.git.view = None;
        }
        if let crate::git::GitView::WorkingFile(p) = &view {
            if widgets::button(ui, ic::UNDO, "Verwerfen", pal, BtnKind::Danger).clicked() {
                app.dialog = Some(crate::app::Dialog::GitRevert { paths: vec![p.clone()] });
            }
            if crate::project::is_text_file(p) && widgets::button(ui, ic::FILE_TEXT, "Öffnen", pal, BtnKind::Secondary).clicked() {
                let p = p.clone();
                app.git.view = None;
                app.open_file(&p, t);
            }
        }
    });
    if app.git.view.is_none() {
        return;
    }
    // files of a commit, each restorable
    if let crate::git::GitView::Commit(h) = &view {
        if !app.git.view_files.is_empty() {
            Frame::new().inner_margin(Margin { left: 16, right: 16, top: 8, bottom: 8 }).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for f in app.git.view_files.clone() {
                        let col = match f.code.chars().next() {
                            Some('A') => pal.green,
                            Some('D') => pal.red,
                            _ => pal.yellow,
                        };
                        Frame::new().fill(pal.surface).corner_radius(8).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&f.code[..1]).font(widgets::mono_font(11.5)).color(col));
                                ui.label(egui::RichText::new(&f.path).font(widgets::ui_font(12.0)).color(pal.text));
                                if !f.code.starts_with('D') && widgets::icon_button_sized(ui, ic::UNDO, "Datei auf diesen Stand zurücksetzen", pal, false, 22.0).clicked() {
                                    app.dialog = Some(crate::app::Dialog::GitRestore { hash: h.clone(), path: f.path.clone() });
                                }
                            });
                        });
                    }
                });
            });
        }
    }
    // diff
    let lines = &app.git.view_lines;
    let row_h = 19.0;
    egui::ScrollArea::both().auto_shrink([false, false]).id_salt("git-diff").show_rows(ui, row_h, lines.len(), |ui, range| {
        for (k, l) in &lines[range] {
            let (bg, fg) = match k {
                crate::git::DiffKind::Add => (with_alpha(pal.green, 28), mix(pal.green, pal.text, 0.35)),
                crate::git::DiffKind::Del => (with_alpha(pal.red, 28), mix(pal.red, pal.text, 0.35)),
                crate::git::DiffKind::Hunk => (with_alpha(pal.cyan, 18), pal.cyan),
                crate::git::DiffKind::Header => (Color32::TRANSPARENT, pal.dim),
                crate::git::DiffKind::Ctx => (Color32::TRANSPARENT, pal.subtext),
            };
            let g = ui.painter().layout_no_wrap(l.replace('\t', "    "), widgets::mono_font(12.5), fg);
            let w = (g.size().x + 40.0).max(ui.available_width());
            let (rect, _) = ui.allocate_exact_size(vec2(w, row_h), Sense::hover());
            if bg != Color32::TRANSPARENT {
                ui.painter().rect_filled(rect, 0.0, bg);
            }
            ui.painter().galley(pos2(rect.min.x + 16.0, rect.center().y - g.size().y / 2.0), g, fg);
        }
    });
    let _ = root;
}
