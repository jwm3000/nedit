//! The "Bibliothek" tab: a shelf of paper cards, adding by DOI/arXiv/title/BibTeX/PDF,
//! a detail panel and a built-in PDF reader.

use crate::app::{truncate, App, Tab};
use crate::bib;
use crate::icons as ic;
use crate::shelf::{self, Paper, ReadStatus};
use crate::theme::{mix, with_alpha, Palette};
use crate::widgets::{self, BtnKind};
use egui::{pos2, vec2, Align2, Color32, Frame, Margin, Rect, Sense, Stroke, StrokeKind, Ui};

fn kind_label(p: &Paper) -> &'static str {
    match p.entry.kind.as_str() {
        "article" => "Artikel",
        "inproceedings" | "conference" | "proceedings" => "Konferenz",
        "book" => "Buch",
        "incollection" | "inbook" => "Kapitel",
        "phdthesis" => "Dissertation",
        "mastersthesis" | "thesis" => "Abschlussarbeit",
        "techreport" | "report" => "Bericht",
        "online" | "www" => "Web",
        _ if p.entry.get("eprint").is_some() || p.entry.get("archiveprefix").is_some() => "Preprint",
        _ => "Sonstiges",
    }
}

fn status_color(s: ReadStatus, pal: &Palette) -> Color32 {
    match s {
        ReadStatus::Unread => pal.dim,
        ReadStatus::Reading => pal.yellow,
        ReadStatus::Read => pal.green,
    }
}

