//! Settings window: navigation on the left, pages on the right, theme gallery with live
//! previews of every theme.

use crate::app::{App, Dialog};
use crate::icons as ic;
use crate::theme::{self, mix, with_alpha, Palette};
use crate::widgets;
use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Appearance,
    Editor,
    Project,
    General,
}

impl Page {
    fn all() -> [Page; 4] {
        [Page::Appearance, Page::Editor, Page::Project, Page::General]
    }
    fn icon(self) -> &'static str {
        match self {
            Page::Appearance => ic::BRUSH,
            Page::Editor => ic::PENCIL,
            Page::Project => ic::GRADUATION,
            Page::General => ic::COG,
        }
    }
    fn title(self) -> &'static str {
        match self {
            Page::Appearance => tr!("Darstellung" | "Appearance"),
            Page::Editor => "Editor",
            Page::Project => tr!("Projekt" | "Project"),
            Page::General => tr!("Allgemein" | "General"),
        }
    }
    fn subtitle(self) -> &'static str {
        match self {
            Page::Appearance => tr!("Theme und Schrift" | "Theme and font"),
            Page::Editor => tr!("Eingabe und Kompilieren" | "Input and compiling"),
            Page::Project => tr!("Compiler und Präsentation" | "Compiler and presentation"),
            Page::General => tr!("Sprache, Updates, Hilfe" | "Language, updates, help"),
        }
    }
}

// ───────────────────────────── small widgets ─────────────────────────────

/// iOS-style switch.
fn toggle(ui: &mut Ui, on: &mut bool, pal: &Palette) -> egui::Response {
    let (r, mut resp) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let off = mix(pal.surface, pal.text, 0.22);
    let track = mix(off, pal.accent, t);
    ui.painter().rect_filled(r, 11.0, track);
    ui.painter().rect_stroke(r, 11.0, Stroke::new(1.0, with_alpha(pal.border, ((1.0 - t) * 200.0) as u8)), StrokeKind::Inside);
    let x = egui::lerp(r.min.x + 11.0..=r.max.x - 11.0, t);
    ui.painter().circle_filled(pos2(x, r.center().y + 1.0), 8.5, with_alpha(Color32::BLACK, 50));
    ui.painter().circle_filled(pos2(x, r.center().y), 8.5, if t > 0.5 { pal.on_accent } else { pal.bright });
    resp
}

