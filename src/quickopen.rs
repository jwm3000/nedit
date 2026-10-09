//! Quick open (double Shift / Ctrl+P): one floating input to jump to files, headings,
//! literature and commands with fuzzy matching.

use crate::app::{App, SideMode, Tab};
use crate::icons as ic;
use crate::theme::{mix, with_alpha};
use crate::widgets;
use egui::text::{LayoutJob, TextFormat};
use egui::{pos2, vec2, Align2, Color32, Key, Modifiers, Rect, Sense, Stroke, StrokeKind};

#[derive(Default)]
pub struct QuickOpen {
    pub query: String,
    pub selected: usize,
    pub opened_at: f64,
    focus: bool,
}

/// Double-Shift detection state.
#[derive(Default)]
pub struct ShiftTap {
    prev_down: bool,
    last_tap: f64,
    down_since: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cat {
    Recent,
    File,
    Heading,
    Paper,
    Command,
    Line,
}

impl Cat {
    fn label(self) -> &'static str {
        match self {
            Cat::Recent => tr!("Zuletzt" | "Recent"),
            Cat::File => tr!("Datei" | "File"),
            Cat::Heading => tr!("Gliederung" | "Outline"),
            Cat::Paper => tr!("Literatur" | "Literature"),
            Cat::Command => tr!("Befehl" | "Command"),
            Cat::Line => tr!("Zeile" | "Line"),
        }
    }
}

#[derive(Clone)]
enum Action {
    OpenFile(String),
    Jump(String, usize, Tab),
    Cite(String),
    Line(usize),
    Cmd(Cmd),
}

#[derive(Clone)]
enum Cmd {
    Compile,
    Present,
    Tab(Tab),
    Visual(bool),
    DocMode,
    Focus,
    TogglePdf,
    Git,
    NewFile,
    Lang,
    Updates,
    About,
    Vim,
    Theme(Option<String>),
}

struct Item {
    cat: Cat,
    icon: &'static str,
    color: Color32,
    title: String,
    sub: String,
    action: Action,
    score: i32,
    hits: Vec<usize>,
}

/// Fuzzy subsequence match. Returns score and matched char positions in `text`.
pub fn fuzzy(q: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    if q.is_empty() {
        return Some((0, vec![]));
    }
    let t: Vec<char> = text.chars().collect();
    let tl: Vec<char> = text.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect();
    let ql: Vec<char> = q.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect();
    // exact substring: best case
    let hay: String = tl.iter().collect();
    let needle: String = ql.iter().collect();
    if let Some(bpos) = hay.find(&needle) {
        let start = hay[..bpos].chars().count();
        let word_start = start == 0 || !t[start - 1].is_alphanumeric();
        let score = 200 + if word_start { 60 } else { 0 } - start as i32 - (t.len() as i32 - ql.len() as i32) / 4;
        return Some((score, (start..start + ql.len()).collect()));
    }
    let mut hits = vec![];
    let mut score = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in tl.iter().enumerate() {
        if qi < ql.len() && c == ql[qi] {
            let word_start = i == 0 || !t[i - 1].is_alphanumeric() || (t[i].is_uppercase() && t[i - 1].is_lowercase());
            score += 10;
            if word_start {
                score += 12;
            }
            if let Some(l) = last {
                if l + 1 == i {
                    score += 8;
                } else {
                    score -= ((i - l) as i32).min(6);
                }
            } else {
                score -= (i as i32).min(10);
            }
            hits.push(i);
            last = Some(i);
            qi += 1;
        }
    }
    (qi == ql.len()).then_some((score - (t.len() as i32) / 6, hits))
}

impl App {
    pub fn open_quick(&mut self, now: f64) {
        self.quick = Some(QuickOpen { opened_at: now, focus: true, ..Default::default() });
    }