pub fn shelf_ui(app: &mut App, ui: &mut Ui, now: f64) {
    let pal = app.pal.clone();
    let ctx = ui.ctx().clone();

    // ── reader mode ──
    if let Some(rel) = app.shelf_ui.reading.clone() {
        egui::CentralPanel::default().frame(Frame::new().fill(pal.crust)).show(ui, |ui| {
            let bar = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover()).0;
            ui.painter().rect_filled(bar, 0.0, pal.mantle);
            ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 150)));
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(14.0, 6.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            if widgets::button(&mut c, ic::ARROW_LEFT, "Bibliothek", &pal, BtnKind::Ghost).clicked() {
                app.shelf_ui.reading = None;
            }
            let title = app
                .shelf
                .papers
                .iter()
                .find(|p| p.meta.pdf.as_deref() == Some(rel.as_str()))
                .map(|p| format!("{} — {}", p.entry.authors_short(), p.entry.title()))
                .unwrap_or(rel.clone());
            c.label(egui::RichText::new(truncate(&title, 90)).font(widgets::bold_font(13.5)).color(pal.bright));
            c.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button_sized(ui, ic::EXTERNAL, "Extern öffnen", &pal, false, 28.0).clicked() {
                    crate::platform::open_external(app.project.root.join(&rel));
                }
                if widgets::icon_button_sized(ui, ic::ZOOM_IN, "Vergrößern", &pal, false, 28.0).clicked() {
                    app.reader.zoom_by(1.15);
                }
                if widgets::icon_button_sized(ui, ic::ZOOM_OUT, "Verkleinern", &pal, false, 28.0).clicked() {
                    app.reader.zoom_by(1.0 / 1.15);
                }
                if widgets::icon_button_sized(ui, ic::COLUMNS, "Breite", &pal, false, 28.0).clicked() {
                    app.reader.zoom = crate::pdfview::Zoom::FitWidth;
                }
                let n = app.reader.page_count();
                ui.label(egui::RichText::new(format!("{} / {n}", app.reader.current_page + 1)).color(pal.subtext));
            });
            let renderer = app.renderer.clone();
            app.reader.show(ui, &renderer, &pal);
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.shelf_ui.reading = None;
        }
        return;
    }

    // ── detail panel ──
    if let Some(key) = app.shelf_ui.selected.clone() {
        if app.shelf.papers.iter().any(|p| p.entry.key == key) {
            egui::Panel::right("paper-detail")
                .resizable(true)
                .default_size(420.0)
                .size_range(340.0..=640.0)
                .frame(Frame::new().fill(pal.mantle).inner_margin(Margin::same(22)))
                .show(ui, |ui| detail_panel(app, ui, &pal, &key, now));
        } else {
            app.shelf_ui.selected = None;
        }
    }

    egui::CentralPanel::default().frame(Frame::new().fill(pal.base).inner_margin(Margin { left: 34, right: 28, top: 26, bottom: 10 })).show(ui, |ui| {
        // ── header ──
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Bibliothek").font(widgets::display_font(34.0)).color(pal.bright));
                let n = app.shelf.papers.len();
                let read = app.shelf.papers.iter().filter(|p| p.meta.status == ReadStatus::Read).count();
                let pdfs = app.shelf.papers.iter().filter(|p| p.meta.pdf.is_some()).count();
                ui.label(egui::RichText::new(format!("{n} Quellen  ·  {read} gelesen  ·  {pdfs} PDFs  ·  references.bib")).font(widgets::ui_font(12.5)).color(pal.dim));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::button(ui, ic::CODE, "references.bib", &pal, BtnKind::Ghost).on_hover_text("Im Editor öffnen").clicked() {
                    app.open_file("references.bib", Tab::Thesis);
                    app.tab = Tab::Thesis;
                }
                if widgets::button(ui, ic::UPLOAD, "PDF importieren", &pal, BtnKind::Secondary).clicked() {
                    let tx = app.shelf_tx.clone();
                    let c = ctx.clone();
                    app.shelf_ui.busy = true;
                    app.shelf_ui.status = "Dateiauswahl …".into();
                    std::thread::spawn(move || {
                        let files = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_title("Paper importieren").pick_files();
                        match files {
                            Some(f) if !f.is_empty() => shelf::import_pdfs(f, tx, c),
                            _ => {
                                let _ = tx.send(shelf::ShelfMsg::Error("Keine Datei gewählt".into()));
                                c.request_repaint();
                            }
                        }
                    });
                }
            });
        });
        ui.add_space(18.0);

        // ── add bar ──
        let w = (ui.available_width() - 150.0).min(760.0);
        let mut submit = false;
        ui.horizontal(|ui| {
            let mut input = std::mem::take(&mut app.shelf_ui.input);
            let r = widgets::search_field(ui, &mut input, "DOI, arXiv-ID, Titel suchen oder BibTeX einfügen …", ic::PLUS, &pal, w, "shelf-add");
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            app.shelf_ui.input = input;
            if widgets::button(ui, ic::SEARCH, "Hinzufügen", &pal, BtnKind::Primary).clicked() {
                submit = true;
            }
        });
        if submit && !app.shelf_ui.input.trim().is_empty() {
            let input = app.shelf_ui.input.trim().to_string();
            if input.starts_with('@') {
                let parsed = bib::parse(&input);
                let n = parsed.entries.len();
                let mut last = None;
                for e in parsed.entries {
                    last = Some(app.shelf.add(e, None));
                }
                if n > 0 {
                    app.shelf_ui.input.clear();
                    app.shelf_ui.selected = last;
                    let g = pal.green;
                    app.toast(ic::BOOK, format!("{n} Eintrag/Einträge übernommen"), g, now);
                    app.after_shelf_change();
                } else {
                    app.shelf_ui.status = "BibTeX konnte nicht gelesen werden".into();
                }
            } else {
                app.shelf_ui.busy = true;
                app.shelf_ui.results.clear();
                shelf::run_query(input, app.shelf_tx.clone(), ctx.clone());
            }
        }
        // status
        if app.shelf_ui.busy || !app.shelf_ui.status.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if app.shelf_ui.busy {
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    widgets::draw_spinner(ui, r.center(), 6.0, pal.accent);
                }
                ui.label(egui::RichText::new(&app.shelf_ui.status).font(widgets::ui_font(12.5)).color(pal.subtext));
            });
        }
        // search results
        if !app.shelf_ui.results.is_empty() {
            ui.add_space(10.0);
            let mut add: Option<String> = None;
            Frame::new().fill(pal.surface).stroke(Stroke::new(1.0, pal.border)).corner_radius(12).inner_margin(Margin::same(8)).show(ui, |ui| {
                ui.set_width(w + 140.0);
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("TREFFER BEI CROSSREF").font(widgets::ui_font(10.5)).color(pal.dim).extra_letter_spacing(1.2));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button_sized(ui, ic::TIMES, "Schließen", &pal, false, 22.0).clicked() {
                            app.shelf_ui.results.clear();
                        }
                    });
                });
                for h in app.shelf_ui.results.clone() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 8.0, pal.overlay);
                    }
                    let p = ui.painter();
                    p.text(pos2(rect.min.x + 12.0, rect.min.y + 16.0), Align2::LEFT_CENTER, truncate(&h.title, 95), widgets::bold_font(13.0), pal.bright);
                    p.text(pos2(rect.min.x + 12.0, rect.min.y + 35.0), Align2::LEFT_CENTER, format!("{}  ·  {}  ·  {}", h.authors, h.year, truncate(&h.venue, 60)), widgets::ui_font(11.5), pal.subtext);
                    p.text(pos2(rect.max.x - 14.0, rect.center().y), Align2::RIGHT_CENTER, format!("{}  Hinzufügen", ic::PLUS), widgets::ui_font(12.0), if resp.hovered() { pal.accent } else { pal.dim });
                    if resp.clicked() {
                        add = Some(h.doi.clone());
                    }
                }
            });
            if let Some(doi) = add {
                app.shelf_ui.busy = true;
                app.shelf_ui.results.clear();
                shelf::run_query(doi, app.shelf_tx.clone(), ctx.clone());
            }
        }

        ui.add_space(20.0);
        // ── filters ──
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let sf = app.shelf_ui.status_filter;
            if widgets::chip(ui, "", "Alle", sf.is_none() && !app.shelf_ui.fav_only, pal.accent, &pal).clicked() {
                app.shelf_ui.status_filter = None;
                app.shelf_ui.fav_only = false;
                app.shelf_ui.tag_filter = None;
            }
            for s in [ReadStatus::Unread, ReadStatus::Reading, ReadStatus::Read] {
                if widgets::chip(ui, ic::CIRCLE, s.label(), sf == Some(s), status_color(s, &pal), &pal).clicked() {
                    app.shelf_ui.status_filter = if sf == Some(s) { None } else { Some(s) };
                }
            }
            if widgets::chip(ui, ic::STAR, "Favoriten", app.shelf_ui.fav_only, pal.yellow, &pal).clicked() {
                app.shelf_ui.fav_only = !app.shelf_ui.fav_only;
            }
            for t in app.shelf.all_tags() {
                let sel = app.shelf_ui.tag_filter.as_deref() == Some(t.as_str());
                if widgets::chip(ui, ic::TAG, &t, sel, pal.magenta, &pal).clicked() {
                    app.shelf_ui.tag_filter = if sel { None } else { Some(t.clone()) };
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut f = std::mem::take(&mut app.shelf_ui.filter);
                widgets::search_field(ui, &mut f, "Bibliothek durchsuchen", ic::SEARCH, &pal, 240.0, "shelf-filter");
                app.shelf_ui.filter = f;
            });
        });
        ui.add_space(14.0);

        // ── grid ──
        let q = app.shelf_ui.filter.to_lowercase();
        let visible: Vec<usize> = app
            .shelf
            .papers
            .iter()
            .enumerate()
            .filter(|(_, p)| app.shelf_ui.status_filter.is_none_or(|s| p.meta.status == s))
            .filter(|(_, p)| !app.shelf_ui.fav_only || p.meta.favorite)
            .filter(|(_, p)| app.shelf_ui.tag_filter.as_ref().is_none_or(|t| p.meta.tags.contains(t)))
            .filter(|(_, p)| {
                q.is_empty() || format!("{} {} {} {} {}", p.entry.key, p.entry.title(), p.entry.authors_full(), p.entry.venue(), p.meta.tags.join(" ")).to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        if app.shelf.papers.is_empty() {
            empty_state(ui, &pal);
            return;
        }
        let mut action: Option<CardAction> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let gap = 16.0;
            let avail = ui.available_width() - 4.0;
            let cols = ((avail + gap) / (280.0 + gap)).floor().max(1.0) as usize;
            let cw = (avail - gap * (cols as f32 - 1.0)) / cols as f32;
            for row in visible.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for &i in row {
                        if let Some(a) = paper_card(ui, &app.shelf.papers[i], cw, app.shelf_ui.selected.as_deref() == Some(app.shelf.papers[i].entry.key.as_str()), &pal) {
                            action = Some(a);
                        }
                    }
                });
                ui.add_space(gap);
            }
            if visible.is_empty() {
                ui.add_space(30.0);
                ui.label(egui::RichText::new("Keine Quelle passt zu den Filtern.").color(pal.dim));
            }
        });
        if let Some(a) = action {
            handle_card_action(app, a, now);
        }
    });
}