/// Segmented control; returns the clicked index.
fn segmented(ui: &mut Ui, options: &[(&str, &str)], selected: usize, pal: &Palette, id: Id) -> Option<usize> {
    let font = widgets::ui_font(13.0);
    let logo = |icon: &str| icon == widgets::VIM_LOGO;
    let widths: Vec<f32> = options
        .iter()
        .map(|(icon, label)| {
            let t = if icon.is_empty() || logo(icon) { label.to_string() } else { format!("{icon}  {label}") };
            ui.painter().layout_no_wrap(t, font.clone(), pal.text).size().x + 28.0 + if logo(icon) { 27.0 } else { 0.0 }
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (r, _) = ui.allocate_exact_size(vec2(total, 34.0), Sense::hover());
    ui.painter().rect_filled(r, 9.0, mix(pal.base, pal.crust, 0.5));
    ui.painter().rect_stroke(r, 9.0, Stroke::new(1.0, with_alpha(pal.border, 140)), StrokeKind::Inside);
    // sliding highlight
    let mut xs = vec![];
    let mut x = r.min.x + 3.0;
    for w in &widths {
        xs.push(x);
        x += w;
    }
    let target = xs.get(selected).copied().unwrap_or(r.min.x + 3.0);
    let ax = ui.ctx().animate_value_with_time(id.with("x"), target, 0.15);
    let aw = ui.ctx().animate_value_with_time(id.with("w"), widths.get(selected).copied().unwrap_or(0.0), 0.15);
    let hl = Rect::from_min_size(pos2(ax, r.min.y + 3.0), vec2(aw, r.height() - 6.0));
    ui.painter().rect_filled(hl.translate(vec2(0.0, 1.0)), 7.0, with_alpha(Color32::BLACK, 40));
    ui.painter().rect_filled(hl, 7.0, pal.surface);
    ui.painter().rect_stroke(hl, 7.0, Stroke::new(1.0, with_alpha(pal.accent, 120)), StrokeKind::Inside);
    let mut clicked = None;
    for (i, ((icon, label), w)) in options.iter().zip(&widths).enumerate() {
        let cell = Rect::from_min_size(pos2(xs[i], r.min.y), vec2(*w, r.height()));
        let resp = ui.interact(cell, id.with(i), Sense::click());
        let sel = i == selected;
        let col = if sel { pal.bright } else if resp.hovered() { pal.text } else { pal.subtext };
        if logo(icon) {
            let g = ui.painter().layout_no_wrap(label.to_string(), font.clone(), col);
            let total = 20.0 + 7.0 + g.size().x;
            let x0 = cell.center().x - total / 2.0;
            widgets::paint_vim_logo(ui.painter(), pos2(x0 + 10.0, cell.center().y), 20.0);
            ui.painter().galley(pos2(x0 + 27.0, cell.center().y - g.size().y / 2.0), g, col);
        } else {
            let t = if icon.is_empty() { label.to_string() } else { format!("{icon}  {label}") };
            ui.painter().text(cell.center(), Align2::CENTER_CENTER, t, font.clone(), col);
        }
        if resp.clicked() && !sel {
            clicked = Some(i);
        }
    }
    clicked
}

/// A settings row: title + description on the left, control on the right.
fn row(ui: &mut Ui, pal: &Palette, title: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, 52.0), egui::Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_height(52.0);
        ui.vertical(|ui| {
            ui.set_max_width(w * 0.56);
            ui.add_space(2.0);
            ui.label(egui::RichText::new(title).font(widgets::bold_font(13.5)).color(pal.bright));
            if !desc.is_empty() {
                ui.label(egui::RichText::new(desc).font(widgets::ui_font(12.0)).color(pal.dim));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(Align::Center), control);
    });
}

fn card(ui: &mut Ui, pal: &Palette, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(mix(pal.surface, pal.base, 0.35))
        .stroke(Stroke::new(1.0, with_alpha(pal.border, 110)))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(18, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn divider(ui: &mut Ui, pal: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, with_alpha(pal.border, 70));
}

fn heading(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.add_space(14.0);
    ui.label(egui::RichText::new(text.to_uppercase()).font(widgets::ui_font(11.0)).color(pal.dim).extra_letter_spacing(1.3));
    ui.add_space(6.0);
}

// ───────────────────────────── theme gallery ─────────────────────────────

/// Miniature of the nEdit window in a theme's colors.
fn theme_preview(p: &egui::Painter, r: Rect, t: &Palette) {
    let rad = CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 };
    p.rect_filled(r, rad, t.crust);
    let top = Rect::from_min_size(r.min, vec2(r.width(), 13.0));
    p.rect_filled(top, CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, t.mantle);
    p.circle_filled(pos2(top.min.x + 9.0, top.center().y), 3.2, t.accent);
    p.rect_filled(Rect::from_center_size(pos2(top.center().x, top.center().y), vec2(36.0, 5.0)), 2.5, with_alpha(t.text, 60));
    p.rect_filled(Rect::from_min_size(pos2(top.max.x - 22.0, top.min.y + 3.5), vec2(14.0, 6.0)), 2.0, t.accent);
    let body = Rect::from_min_max(pos2(r.min.x, top.max.y), r.max);
    // sidebar
    let side = Rect::from_min_size(body.min, vec2(body.width() * 0.2, body.height()));
    p.rect_filled(side, 0.0, t.mantle);
    for i in 0..5 {
        let y = side.min.y + 9.0 + i as f32 * 8.0;
        let w = side.width() * [0.62, 0.48, 0.7, 0.4, 0.55][i];
        let c = if i == 2 { t.accent } else { with_alpha(t.text, 70) };
        p.rect_filled(Rect::from_min_size(pos2(side.min.x + 6.0, y), vec2(w - 6.0, 3.0)), 1.5, c);
    }
    // editor with "code"
    let ed = Rect::from_min_max(pos2(side.max.x, body.min.y), pos2(body.min.x + body.width() * 0.68, body.max.y));
    p.rect_filled(ed, 0.0, t.base);
    let cols = [t.blue, t.text, t.green, t.text, t.magenta, t.yellow, t.text, t.cyan];
    let lens = [0.55, 0.8, 0.45, 0.7, 0.35, 0.6, 0.75, 0.4];
    for i in 0..8 {
        let y = ed.min.y + 8.0 + i as f32 * 7.5;
        if y > ed.max.y - 6.0 {
            break;
        }
        let x0 = ed.min.x + 8.0 + if i % 3 == 1 { 6.0 } else { 0.0 };
        let w = (ed.width() - 16.0) * lens[i];
        let split = w * 0.3;
        p.rect_filled(Rect::from_min_size(pos2(x0, y), vec2(split, 3.0)), 1.5, cols[i]);
        p.rect_filled(Rect::from_min_size(pos2(x0 + split + 3.0, y), vec2(w - split, 3.0)), 1.5, with_alpha(t.text, 110));
    }
    // cursor line
    p.rect_filled(Rect::from_min_size(pos2(ed.min.x, ed.min.y + 28.5), vec2(ed.width(), 7.0)), 0.0, with_alpha(t.text, 10));
    // pdf pane
    let pdf = Rect::from_min_max(pos2(ed.max.x, body.min.y), body.max);
    p.rect_filled(pdf, 0.0, t.crust);
    let page = Rect::from_min_max(pdf.min + vec2(7.0, 7.0), pos2(pdf.max.x - 7.0, pdf.max.y + 4.0));
    p.rect_filled(page, 2.0, Color32::from_gray(246));
    for i in 0..4 {
        let y = page.min.y + 8.0 + i as f32 * 6.0;
        p.rect_filled(Rect::from_min_size(pos2(page.min.x + 5.0, y), vec2((page.width() - 10.0) * [0.6, 0.9, 0.8, 0.7][i], 2.0)), 1.0, Color32::from_gray(if i == 0 { 120 } else { 190 }));
    }
}

/// One card of the gallery. Returns true when clicked.
fn theme_card(ui: &mut Ui, pal: &Palette, t: &Palette, name: &str, badge: Option<&str>, selected: bool, w: f32) -> bool {
    let h = w * 0.62 + 40.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let hover = ui.ctx().animate_bool(resp.id.with("h"), resp.hovered());
    let lift = hover * 2.0;
    let r = r.translate(vec2(0.0, -lift));
    if !ui.is_rect_visible(r) {
        return resp.clicked();
    }
    let p = ui.painter();
    p.rect_filled(r.translate(vec2(0.0, 3.0 + lift)), 12.0, with_alpha(Color32::BLACK, (40.0 + hover * 40.0) as u8));
    p.rect_filled(r, 12.0, mix(pal.surface, pal.base, 0.2));
    let prev = Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(w - 12.0, w * 0.62 - 6.0));
    theme_preview(p, prev, t);
    // name row
    let ny = prev.max.y + 17.0;
    let mut x = r.min.x + 12.0;
    // palette dots
    for c in [t.accent, t.green, t.magenta, t.blue] {
        p.circle_filled(pos2(x + 4.0, ny), 4.0, c);
        x += 9.0;
    }
    let g = p.layout_no_wrap(name.to_string(), widgets::bold_font(12.5), if selected { pal.bright } else { pal.text });
    let max_w = r.max.x - x - if badge.is_some() { 64.0 } else { 34.0 };
    p.with_clip_rect(Rect::from_min_max(pos2(x, r.min.y), pos2(x + 6.0 + max_w, r.max.y))).galley(pos2(x + 6.0, ny - g.size().y / 2.0), g, pal.text);
    if let Some(b) = badge {
        let bg = p.layout_no_wrap(b.to_string(), widgets::ui_font(10.0), pal.subtext);
        let br = Rect::from_min_size(pos2(r.max.x - 12.0 - bg.size().x - 10.0 - if selected { 26.0 } else { 0.0 }, ny - 8.0), vec2(bg.size().x + 10.0, 16.0));
        p.rect_filled(br, 8.0, with_alpha(pal.text, 18));
        p.galley(br.center() - bg.size() / 2.0, bg, pal.subtext);
    }
    if selected {
        let c = pos2(r.max.x - 18.0, ny);
        p.circle_filled(c, 9.0, pal.accent);
        p.text(c, Align2::CENTER_CENTER, ic::CHECK, widgets::ui_font(9.5), pal.on_accent);
    }
    let border = if selected { Stroke::new(2.0, pal.accent) } else { Stroke::new(1.0, with_alpha(pal.border, (110.0 + hover * 100.0) as u8)) };
    p.rect_stroke(r, 12.0, border, StrokeKind::Inside);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// Theme palettes are parsed once per opening of the window.
fn cached_palette(ctx: &egui::Context, name: &str) -> Option<Palette> {
    let id = Id::new(("theme-preview", name));
    if let Some(p) = ctx.data(|d| d.get_temp::<Option<Palette>>(id)) {
        return p;
    }
    let p = theme::load_named(name);
    ctx.data_mut(|d| d.insert_temp(id, p.clone()));
    p
}

fn gallery(app: &mut App, ui: &mut Ui, pal: &Palette, ctx: &egui::Context) {
    let omarchy = theme::omarchy_installed();
    let follow = app.settings.theme.is_none();
    let mut entries: Vec<(Option<String>, String, Palette, Option<&str>)> = vec![];
    if omarchy {
        let cur = theme::current_omarchy_name();
        let p = cur.as_deref().and_then(theme::load_named).unwrap_or_else(Palette::default_dark);
        let label = cur.map(|n| theme::pretty_name(&n)).unwrap_or_else(|| "Omarchy".into());
        entries.push((None, label, p, Some("Omarchy")));
    }
    entries.push((Some("nedit".into()), "nEdit Ink".into(), Palette::default_dark(), Some(tr!("Standard" | "Default"))));
    for name in theme::all_theme_names() {
        if let Some(p) = cached_palette(ctx, &name) {
            let badge = if p.dark { None } else { Some(tr!("hell" | "light")) };
            entries.push((Some(name.clone()), theme::pretty_name(&name), p, badge));
        }
    }
    let cols = ((ui.available_width() + 14.0) / 214.0).floor().max(2.0) as usize;
    let gap = 14.0;
    let w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
    let mut picked: Option<Option<String>> = None;
    for chunk in entries.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (key, label, p, badge) in chunk {
                let selected = match key {
                    None => follow && omarchy,
                    Some(k) if k == "nedit" => app.settings.theme.as_deref() == Some("nedit") || (follow && !omarchy),
                    Some(k) => app.settings.theme.as_deref() == Some(k.as_str()),
                };
                if theme_card(ui, pal, p, label, *badge, selected, w) {
                    picked = Some(key.clone());
                }
            }
        });
        ui.add_space(gap);
    }
    if let Some(k) = picked {
        app.set_theme(k, ctx);
    }
}