    /// Detect two short Shift taps (no other key in between) and Ctrl+P.
    pub fn quick_shortcut(&mut self, ctx: &egui::Context, now: f64) {
        let (down, other, ctrl_p) = ctx.input_mut(|i| {
            let other = i.events.iter().any(|e| matches!(e, egui::Event::Text(_) | egui::Event::Key { .. } | egui::Event::PointerButton { .. } | egui::Event::Paste(_)));
            (i.modifiers.shift && !i.modifiers.ctrl && !i.modifiers.alt && !i.modifiers.command, other, i.consume_key(Modifiers::COMMAND, Key::P))
        });
        if ctrl_p {
            self.open_quick(now);
            return;
        }
        let st = &mut self.shift_tap;
        if other {
            st.last_tap = -10.0;
            st.down_since = -10.0;
        }
        if down && !st.prev_down {
            st.down_since = now;
        }
        if !down && st.prev_down && !other {
            // a tap = short press
            if now - st.down_since < 0.35 {
                if now - st.last_tap < 0.45 {
                    st.last_tap = -10.0;
                    st.prev_down = down;
                    if self.quick.is_none() {
                        self.open_quick(now);
                    }
                    return;
                }
                st.last_tap = now;
            }
        }
        st.prev_down = down;
    }

    fn quick_items(&self, q: &str) -> Vec<Item> {
        let pal = &self.pal;
        let mut items: Vec<Item> = vec![];
        let (filter, q) = match q.chars().next() {
            Some('#') => (Some(Cat::Heading), q[1..].trim_start()),
            Some('@') => (Some(Cat::Paper), q[1..].trim_start()),
            Some('>') => (Some(Cat::Command), q[1..].trim_start()),
            _ => (None, q),
        };
        let want = |c: Cat| filter.is_none() || filter == Some(c) || (filter == Some(Cat::File) && c == Cat::Recent);

        // :123 → go to line in the current file
        if let Some(n) = q.strip_prefix(':').and_then(|n| n.trim().parse::<usize>().ok()) {
            let doc = if self.tab == Tab::Shelf { self.last_doc_tab } else { self.tab };
            if let Some(rel) = self.ws(doc).active.clone() {
                items.push(Item {
                    cat: Cat::Line,
                    icon: ic::ARROW_RIGHT,
                    color: pal.accent,
                    title: trf!("Zu Zeile {n} springen" | "Go to line {n}"),
                    sub: rel,
                    action: Action::Line(n),
                    score: 1000,
                    hits: vec![],
                });
            }
            return items;
        }

        // files (recent first when the query is empty)
        if want(Cat::File) {
            for f in &self.flat {
                let name = f.rsplit('/').next().unwrap_or(f).to_string();
                let dir = f.strip_suffix(&name).unwrap_or("").trim_end_matches('/').to_string();
                let recent_rank = self.recent.iter().position(|r| r == f);
                let (score, hits) = if q.is_empty() {
                    match recent_rank {
                        Some(r) => (500 - r as i32 * 10, vec![]),
                        None => continue,
                    }
                } else {
                    let by_name = fuzzy(q, &name).map(|(s, h)| (s + 25, h));
                    let by_path = fuzzy(q, f).map(|(s, _)| (s, vec![]));
                    match by_name.or(by_path) {
                        Some((s, h)) => (s + recent_rank.map_or(0, |r| 30 - (r as i32).min(30)), h),
                        None => continue,
                    }
                };
                let (icon, color) = crate::filetree::file_icon(&name, pal);
                items.push(Item {
                    cat: if recent_rank.is_some() && q.is_empty() { Cat::Recent } else { Cat::File },
                    icon,
                    color,
                    title: name,
                    sub: if dir.is_empty() { "/".into() } else { dir },
                    action: Action::OpenFile(f.clone()),
                    score,
                    hits,
                });
            }
        }
        // headings of thesis and slides
        if want(Cat::Heading) && (!q.is_empty() || filter == Some(Cat::Heading)) {
            for (list, tab) in [(&self.outline, Tab::Thesis), (&self.slide_outline, Tab::Slides)] {
                for it in list {
                    let Some((s, h)) = fuzzy(q, &it.title) else { continue };
                    let indent = match it.level {
                        0 | 1 => "",
                        2 => "  ",
                        _ => "    ",
                    };
                    items.push(Item {
                        cat: Cat::Heading,
                        icon: if tab == Tab::Slides { ic::TV } else { ic::HEADER },
                        color: mix(pal.magenta, pal.text, 0.2),
                        title: format!("{indent}{}", it.title),
                        sub: format!("{}:{}", it.file, it.line),
                        action: Action::Jump(it.file.clone(), it.line, tab),
                        score: s - 5 + if it.level <= 1 { 8 } else { 0 },
                        hits: h.iter().map(|x| x + indent.chars().count()).collect(),
                    });
                }
            }
        }
        // literature
        if want(Cat::Paper) && (!q.is_empty() || filter == Some(Cat::Paper)) {
            for p in &self.shelf.papers {
                let title = p.entry.title();
                let hay = format!("{} {} {}", p.entry.key, p.entry.authors_short(), title);
                let Some((s, _)) = fuzzy(q, &hay) else { continue };
                let hits = fuzzy(q, &title).map(|x| x.1).unwrap_or_default();
                items.push(Item {
                    cat: Cat::Paper,
                    icon: ic::BOOK,
                    color: pal.yellow,
                    title,
                    sub: format!("{} {}  ·  \\citep{{{}}}", p.entry.authors_short(), p.entry.year(), p.entry.key),
                    action: Action::Cite(p.entry.key.clone()),
                    score: s - 10,
                    hits,
                });
            }
        }
        // commands
        if want(Cat::Command) {
            let vis = self.settings.visual;
            let mut cmds: Vec<(&'static str, String, Cmd)> = vec![
                (ic::PLAY, tr!("Kompilieren" | "Compile").into(), Cmd::Compile),
                (ic::TV, tr!("Präsentieren" | "Present").into(), Cmd::Present),
                (ic::FILE_TEXT, tr!("Zur Masterarbeit" | "Go to thesis").into(), Cmd::Tab(Tab::Thesis)),
                (ic::TV, tr!("Zur Präsentation" | "Go to presentation").into(), Cmd::Tab(Tab::Slides)),
                (ic::BOOK, tr!("Bibliothek öffnen" | "Open library").into(), Cmd::Tab(Tab::Shelf)),
                (if vis { ic::CODE } else { ic::EYE }, if vis { tr!("Code-Ansicht" | "Code view") } else { tr!("Visuelle Ansicht" | "Visual view") }.into(), Cmd::Visual(!vis)),
                (ic::BOOK, tr!("Dokument-Modus umschalten" | "Toggle document mode").into(), Cmd::DocMode),
                (ic::EXPAND, tr!("Vollbild umschalten" | "Toggle full screen").into(), Cmd::Focus),
                (ic::FILE_PDF, tr!("PDF-Vorschau ein/aus" | "Toggle PDF preview").into(), Cmd::TogglePdf),
                (ic::GIT, tr!("Git-Panel öffnen" | "Open Git panel").into(), Cmd::Git),
                (ic::PLUS, tr!("Neue Datei" | "New file").into(), Cmd::NewFile),
                (ic::GLOBE, tr!("Sprache: English" | "Language: Deutsch").into(), Cmd::Lang),
                (ic::TERMINAL, if self.settings.input_vim { tr!("Vim-Modus ausschalten" | "Turn off vim mode") } else { tr!("Vim-Modus einschalten" | "Turn on vim mode") }.into(), Cmd::Vim),
                (ic::REFRESH, tr!("Nach Updates suchen" | "Check for updates").into(), Cmd::Updates),
                (ic::GRADUATION, tr!("Über nEdit" | "About nEdit").into(), Cmd::About),
                (ic::MAGIC, tr!("Theme: Omarchy folgen" | "Theme: follow Omarchy").into(), Cmd::Theme(None)),
            ];
            if !q.is_empty() {
                for (name, _) in crate::theme::list_themes() {
                    cmds.push((ic::BRUSH, format!("Theme: {}", crate::theme::pretty_name(&name)), Cmd::Theme(Some(name))));
                }
            }
            for (icon, title, cmd) in cmds {
                let (s, h) = if q.is_empty() {
                    if filter != Some(Cat::Command) && !matches!(cmd, Cmd::Compile | Cmd::Present | Cmd::Visual(_) | Cmd::DocMode | Cmd::Git) {
                        continue;
                    }
                    (100, vec![])
                } else {
                    match fuzzy(q, &title) {
                        Some(x) => x,
                        None => continue,
                    }
                };
                items.push(Item { cat: Cat::Command, icon, color: pal.accent, title, sub: String::new(), action: Action::Cmd(cmd), score: s - 15, hits: h });
            }
        }
        items.sort_by(|a, b| b.score.cmp(&a.score));
        items.truncate(60);
        items
    }