enum CardAction {
    Select(String),
    Open(String),
    Cite(String),
    Delete(String),
    Status(String, ReadStatus),
}

fn handle_card_action(app: &mut App, a: CardAction, now: f64) {
    match a {
        CardAction::Select(k) => {
            app.shelf_ui.tags_edit.clear();
            app.shelf_ui.bib_edit = None;
            app.shelf_ui.confirm_delete = None;
            app.shelf_ui.selected = if app.shelf_ui.selected.as_deref() == Some(k.as_str()) { None } else { Some(k) };
        }
        CardAction::Open(k) => {
            let pdf = app.shelf.papers.iter().find(|p| p.entry.key == k).and_then(|p| p.meta.pdf.clone());
            match pdf {
                Some(rel) => {
                    app.reader.load(&app.project.root.join(&rel));
                    app.shelf_ui.reading = Some(rel);
                    if let Some(i) = app.shelf.papers.iter().position(|p| p.entry.key == k) {
                        if app.shelf.papers[i].meta.status == ReadStatus::Unread {
                            app.shelf.papers[i].meta.status = ReadStatus::Reading;
                            let _ = app.shelf.save();
                        }
                    }
                }
                None => app.shelf_ui.selected = Some(k),
            }
        }
        CardAction::Cite(k) => cite(app, &k, now),
        CardAction::Delete(k) => {
            if let Some(i) = app.shelf.papers.iter().position(|p| p.entry.key == k) {
                app.shelf.remove(i);
                app.after_shelf_change();
            }
        }
        CardAction::Status(k, s) => {
            if let Some(i) = app.shelf.papers.iter().position(|p| p.entry.key == k) {
                app.shelf.papers[i].meta.status = s;
                let _ = app.shelf.save();
            }
        }
    }
}