// ───────────────────────────── window ─────────────────────────────

impl App {
    pub fn open_settings(&mut self, page: Option<Page>, ctx: &egui::Context) {
        // re-read themes (Omarchy themes may have been installed meanwhile)
        for name in theme::all_theme_names() {
            ctx.data_mut(|d| d.remove::<Option<Palette>>(Id::new(("theme-preview", name.as_str()))));
        }
        if let Some(p) = page {
            self.settings_page = p;
        }
        self.settings_open = true;
    }

    pub fn settings_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.settings_open {
            return;
        }
        let screen = ctx.content_rect();
        let size = vec2((screen.width() - 80.0).clamp(560.0, 980.0), (screen.height() - 90.0).clamp(420.0, 680.0));
        let modal = egui::Modal::new(Id::new("settings-window"))
            .backdrop_color(with_alpha(Color32::BLACK, 120))
            .frame(
                egui::Frame::new()
                    .fill(pal.base)
                    .stroke(Stroke::new(1.0, with_alpha(pal.border, 160)))
                    .corner_radius(16)
                    .shadow(egui::Shadow { offset: [0, 18], blur: 48, spread: 0, color: with_alpha(Color32::BLACK, 130) }),
            )
            .show(ctx, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                let full = ui.max_rect();
                let nav_w = 220.0;
                let nav = Rect::from_min_size(full.min, vec2(nav_w, full.height()));
                ui.painter().rect_filled(nav, CornerRadius { nw: 16, sw: 16, ne: 0, se: 0 }, pal.mantle);
                ui.painter().line_segment([nav.right_top(), nav.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 90)));
                // navigation
                let mut nui = ui.new_child(egui::UiBuilder::new().max_rect(nav.shrink2(vec2(14.0, 18.0))).layout(egui::Layout::top_down(Align::Min)));
                nui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(tr!("Einstellungen" | "Settings")).font(widgets::display_font(21.0)).color(pal.bright));
                });
                nui.add_space(18.0);
                for page in Page::all() {
                    let sel = self.settings_page == page;
                    let (r, resp) = nui.allocate_exact_size(vec2(nav_w - 28.0, 46.0), Sense::click());
                    let hov = nui.ctx().animate_bool(resp.id, resp.hovered() || sel);
                    if hov > 0.0 {
                        nui.painter().rect_filled(r, 10.0, with_alpha(if sel { pal.accent } else { pal.text }, if sel { 34 } else { (12.0 * hov) as u8 }));
                    }
                    if sel {
                        nui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x, r.min.y + 12.0), vec2(3.0, 22.0)), 2.0, pal.accent);
                    }
                    let ir = Rect::from_center_size(pos2(r.min.x + 24.0, r.center().y), vec2(28.0, 28.0));
                    nui.painter().rect_filled(ir, 8.0, if sel { pal.accent } else { with_alpha(pal.text, 16) });
                    nui.painter().text(ir.center(), Align2::CENTER_CENTER, page.icon(), widgets::ui_font(12.5), if sel { pal.on_accent } else { pal.subtext });
                    nui.painter().text(pos2(r.min.x + 46.0, r.center().y - 7.5), Align2::LEFT_CENTER, page.title(), widgets::bold_font(13.5), if sel { pal.bright } else { pal.text });
                    nui.painter().text(pos2(r.min.x + 46.0, r.center().y + 8.5), Align2::LEFT_CENTER, page.subtitle(), widgets::ui_font(11.0), pal.dim);
                    if resp.clicked() {
                        self.settings_page = page;
                    }
                    nui.add_space(4.0);
                }
                // version at the bottom of the navigation
                nui.painter().text(pos2(nav.min.x + 22.0, nav.max.y - 22.0), Align2::LEFT_CENTER, format!("nEdit {}", crate::updater::VERSION), widgets::ui_font(11.0), pal.dim);

                // close button
                let cr = Rect::from_center_size(pos2(full.max.x - 26.0, full.min.y + 26.0), vec2(28.0, 28.0));
                let cresp = ui.interact(cr, Id::new("settings-close"), Sense::click()).on_hover_text(tr!("Schließen (Esc)" | "Close (Esc)"));
                ui.painter().rect_filled(cr, 8.0, with_alpha(pal.text, if cresp.hovered() { 26 } else { 0 }));
                ui.painter().text(cr.center(), Align2::CENTER_CENTER, ic::TIMES, widgets::ui_font(13.0), pal.subtext);
                if cresp.clicked() {
                    self.settings_open = false;
                }

                // page content
                let content = Rect::from_min_max(pos2(nav.max.x + 30.0, full.min.y + 22.0), pos2(full.max.x - 30.0, full.max.y - 10.0));
                let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(Align::Min)));
                cui.label(egui::RichText::new(self.settings_page.title()).font(widgets::display_font(26.0)).color(pal.bright));
                cui.label(egui::RichText::new(self.settings_page.subtitle()).font(widgets::ui_font(12.5)).color(pal.dim));
                cui.add_space(8.0);
                egui::ScrollArea::vertical().id_salt(("settings-page", self.settings_page as u8)).auto_shrink([false, false]).show(&mut cui, |ui| {
                    ui.set_width(content.width() - 12.0);
                    match self.settings_page {
                        Page::Appearance => self.page_appearance(ui, pal, ctx),
                        Page::Editor => self.page_editor(ui, pal),
                        Page::Project => self.page_project(ui, pal),
                        Page::General => self.page_general(ui, pal, ctx),
                    }
                    ui.add_space(16.0);
                });
            });
        if modal.should_close() {
            self.settings_open = false;
        }
    }

    fn page_appearance(&mut self, ui: &mut Ui, pal: &Palette, ctx: &egui::Context) {
        heading(ui, pal, "Theme");
        gallery(self, ui, pal, ctx);
        heading(ui, pal, tr!("Schrift" | "Font"));
        card(ui, pal, |ui| {
            let mut fs = self.settings.font_size;
            row(ui, pal, tr!("Schriftgröße im Editor" | "Editor font size"), tr!("Auch mit Strg + / Strg − änderbar" | "Also via Ctrl + / Ctrl −"), |ui| {
                if widgets::fancy_slider(ui, &mut fs, 10.0, 24.0, 0.5, "pt", pal).changed() {
                    self.settings.font_size = fs;
                    self.settings.save();
                }
            });
            divider(ui, pal);
            let mut dark = self.settings.dark_pdf;
            row(ui, pal, tr!("PDF abdunkeln" | "Dim PDF"), tr!("Seiten im dunklen Theme invertiert anzeigen" | "Show pages inverted in dark themes"), |ui| {
                if toggle(ui, &mut dark, pal).changed() {
                    self.settings.dark_pdf = dark;
                    self.settings.save();
                    self.thesis.viewer.dark_pages = dark && pal.dark;
                }
            });
        });
    }

    fn page_editor(&mut self, ui: &mut Ui, pal: &Palette) {
        heading(ui, pal, tr!("Eingabe" | "Input"));
        card(ui, pal, |ui| {
            let cur = self.settings.input_vim as usize;
            let mut pick = None;
            row(ui, pal, tr!("Tastaturbelegung" | "Keybindings"), tr!("Vim gilt im Code-Modus, visuell bleibt die Standardeingabe" | "Vim applies in code mode; visual mode keeps standard input"), |ui| {
                pick = segmented(ui, &[("", "Standard"), (widgets::VIM_LOGO, "Vim")], cur, pal, Id::new("seg-input"));
            });
            if let Some(i) = pick {
                self.settings.input_vim = i == 1;
                self.vim = Default::default();
                self.settings.save();
            }
        });
        heading(ui, pal, tr!("Kompilieren" | "Compiling"));
        card(ui, pal, |ui| {
            let mut auto = self.settings.auto_compile;
            row(ui, pal, tr!("Automatisch kompilieren" | "Compile automatically"), tr!("Nach kurzer Tipp-Pause – sonst nur mit Strg+S" | "After a short typing pause – otherwise only with Ctrl+S"), |ui| {
                if toggle(ui, &mut auto, pal).changed() {
                    self.settings.auto_compile = auto;
                    self.settings.save();
                }
            });
            divider(ui, pal);
            let mut pdf = self.settings.show_pdf;
            row(ui, pal, tr!("PDF-Vorschau neben dem Editor" | "PDF preview next to the editor"), tr!("In der Standardansicht der Masterarbeit" | "In the thesis standard view"), |ui| {
                if toggle(ui, &mut pdf, pal).changed() {
                    self.settings.show_pdf = pdf;
                    self.settings.save();
                }
            });
        });
        heading(ui, pal, "Git");
        card(ui, pal, |ui| {
            let cur = self.settings.diff_split as usize;
            let mut pick = None;
            row(ui, pal, tr!("Diff-Ansicht" | "Diff view"), tr!("Änderungen untereinander oder in zwei Spalten" | "Changes inline or in two columns"), |ui| {
                pick = segmented(ui, &[(ic::ALIGN_LEFT, "Inline"), (ic::COLUMNS, tr!("Nebeneinander" | "Side by side"))], cur, pal, Id::new("seg-diff"));
            });
            if let Some(i) = pick {
                self.settings.diff_split = i == 1;
                self.settings.save();
            }
        });
    }

    fn page_project(&mut self, ui: &mut Ui, pal: &Palette) {
        let engines = ["pdflatex", "xelatex", "lualatex"];
        let mut changed = false;
        heading(ui, pal, "Compiler");
        card(ui, pal, |ui| {
            for (label, desc, slides) in [(tr!("Masterarbeit" | "Thesis"), tr!("xelatex/lualatex für Systemschriften" | "xelatex/lualatex for system fonts"), false), (tr!("Präsentation" | "Presentation"), "", true)] {
                let cur_s = if slides { self.project.config.slides_engine.clone() } else { self.project.config.engine.clone() };
                let cur = engines.iter().position(|e| *e == cur_s).unwrap_or(0);
                let mut pick = None;
                row(ui, pal, label, desc, |ui| {
                    pick = segmented(ui, &[("", "pdfLaTeX"), ("", "XeLaTeX"), ("", "LuaLaTeX")], cur, pal, Id::new(("seg-engine", slides)));
                });
                if let Some(i) = pick {
                    let e = engines[i].to_string();
                    if slides {
                        self.project.config.slides_engine = e;
                    } else {
                        self.project.config.engine = e;
                    }
                    changed = true;
                }
                if !slides {
                    divider(ui, pal);
                }
            }
        });
        heading(ui, pal, tr!("Präsentation" | "Presentation"));
        card(ui, pal, |ui| {
            let mut m = self.project.config.talk_minutes;
            row(ui, pal, tr!("Redezeit" | "Talk length"), tr!("Der Timer beim Präsentieren wird gelb, dann rot" | "The presentation timer turns yellow, then red"), |ui| {
                let mut f = m as f32;
                if widgets::fancy_slider(ui, &mut f, 5.0, 60.0, 1.0, "min", pal).changed() {
                    m = f.round() as u32;
                }
            });
            if m != self.project.config.talk_minutes {
                self.project.config.talk_minutes = m;
                changed = true;
            }
        });
        if changed {
            let _ = self.project.save_config();
        }
    }

    fn page_general(&mut self, ui: &mut Ui, pal: &Palette, ctx: &egui::Context) {
        heading(ui, pal, tr!("Sprache" | "Language"));
        card(ui, pal, |ui| {
            let cur = self.settings.lang_en as usize;
            let mut pick = None;
            row(ui, pal, tr!("Sprache der Oberfläche" | "Interface language"), tr!("Wirkt sofort" | "Applies immediately"), |ui| {
                pick = segmented(ui, &[("DE", "Deutsch"), ("EN", "English")], cur, pal, Id::new("seg-lang"));
            });
            if let Some(i) = pick {
                self.settings.lang_en = i == 1;
                crate::i18n::set_english(self.settings.lang_en);
                self.settings.save();
            }
        });
        heading(ui, pal, "Updates");
        card(ui, pal, |ui| {
            let checking = self.updater.checking;
            let status = if self.updater.status.is_empty() { tr!("Installierte Version" | "Installed version").to_string() } else { self.updater.status.clone() };
            let mut check = false;
            let mut show = false;
            let available = self.updater.available.is_some();
            row(ui, pal, &format!("nEdit {}", crate::updater::VERSION), &status, |ui| {
                if available {
                    show = widgets::button(ui, ic::DOWNLOAD, tr!("Update anzeigen" | "Show update"), pal, widgets::BtnKind::Primary).clicked();
                } else {
                    check = ui.add_enabled_ui(!checking, |ui| widgets::button(ui, ic::REFRESH, tr!("Nach Updates suchen" | "Check for updates"), pal, widgets::BtnKind::Secondary)).inner.clicked();
                }
            });
            if check {
                self.updater.check(true, ctx);
            }
            if show {
                self.dialog = Some(Dialog::Update);
                self.settings_open = false;
            }
            divider(ui, pal);
            let mut on = self.settings.update_check;
            row(ui, pal, tr!("Beim Start prüfen" | "Check on start"), tr!("Neue Versionen von GitHub" | "New versions from GitHub"), |ui| {
                if toggle(ui, &mut on, pal).changed() {
                    self.settings.update_check = on;
                    self.settings.save();
                }
            });
        });
        heading(ui, pal, tr!("Hilfe" | "Help"));
        card(ui, pal, |ui| {
            let mut help = false;
            let mut about = false;
            row(ui, pal, tr!("Tastenkürzel & Funktionen" | "Shortcuts & features"), tr!("Alle Kürzel auf einen Blick (F1)" | "All shortcuts at a glance (F1)"), |ui| {
                help = widgets::button(ui, ic::BOOKMARK, tr!("Hilfe öffnen" | "Open help"), pal, widgets::BtnKind::Secondary).clicked();
            });
            divider(ui, pal);
            row(ui, pal, tr!("Über nEdit" | "About nEdit"), tr!("Wer und warum" | "Who and why"), |ui| {
                about = widgets::button(ui, ic::GRADUATION, tr!("Anzeigen" | "Show"), pal, widgets::BtnKind::Secondary).clicked();
            });
            if help {
                self.help_open = true;
                self.settings_open = false;
            }
            if about {
                self.about_open = true;
                self.settings_open = false;
            }
        });
    }
}