    fn quick_run(&mut self, action: Action, ctx: &egui::Context, now: f64) {
        let doc = if self.tab == Tab::Shelf { self.last_doc_tab } else { self.tab };
        match action {
            Action::OpenFile(f) => {
                let slides_dir = std::path::Path::new(&self.project.config.slides_main).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                let t = if !slides_dir.is_empty() && f.starts_with(&format!("{slides_dir}/")) { Tab::Slides } else { Tab::Thesis };
                if crate::project::is_text_file(&f) {
                    self.tab = t;
                    self.doc_mode = false;
                }
                self.open_file(&f, t);
            }
            Action::Jump(f, line, t) => {
                self.tab = t;
                self.jump_to(&f, line, t);
            }
            Action::Cite(key) => {
                if let Some(rel) = self.insert_snippet(doc, &format!("\\citep{{{key}}}$0")) {
                    self.tab = doc;
                    let g = self.pal.green;
                    self.toast(ic::QUOTE, trf!("\\citep{{{key}}} eingefügt in {rel}" | "Inserted \\citep{{{key}}} into {rel}"), g, now);
                }
            }
            Action::Line(n) => {
                if let Some(b) = self.active_buffer_mut(doc) {
                    b.goto_line(n);
                }
            }
            Action::Cmd(c) => match c {
                Cmd::Compile => self.compile(doc, ctx),
                Cmd::Present => {
                    self.tab = Tab::Slides;
                    crate::workspace::start_presentation(self, ctx, now);
                }
                Cmd::Tab(t) => self.tab = t,
                Cmd::Visual(v) => {
                    self.settings.visual = v;
                    self.settings.save();
                }
                Cmd::DocMode => {
                    self.tab = Tab::Thesis;
                    let on = !self.doc_mode;
                    self.set_doc_mode(on, ctx);
                }
                Cmd::Focus => {
                    self.tab = Tab::Thesis;
                    let on = !self.focus;
                    self.set_focus(on, ctx);
                }
                Cmd::TogglePdf => {
                    if self.doc_mode {
                        self.doc_pdf = !self.doc_pdf;
                    } else {
                        self.settings.show_pdf = !self.settings.show_pdf;
                        self.settings.save();
                    }
                }
                Cmd::Git => {
                    self.side = SideMode::Git;
                    if self.tab == Tab::Shelf {
                        self.tab = Tab::Thesis;
                    }
                }
                Cmd::NewFile => {
                    self.side = SideMode::Files;
                    self.begin_new(false, None);
                }
                Cmd::Lang => {
                    self.settings.lang_en = !self.settings.lang_en;
                    crate::i18n::set_english(self.settings.lang_en);
                    self.settings.save();
                }
                Cmd::Vim => {
                    self.settings.input_vim = !self.settings.input_vim;
                    self.vim = Default::default();
                    self.settings.save();
                }
                Cmd::Updates => self.updater.check(true, ctx),
                Cmd::About => self.about_open = true,
                Cmd::Theme(name) => self.set_theme(name, ctx),
            },
        }
    }