fn cite(app: &mut App, key: &str, now: f64) {
    let t = app.last_doc_tab;
    let cmd = format!("\\citep{{{key}}}");
    match app.insert_snippet(t, &format!("{cmd}$0")) {
        Some(rel) => {
            let g = app.pal.green;
            app.toast(ic::QUOTE, format!("{cmd} eingefügt in {rel}"), g, now);
        }
        None => {
            app.shelf_ui.status = "Keine Datei geöffnet".into();
        }
    }
}

fn empty_state(ui: &mut Ui, pal: &Palette) {
    ui.add_space(60.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(ic::BOOK).font(widgets::ui_font(46.0)).color(with_alpha(pal.accent, 160)));
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Dein Regal ist noch leer").font(widgets::display_font(22.0)).color(pal.bright));
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Füge oben eine DOI oder arXiv-ID ein, suche nach einem Titel,\noder zieh PDFs einfach ins Fenster.").color(pal.dim));
    });
}

fn paper_card(ui: &mut Ui, p: &Paper, w: f32, selected: bool, pal: &Palette) -> Option<CardAction> {
    let h = 176.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let key = p.entry.key.clone();
    let hov = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.15);
    let lift = hov * 3.0;
    let r = rect.translate(vec2(0.0, -lift));
    let painter = ui.painter();
    // shadow
    for k in 1..=3 {
        let k = k as f32;
        painter.rect_filled(r.translate(vec2(0.0, 2.0 + k * (1.0 + hov))).expand(k), 12.0 + k, with_alpha(Color32::BLACK, ((10.0 + hov * 10.0) / k) as u8));
    }
    painter.rect_filled(r, 12.0, mix(pal.surface, pal.overlay, hov * 0.5));
    let spine = widgets::color_for(&key, pal);
    // spine (book metaphor)
    painter.rect_filled(Rect::from_min_size(r.min, vec2(7.0, r.height())), egui::CornerRadius { nw: 12, sw: 12, ne: 0, se: 0 }, spine);
    painter.rect_filled(Rect::from_min_size(r.min + vec2(7.0, 0.0), vec2(1.0, r.height())), 0.0, with_alpha(Color32::BLACK, 40));
    painter.rect_stroke(r, 12.0, Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { pal.accent } else { with_alpha(pal.border, 170) }), StrokeKind::Inside);

    let x0 = r.min.x + 22.0;
    let inner_w = r.max.x - x0 - 16.0;
    // top row: type + icons
    let kl = kind_label(p).to_uppercase();
    painter.text(pos2(x0, r.min.y + 20.0), Align2::LEFT_CENTER, &kl, widgets::ui_font(10.0), mix(spine, pal.text, 0.25));
    let mut ix = r.max.x - 18.0;
    if p.meta.pdf.is_some() {
        painter.text(pos2(ix, r.min.y + 20.0), Align2::CENTER_CENTER, ic::FILE_PDF, widgets::ui_font(12.0), pal.red);
        ix -= 20.0;
    }
    if p.meta.favorite {
        painter.text(pos2(ix, r.min.y + 20.0), Align2::CENTER_CENTER, ic::STAR, widgets::ui_font(12.0), pal.yellow);
    }
    // title (serif, up to 3 lines)
    let mut job = egui::text::LayoutJob::simple(p.entry.title(), widgets::display_font(16.5), pal.bright, inner_w);
    job.wrap.max_rows = 3;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let g = ui.painter().layout_job(job);
    let th = g.size().y;
    ui.painter().galley(pos2(x0, r.min.y + 34.0), g, pal.bright);
    let y = r.min.y + 40.0 + th;
    let p2 = ui.painter();
    let auth = format!("{}  ·  {}", p.entry.authors_short(), p.entry.year());
    p2.text(pos2(x0, y + 6.0), Align2::LEFT_CENTER, truncate(&auth, (inner_w / 6.6) as usize), widgets::ui_font(12.5), pal.subtext);
    let venue = p.entry.venue();
    if !venue.is_empty() {
        p2.text(pos2(x0, y + 24.0), Align2::LEFT_CENTER, truncate(&venue, (inner_w / 6.2) as usize), widgets::ui_font(11.5), pal.dim);
    }
    // bottom row
    let by = r.max.y - 20.0;
    let sc = status_color(p.meta.status, pal);
    p2.circle_filled(pos2(x0 + 4.0, by), 4.0, sc);
    p2.text(pos2(x0 + 14.0, by), Align2::LEFT_CENTER, p.meta.status.label(), widgets::ui_font(11.5), pal.subtext);
    p2.text(pos2(r.max.x - 16.0, by), Align2::RIGHT_CENTER, truncate(&key, 26), widgets::mono_font(11.0), pal.dim);
    p2.line_segment([pos2(x0, by - 16.0), pos2(r.max.x - 16.0, by - 16.0)], Stroke::new(1.0, with_alpha(pal.border, 110)));

    let mut act = None;
    if resp.double_clicked() {
        act = Some(CardAction::Open(key.clone()));
    } else if resp.clicked() {
        act = Some(CardAction::Select(key.clone()));
    }
    resp.context_menu(|ui| {
        if ui.button(format!("{}  Zitieren", ic::QUOTE)).clicked() {
            act = Some(CardAction::Cite(key.clone()));
            ui.close();
        }
        if ui.button(format!("{}  \\citep{{…}} kopieren", ic::COPY)).clicked() {
            ui.ctx().copy_text(format!("\\citep{{{key}}}"));
            ui.close();
        }
        if p.meta.pdf.is_some() && ui.button(format!("{}  PDF lesen", ic::FILE_PDF)).clicked() {
            act = Some(CardAction::Open(key.clone()));
            ui.close();
        }
        ui.separator();
        for s in [ReadStatus::Unread, ReadStatus::Reading, ReadStatus::Read] {
            if ui.button(format!("{}  {}", ic::CIRCLE, s.label())).clicked() {
                act = Some(CardAction::Status(key.clone(), s));
                ui.close();
            }
        }
        ui.separator();
        if ui.button(egui::RichText::new(format!("{}  Entfernen", ic::TRASH)).color(pal.red)).clicked() {
            act = Some(CardAction::Delete(key.clone()));
            ui.close();
        }
    });
    act
}