// ───────────────────────────── compact menus (brush / cog) ─────────────────────────────

/// Small theme tile for the quick menu.
fn theme_tile(ui: &mut Ui, pal: &Palette, t: &Palette, name: &str, selected: bool, w: f32) -> bool {
    let ph = w * 0.62;
    let (r, resp) = ui.allocate_exact_size(vec2(w, ph + 22.0), Sense::click());
    if !ui.is_rect_visible(r) {
        return resp.clicked();
    }
    let hover = ui.ctx().animate_bool(resp.id.with("h"), resp.hovered());
    let prev = Rect::from_min_size(r.min, vec2(w, ph));
    let p = ui.painter();
    theme_preview(p, prev, t);
    // round the bottom of the preview
    p.rect_stroke(prev, CornerRadius { nw: 10, ne: 10, sw: 6, se: 6 }, Stroke::new(1.0, with_alpha(pal.border, 120)), StrokeKind::Inside);
    if selected || hover > 0.0 {
        let s = if selected { Stroke::new(2.0, pal.accent) } else { Stroke::new(1.5, with_alpha(pal.text, (80.0 * hover) as u8)) };
        p.rect_stroke(prev.expand(2.5), 12.0, s, StrokeKind::Outside);
    }
    if selected {
        let c = pos2(prev.max.x - 10.0, prev.max.y - 10.0);
        p.circle_filled(c, 7.5, pal.accent);
        p.text(c, Align2::CENTER_CENTER, ic::CHECK, widgets::ui_font(8.0), pal.on_accent);
    }
    let g = p.layout_no_wrap(name.to_string(), widgets::ui_font(11.0), if selected { pal.bright } else { pal.subtext });
    let tx = (r.center().x - g.size().x / 2.0).max(r.min.x);
    p.with_clip_rect(r).galley(pos2(tx, prev.max.y + 5.0), g, pal.text);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(name).clicked()
}