    pub fn quick_ui(&mut self, ctx: &egui::Context, now: f64) {
        let Some(mut qo) = self.quick.take() else { return };
        let pal = self.pal.clone();
        let items = self.quick_items(qo.query.trim());
        if qo.selected >= items.len() {
            qo.selected = items.len().saturating_sub(1);
        }
        // keyboard
        let (up, down, enter, esc, tab) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::CTRL, Key::K),
                i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::CTRL, Key::J),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.key_pressed(Key::Escape),
                i.consume_key(Modifiers::NONE, Key::Tab),
            )
        });
        if up && !items.is_empty() {
            qo.selected = (qo.selected + items.len() - 1) % items.len();
        }
        if (down || tab) && !items.is_empty() {
            qo.selected = (qo.selected + 1) % items.len();
        }
        let mut run: Option<Action> = None;
        let mut close = esc;
        if enter {
            if let Some(it) = items.get(qo.selected) {
                run = Some(it.action.clone());
            }
            close = true;
        }

        let screen = ctx.content_rect();
        let w = (screen.width() * 0.62).clamp(420.0, 720.0);
        let appear = ((now - qo.opened_at) as f32 / 0.14).clamp(0.0, 1.0);
        let id = egui::Id::new("quick-open");
        let modal = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).anchor(Align2::CENTER_TOP, vec2(0.0, 70.0 + 14.0 * (1.0 - appear))))
            .backdrop_color(with_alpha(Color32::BLACK, (90.0 * appear) as u8))
            .frame(
                egui::Frame::new()
                    .fill(pal.surface)
                    .stroke(Stroke::new(1.0, mix(pal.border, pal.accent, 0.35)))
                    .corner_radius(16)
                    .inner_margin(egui::Margin::same(0))
                    .shadow(egui::Shadow { offset: [0, 18], blur: 50, spread: 0, color: with_alpha(Color32::BLACK, 140) }),
            )
            .show(ctx, |ui| {
                ui.set_width(w);
                ui.set_opacity(appear);
                // input row
                let (row, _) = ui.allocate_exact_size(vec2(w, 58.0), Sense::hover());
                ui.painter().text(pos2(row.min.x + 26.0, row.center().y), Align2::CENTER_CENTER, ic::SEARCH, widgets::ui_font(17.0), pal.accent);
                let input_rect = Rect::from_min_max(pos2(row.min.x + 48.0, row.min.y + 8.0), pos2(row.max.x - 70.0, row.max.y - 8.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(input_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
                let r = child.add(
                    egui::TextEdit::singleline(&mut qo.query)
                        .id(egui::Id::new("quick-open-input"))
                        .frame(egui::Frame::NONE)
                        .font(widgets::ui_font(18.0))
                        .hint_text(egui::RichText::new(tr!("Datei, Kapitel, Quelle oder Befehl …" | "File, chapter, source or command …")).color(pal.dim))
                        .desired_width(input_rect.width()),
                );
                if qo.focus {
                    r.request_focus();
                    qo.focus = false;
                }
                if r.changed() {
                    qo.selected = 0;
                }
                if !r.has_focus() && !esc {
                    r.request_focus();
                }
                let kr = Rect::from_center_size(pos2(row.max.x - 36.0, row.center().y), vec2(38.0, 22.0));
                ui.painter().rect_stroke(kr, 6.0, Stroke::new(1.0, pal.border), StrokeKind::Inside);
                ui.painter().text(kr.center(), Align2::CENTER_CENTER, "Esc", widgets::ui_font(11.0), pal.dim);
                ui.painter().line_segment([pos2(row.min.x, row.max.y), pos2(row.max.x, row.max.y)], Stroke::new(1.0, with_alpha(pal.border, 160)));

                // results
                egui::ScrollArea::vertical().max_height((screen.height() * 0.55).min(460.0)).auto_shrink([false, true]).show(ui, |ui| {
                    ui.add_space(6.0);
                    if items.is_empty() {
                        ui.add_space(18.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new(tr!("Nichts gefunden" | "Nothing found")).font(widgets::ui_font(14.0)).color(pal.dim));
                        });
                        ui.add_space(18.0);
                    }
                    let mut last_cat: Option<Cat> = None;
                    let grouped = qo.query.trim().is_empty();
                    for (k, it) in items.iter().enumerate() {
                        if grouped && last_cat != Some(it.cat) {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.label(egui::RichText::new(it.cat.label().to_uppercase()).font(widgets::ui_font(10.0)).color(pal.dim).extra_letter_spacing(1.2));
                            });
                            last_cat = Some(it.cat);
                        }
                        let (rr, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
                        let sel = k == qo.selected;
                        let p = ui.painter();
                        let inner = rr.shrink2(vec2(8.0, 2.0));
                        if sel {
                            p.rect_filled(inner, 10.0, with_alpha(pal.accent, 34));
                            p.rect_filled(Rect::from_min_size(inner.min + vec2(0.0, 10.0), vec2(3.0, inner.height() - 20.0)), 2.0, pal.accent);
                            ui.scroll_to_rect(rr, None);
                        } else if resp.hovered() {
                            p.rect_filled(inner, 10.0, with_alpha(pal.text, 10));
                        }
                        // icon tile
                        let tile = Rect::from_center_size(pos2(inner.min.x + 24.0, inner.center().y), vec2(28.0, 28.0));
                        p.rect_filled(tile, 7.0, with_alpha(it.color, 30));
                        p.text(tile.center(), Align2::CENTER_CENTER, it.icon, widgets::ui_font(13.0), it.color);
                        // title with highlighted matches
                        let mut job = LayoutJob::default();
                        for (ci, ch) in it.title.chars().enumerate() {
                            let hit = it.hits.contains(&ci);
                            job.append(
                                &ch.to_string(),
                                0.0,
                                TextFormat {
                                    font_id: if hit { widgets::bold_font(14.0) } else { widgets::ui_font(14.0) },
                                    color: if hit { pal.accent } else if sel { pal.bright } else { pal.text },
                                    ..Default::default()
                                },
                            );
                        }
                        let tx = inner.min.x + 48.0;
                        let g = ui.painter().layout_job(job);
                        let title_y = if it.sub.is_empty() { inner.center().y - g.size().y / 2.0 } else { inner.min.y + 4.0 };
                        ui.painter().galley(pos2(tx, title_y), g, pal.text);
                        if !it.sub.is_empty() {
                            ui.painter().text(pos2(tx, inner.max.y - 11.0), Align2::LEFT_CENTER, crate::app::truncate(&it.sub, 80), widgets::ui_font(11.5), pal.dim);
                        }
                        // category pill
                        let cl = it.cat.label();
                        let cg = ui.painter().layout_no_wrap(cl.to_string(), widgets::ui_font(10.5), pal.subtext);
                        let pr = Rect::from_min_size(pos2(inner.max.x - cg.size().x - 22.0, inner.center().y - 10.0), vec2(cg.size().x + 14.0, 20.0));
                        ui.painter().rect_filled(pr, 10.0, if sel { with_alpha(pal.accent, 40) } else { with_alpha(pal.text, 12) });
                        ui.painter().galley(pos2(pr.min.x + 7.0, pr.center().y - cg.size().y / 2.0), cg, pal.subtext);
                        if resp.clicked() {
                            run = Some(it.action.clone());
                            close = true;
                        }
                        if resp.hovered() && ui.input(|i| i.pointer.delta().length() > 0.0) {
                            qo.selected = k;
                        }
                    }
                    ui.add_space(6.0);
                });
                // footer
                let (fr, _) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
                ui.painter().rect_filled(fr, egui::CornerRadius { nw: 0, ne: 0, sw: 16, se: 16 }, mix(pal.surface, pal.mantle, 0.6));
                ui.painter().line_segment([fr.left_top(), fr.right_top()], Stroke::new(1.0, with_alpha(pal.border, 120)));
                let hint = tr!(
                    "↑↓ wählen   ↵ öffnen   #  Gliederung   @  Literatur   >  Befehle   :42  Zeile"
                        | "↑↓ select   ↵ open   #  outline   @  literature   >  commands   :42  line"
                );
                ui.painter().text(pos2(fr.min.x + 18.0, fr.center().y), Align2::LEFT_CENTER, hint, widgets::ui_font(11.0), pal.dim);
            });
        if modal.should_close() {
            close = true;
        }
        if appear < 1.0 {
            ctx.request_repaint();
        }
        if let Some(a) = run {
            self.quick_run(a, ctx, now);
        }
        if close {
            // give the keyboard back to the editor
            let doc = if self.tab == Tab::Shelf { self.last_doc_tab } else { self.tab };
            if let Some(b) = self.active_buffer_mut(doc) {
                b.request_focus = true;
            }
        } else {
            self.quick = Some(qo);
        }
        let _ = pal;
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy;

    #[test]
    fn fuzzy_ranks() {
        assert!(fuzzy("einl", "einleitung.tex").is_some());
        assert!(fuzzy("eit", "einleitung.tex").is_some());
        assert!(fuzzy("xyz", "einleitung.tex").is_none());
        let a = fuzzy("met", "methodik.tex").unwrap().0;
        let b = fuzzy("met", "kapitel/ergebnisse_mit_tabellen.tex").unwrap().0;
        assert!(a > b);
        assert_eq!(fuzzy("mth", "methodik").unwrap().1, vec![0, 2, 3]);
    }
}