fn detail_panel(app: &mut App, ui: &mut Ui, pal: &Palette, key: &str, now: f64) {
    let Some(idx) = app.shelf.papers.iter().position(|p| p.entry.key == key) else { return };
    let p = app.shelf.papers[idx].clone();
    let spine = widgets::color_for(key, pal);
    let mut dirty = false;
    let mut action: Option<CardAction> = None;

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::badge(ui, &kind_label(&p).to_uppercase(), spine);
            if !p.entry.year().is_empty() {
                widgets::badge(ui, &p.entry.year(), pal.subtext);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button_sized(ui, ic::TIMES, "Schließen", pal, false, 26.0).clicked() {
                    app.shelf_ui.selected = None;
                }
                let fav = p.meta.favorite;
                if widgets::icon_button_sized(ui, if fav { ic::STAR } else { ic::STAR_O }, "Favorit", pal, fav, 26.0).clicked() {
                    app.shelf.papers[idx].meta.favorite = !fav;
                    dirty = true;
                }
            });
        });
        ui.add_space(10.0);
        ui.label(egui::RichText::new(p.entry.title()).font(widgets::display_font(23.0)).color(pal.bright));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(p.entry.authors_full()).font(widgets::ui_font(13.0)).color(pal.subtext));
        let venue = p.entry.venue();
        if !venue.is_empty() {
            ui.label(egui::RichText::new(venue).font(widgets::ui_font(12.5)).italics().color(pal.dim));
        }
        ui.add_space(14.0);

        ui.horizontal_wrapped(|ui| {
            if widgets::button(ui, ic::QUOTE, "Zitieren", pal, BtnKind::Primary).on_hover_text(format!("\\citep{{{key}}} an der Cursorposition einfügen")).clicked() {
                action = Some(CardAction::Cite(key.to_string()));
            }
            if p.meta.pdf.is_some() {
                if widgets::button(ui, ic::FILE_PDF, "PDF lesen", pal, BtnKind::Secondary).clicked() {
                    action = Some(CardAction::Open(key.to_string()));
                }
            } else if widgets::button(ui, ic::PAPERCLIP, "PDF anhängen", pal, BtnKind::Secondary).clicked() {
                let tx = app.shelf_tx.clone();
                let c = ui.ctx().clone();
                let k = key.to_string();
                std::thread::spawn(move || {
                    if let Some(f) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                        let _ = tx.send(shelf::ShelfMsg::Attach { key: k, path: f });
                        c.request_repaint();
                    }
                });
            }
            if let Some(url) = p.entry.url() {
                if widgets::icon_button(ui, ic::GLOBE, &url, pal, false).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
        });
        ui.add_space(14.0);

        // key
        widgets::section_label(ui, "Zitierschlüssel", pal);
        ui.horizontal(|ui| {
            Frame::new().fill(pal.base).corner_radius(7).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.label(egui::RichText::new(key).font(widgets::mono_font(13.0)).color(pal.bright));
            });
            if widgets::icon_button_sized(ui, ic::COPY, "\\citep{…} kopieren", pal, false, 28.0).clicked() {
                ui.ctx().copy_text(format!("\\citep{{{key}}}"));
                let g = pal.green;
                app.toast(ic::COPY, "In die Zwischenablage kopiert", g, now);
            }
        });
        ui.add_space(12.0);

        // status
        widgets::section_label(ui, "Lesestatus", pal);
        ui.horizontal(|ui| {
            for s in [ReadStatus::Unread, ReadStatus::Reading, ReadStatus::Read] {
                if widgets::chip(ui, ic::CIRCLE, s.label(), p.meta.status == s, status_color(s, pal), pal).clicked() {
                    app.shelf.papers[idx].meta.status = s;
                    dirty = true;
                }
            }
        });
        ui.add_space(12.0);

        // tags
        widgets::section_label(ui, "Schlagwörter", pal);
        ui.horizontal_wrapped(|ui| {
            let mut remove = None;
            for (ti, t) in p.meta.tags.iter().enumerate() {
                if widgets::chip(ui, ic::TAG, &format!("{t}  {}", ic::TIMES), true, pal.magenta, pal).on_hover_text("Entfernen").clicked() {
                    remove = Some(ti);
                }
            }
            if let Some(ti) = remove {
                app.shelf.papers[idx].meta.tags.remove(ti);
                dirty = true;
            }
            let r = ui.add(egui::TextEdit::singleline(&mut app.shelf_ui.tags_edit).hint_text("+ Schlagwort").desired_width(120.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let t = app.shelf_ui.tags_edit.trim().to_string();
                if !t.is_empty() && !app.shelf.papers[idx].meta.tags.contains(&t) {
                    app.shelf.papers[idx].meta.tags.push(t);
                    dirty = true;
                }
                app.shelf_ui.tags_edit.clear();
                r.request_focus();
            }
        });
        ui.add_space(12.0);

        // notes
        widgets::section_label(ui, "Notizen", pal);
        let r = ui.add(
            egui::TextEdit::multiline(&mut app.shelf.papers[idx].meta.notes)
                .hint_text("Kernaussagen, Zitate, Seitenzahlen …")
                .desired_rows(6)
                .desired_width(f32::INFINITY),
        );
        if r.lost_focus() || (r.changed() && ui.input(|i| i.modifiers.command)) {
            dirty = true;
        }
        ui.add_space(12.0);

        // abstract if present
        if let Some(abs) = p.entry.get("abstract") {
            widgets::section_label(ui, "Abstract", pal);
            ui.label(egui::RichText::new(bib::clean(abs)).font(widgets::ui_font(12.5)).color(pal.subtext));
            ui.add_space(12.0);
        }

        // bibtex
        widgets::section_label(ui, "BibTeX", pal);
        match &mut app.shelf_ui.bib_edit {
            None => {
                Frame::new().fill(pal.base).corner_radius(8).inner_margin(Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new(p.entry.to_bibtex()).font(widgets::mono_font(11.5)).color(pal.subtext));
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, ic::PENCIL, "Bearbeiten", pal, BtnKind::Ghost).clicked() {
                        app.shelf_ui.bib_edit = Some(p.entry.to_bibtex());
                    }
                    if widgets::button(ui, ic::COPY, "Kopieren", pal, BtnKind::Ghost).clicked() {
                        ui.ctx().copy_text(p.entry.to_bibtex());
                    }
                });
            }
            Some(src) => {
                ui.add(egui::TextEdit::multiline(src).font(widgets::mono_font(12.0)).desired_rows(10).desired_width(f32::INFINITY));
                ui.add_space(6.0);
                let src_c = src.clone();
                ui.horizontal(|ui| {
                    if widgets::button(ui, ic::CHECK, "Übernehmen", pal, BtnKind::Primary).clicked() {
                        if let Some(mut e) = bib::parse(&src_c).entries.into_iter().next() {
                            if e.key.is_empty() {
                                e.key = key.to_string();
                            }
                            if e.key != key && app.shelf.papers.iter().any(|q| q.entry.key == e.key) {
                                e.key = app.shelf.unique_key(&e.key);
                            }
                            let new_key = e.key.clone();
                            app.shelf.papers[idx].entry = e;
                            app.shelf_ui.selected = Some(new_key);
                            app.shelf_ui.bib_edit = None;
                            dirty = true;
                        }
                    }
                    if widgets::button(ui, "", "Abbrechen", pal, BtnKind::Ghost).clicked() {
                        app.shelf_ui.bib_edit = None;
                    }
                });
            }
        }
        ui.add_space(22.0);
        if app.shelf_ui.confirm_delete.as_deref() == Some(key) {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Wirklich entfernen?").color(pal.red));
                if widgets::button(ui, ic::TRASH, "Ja, entfernen", pal, BtnKind::Danger).clicked() {
                    action = Some(CardAction::Delete(key.to_string()));
                    app.shelf_ui.confirm_delete = None;
                }
                if widgets::button(ui, "", "Nein", pal, BtnKind::Ghost).clicked() {
                    app.shelf_ui.confirm_delete = None;
                }
            });
        } else if widgets::button(ui, ic::TRASH, "Aus Bibliothek entfernen", pal, BtnKind::Danger).clicked() {
            app.shelf_ui.confirm_delete = Some(key.to_string());
        }
        ui.add_space(20.0);
    });
    if dirty {
        let _ = app.shelf.save();
        app.after_shelf_change();
    }
    if let Some(a) = action {
        handle_card_action(app, a, now);
    }
}