/// Compact menu row: icon, label, optional hint on the right. Returns the response.
fn menu_row(ui: &mut Ui, pal: &Palette, icon: &str, label: &str, hint: &str) -> egui::Response {
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
    if hov > 0.0 {
        ui.painter().rect_filled(r, 8.0, with_alpha(pal.text, (14.0 * hov) as u8));
    }
    let ir = Rect::from_center_size(pos2(r.min.x + 17.0, r.center().y), vec2(24.0, 24.0));
    ui.painter().rect_filled(ir, 7.0, with_alpha(pal.text, 14));
    ui.painter().text(ir.center(), Align2::CENTER_CENTER, icon, widgets::ui_font(11.5), pal.subtext);
    ui.painter().text(pos2(r.min.x + 38.0, r.center().y), Align2::LEFT_CENTER, label, widgets::ui_font(13.0), pal.text);
    if !hint.is_empty() {
        ui.painter().text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, hint, widgets::ui_font(11.0), pal.dim);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Label on the left, control on the right – one compact line.
fn menu_line(ui: &mut Ui, pal: &Palette, label: &str, control: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, 36.0), egui::Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_height(36.0);
        ui.add_space(4.0);
        ui.label(egui::RichText::new(label).font(widgets::ui_font(13.0)).color(pal.text));
        ui.with_layout(egui::Layout::right_to_left(Align::Center), control);
    });
}

fn menu_section(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(text.to_uppercase()).font(widgets::ui_font(10.0)).color(pal.dim).extra_letter_spacing(1.2));
    });
    ui.add_space(2.0);
}

/// Footer button that opens the big settings window.
fn menu_footer(ui: &mut Ui, pal: &Palette, label: &str) -> bool {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, with_alpha(pal.border, 90));
    ui.add_space(6.0);
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
    ui.painter().rect_filled(r, 9.0, with_alpha(pal.accent, (26.0 + 30.0 * hov) as u8));
    ui.painter().text(pos2(r.min.x + 14.0, r.center().y), Align2::LEFT_CENTER, format!("{}  {label}", ic::COG), widgets::bold_font(12.5), mix(pal.accent, pal.bright, 0.3));
    ui.painter().text(pos2(r.max.x - 12.0, r.center().y), Align2::RIGHT_CENTER, ic::CHEVRON_RIGHT, widgets::ui_font(11.0), pal.accent);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// Styled frame for the quick menus.
pub fn menu_frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.surface)
        .stroke(Stroke::new(1.0, with_alpha(pal.border, 170)))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(12))
        .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: with_alpha(Color32::BLACK, 110) })
}

impl App {
    /// Brush button: quick theme picker.
    pub fn theme_menu(&mut self, ui: &mut Ui, pal: &Palette, ctx: &egui::Context) {
        let width = 318.0;
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Theme").font(widgets::display_font(18.0)).color(pal.bright));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(egui::RichText::new(self.pal.name.clone()).font(widgets::ui_font(11.5)).color(pal.dim));
            });
        });
        ui.add_space(4.0);
        let omarchy = theme::omarchy_installed();
        let follow = self.settings.theme.is_none();
        if omarchy {
            let cur = theme::current_omarchy_name().map(|n| theme::pretty_name(&n)).unwrap_or_default();
            let mut on = follow;
            menu_line(ui, pal, &format!("{}  {}", ic::MAGIC, tr!("Omarchy folgen" | "Follow Omarchy")), |ui| {
                if toggle(ui, &mut on, pal).changed() {
                    if on {
                        self.set_theme(None, ctx);
                    } else {
                        let name = theme::current_omarchy_name();
                        self.set_theme(name.or(Some("nedit".into())), ctx);
                    }
                }
                ui.label(egui::RichText::new(cur).font(widgets::ui_font(11.0)).color(pal.dim));
            });
            ui.add_space(4.0);
        }
        let mut list: Vec<(String, String)> = vec![("nedit".into(), "nEdit Ink".into())];
        list.extend(theme::all_theme_names().into_iter().map(|n| (n.clone(), theme::pretty_name(&n))));
        let cols = 3;
        let gap = 10.0;
        let tw = (width - 8.0 - gap * (cols as f32 - 1.0)) / cols as f32;
        let mut picked = None;
        egui::ScrollArea::vertical().max_height(330.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.add_space(4.0);
            for chunk in list.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.x = gap;
                    for (key, label) in chunk {
                        let p = if key == "nedit" { Some(Palette::default_dark()) } else { cached_palette(ctx, key) };
                        let Some(p) = p else { continue };
                        let selected = self.settings.theme.as_deref() == Some(key.as_str()) || (key == "nedit" && follow && !omarchy);
                        if theme_tile(ui, pal, &p, label, selected, tw) {
                            picked = Some(key.clone());
                        }
                    }
                });
                ui.add_space(8.0);
            }
        });
        if let Some(k) = picked {
            self.set_theme(Some(k), ctx);
        }
        if menu_footer(ui, pal, tr!("Alle Einstellungen …" | "All settings …")) {
            self.open_settings(Some(Page::Appearance), ctx);
            ui.close();
        }
    }

    /// Cog button: the most used settings at a glance.
    pub fn settings_menu(&mut self, ui: &mut Ui, pal: &Palette, ctx: &egui::Context) {
        ui.set_width(318.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tr!("Einstellungen" | "Settings")).font(widgets::display_font(18.0)).color(pal.bright));
        });
        menu_section(ui, pal, "Editor");
        let cur = self.settings.input_vim as usize;
        let mut pick = None;
        menu_line(ui, pal, tr!("Eingabe" | "Input"), |ui| {
            pick = segmented(ui, &[("", "Standard"), (widgets::VIM_LOGO, "Vim")], cur, pal, Id::new("menu-seg-input"));
        });
        if let Some(i) = pick {
            self.settings.input_vim = i == 1;
            self.vim = Default::default();
            self.settings.save();
        }
        let mut fs = self.settings.font_size;
        menu_line(ui, pal, tr!("Schrift" | "Font"), |ui| {
            if widgets::fancy_slider(ui, &mut fs, 10.0, 24.0, 0.5, "pt", pal).changed() {
                self.settings.font_size = fs;
                self.settings.save();
            }
        });
        let mut auto = self.settings.auto_compile;
        menu_line(ui, pal, tr!("Automatisch kompilieren" | "Compile automatically"), |ui| {
            if toggle(ui, &mut auto, pal).changed() {
                self.settings.auto_compile = auto;
                self.settings.save();
            }
        });
        let mut dark = self.settings.dark_pdf;
        menu_line(ui, pal, tr!("PDF abdunkeln" | "Dim PDF"), |ui| {
            if toggle(ui, &mut dark, pal).changed() {
                self.settings.dark_pdf = dark;
                self.settings.save();
                self.thesis.viewer.dark_pages = dark && pal.dark;
            }
        });
        menu_section(ui, pal, tr!("Sprache" | "Language"));
        let cur = self.settings.lang_en as usize;
        let mut pick = None;
        menu_line(ui, pal, tr!("Oberfläche" | "Interface"), |ui| {
            pick = segmented(ui, &[("", "Deutsch"), ("", "English")], cur, pal, Id::new("menu-seg-lang"));
        });
        if let Some(i) = pick {
            self.settings.lang_en = i == 1;
            crate::i18n::set_english(self.settings.lang_en);
            self.settings.save();
        }
        ui.add_space(4.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(r, 0.0, with_alpha(pal.border, 90));
        ui.add_space(4.0);
        if menu_row(ui, pal, ic::BOOKMARK, tr!("Hilfe & Tastenkürzel" | "Help & shortcuts"), "F1").clicked() {
            self.help_open = true;
            ui.close();
        }
        let upd_hint = if self.updater.available.is_some() { tr!("Update verfügbar" | "Update available").to_string() } else { format!("v{}", crate::updater::VERSION) };
        if menu_row(ui, pal, ic::REFRESH, tr!("Nach Updates suchen" | "Check for updates"), &upd_hint).clicked() {
            if self.updater.available.is_some() {
                self.dialog = Some(Dialog::Update);
            } else {
                self.updater.check(true, ctx);
            }
            ui.close();
        }
        if menu_row(ui, pal, ic::GRADUATION, tr!("Über nEdit" | "About nEdit"), "").clicked() {
            self.about_open = true;
            ui.close();
        }
        if menu_footer(ui, pal, tr!("Alle Einstellungen …" | "All settings …")) {
            self.open_settings(None, ctx);
            ui.close();
        }
    }
}
