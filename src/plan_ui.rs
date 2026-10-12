//! "Plan" view in the library tab: deadline countdown, progress, smart quick capture,
//! grouped tasks, TODOs from the LaTeX sources and colorful notes.

use crate::app::{App, Tab};
use crate::icons as ic;
use crate::plan::{self, Group, Plan, SourceTodo};
use crate::theme::{mix, with_alpha, Palette};
use crate::widgets;
use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};

#[derive(Default)]
pub struct PlanUi {
    input: String,
    note_mode: bool,
    tag_filter: Option<String>,
    show_source: bool,
    editing: Option<(u64, String)>,
    deadline_edit: Option<String>,
    goal_edit: Option<String>,
    page_goal_edit: Option<String>,
    /// focus this widget once no mouse button is pressed (a click elsewhere would take it away)
    focus_later: Option<Id>,
    /// (time of the last scan, words in the thesis, TODOs in the sources)
    scan: Option<(f64, usize, Vec<SourceTodo>)>,
}

fn note_color(i: u8, pal: &Palette) -> Color32 {
    [pal.yellow, pal.green, pal.cyan, pal.blue, pal.magenta, pal.red][i as usize % plan::NOTE_COLORS]
}

/// Ring showing a fraction (0..1) with a label in the middle.
fn ring(ui: &mut Ui, frac: f32, label: &str, color: Color32, pal: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
    let c = r.center();
    let rad = 18.0;
    let steps = 48;
    let pts = |a0: f32, a1: f32| -> Vec<egui::Pos2> {
        (0..=steps).map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32 - std::f32::consts::FRAC_PI_2;
            pos2(c.x + rad * a.cos(), c.y + rad * a.sin())
        }).collect()
    };
    ui.painter().add(egui::Shape::line(pts(0.0, std::f32::consts::TAU), Stroke::new(5.0, with_alpha(pal.text, 25))));
    if frac > 0.0 {
        ui.painter().add(egui::Shape::line(pts(0.0, std::f32::consts::TAU * frac.min(1.0)), Stroke::new(5.0, color)));
    }
    ui.painter().text(c, Align2::CENTER_CENTER, label, widgets::bold_font(10.5), pal.bright);
}

/// A rounded stat card in the header.
fn stat(ui: &mut Ui, pal: &Palette, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(mix(pal.surface, pal.base, 0.3))
        .stroke(Stroke::new(1.0, with_alpha(pal.border, 110)))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_height(48.0);
            ui.horizontal_centered(|ui| add(ui));
        });
}

fn chip(ui: &mut Ui, text: &str, color: Color32) -> egui::Response {
    let font = widgets::ui_font(11.0);
    let g = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let (r, resp) = ui.allocate_exact_size(g.size() + vec2(14.0, 6.0), Sense::click());
    ui.painter().rect_filled(r, 8.0, with_alpha(color, if resp.hovered() { 45 } else { 28 }));
    ui.painter().galley(r.center() - g.size() / 2.0, g, color);
    resp
}

impl App {
    fn plan_scan(&mut self, now: f64) -> (usize, Vec<SourceTodo>) {
        if let Some((t, w, todos)) = &self.plan_ui.scan {
            if now - t < 3.0 {
                return (*w, todos.clone());
            }
        }
        let files = crate::workspace::doc_files(self);
        let main = self.project.config.thesis_main.clone();
        let mut words = 0;
        for f in files.iter().chain(std::iter::once(&main)) {
            if let Some(t) = self.read_source(f) {
                let body = if *f == main { t.split("\\begin{document}").nth(1).unwrap_or("").to_string() } else { t };
                words += plan::count_words(&body);
            }
        }
        let mut todos = vec![];
        for f in self.flat.clone().iter().filter(|f| f.ends_with(".tex")) {
            if let Some(t) = self.read_source(f) {
                todos.extend(plan::scan_todos(f, &t));
            }
        }
        self.plan_ui.scan = Some((now, words, todos.clone()));
        (words, todos)
    }

    pub fn plan_view(&mut self, ui: &mut Ui, now: f64) {
        let pal = self.pal.clone();
        let today = plan::today();
        let (words, source_todos) = self.plan_scan(now);
        let mut changed = false;
        if let Some(id) = self.plan_ui.focus_later {
            if !ui.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
                ui.ctx().memory_mut(|m| m.request_focus(id));
                if ui.ctx().memory(|m| m.has_focus(id)) {
                    self.plan_ui.focus_later = None;
                }
            }
            ui.ctx().request_repaint();
        }

        // ── header: deadline, progress, words ──
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            // deadline
            stat(ui, &pal, |ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(tr!("ABGABE" | "SUBMISSION")).font(widgets::ui_font(10.0)).color(pal.dim).extra_letter_spacing(1.0));
                    if let Some(s) = &mut self.plan_ui.deadline_edit {
                        let r = ui.add(egui::TextEdit::singleline(s).hint_text("15.12.2026").desired_width(110.0));
                        r.request_focus();
                        if r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let c = plan::parse_capture(s, today);
                            self.plan.deadline = c.due.map(|d| d.to_string()).or(if s.trim().is_empty() { None } else { self.plan.deadline.clone() });
                            self.plan_ui.deadline_edit = None;
                            changed = true;
                        }
                    } else {
                        let (txt, col) = match self.plan.deadline.as_deref().and_then(plan::parse_iso) {
                            Some(d) => {
                                let n = (d - today).num_days();
                                let t = match n {
                                    0 => tr!("heute!" | "today!").to_string(),
                                    n if n < 0 => trf!("vor {} Tagen" | "{} days ago", -n),
                                    n => trf!("in {n} Tagen" | "in {n} days"),
                                };
                                (format!("{t}  ·  {}.{}.{}", chrono::Datelike::day(&d), chrono::Datelike::month(&d), chrono::Datelike::year(&d)), if n < 14 { pal.red } else if n < 45 { pal.yellow } else { pal.green })
                            }
                            None => (tr!("Datum setzen …" | "Set a date …").to_string(), pal.dim),
                        };
                        if ui.add(egui::Label::new(egui::RichText::new(txt).font(widgets::bold_font(15.0)).color(col)).sense(Sense::click())).on_hover_text(tr!("Klicken zum Ändern (z. B. 15.12.2026)" | "Click to change (e.g. 2026-12-15)")).clicked() {
                            self.plan_ui.deadline_edit = Some(self.plan.deadline.clone().unwrap_or_default());
                        }
                    }
                });
            });
            // tasks progress
            let total = self.plan.tasks.len();
            let done = self.plan.tasks.iter().filter(|t| t.done).count();
            stat(ui, &pal, |ui| {
                let f = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                ring(ui, f, &format!("{:.0}%", f * 100.0), pal.green, &pal);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(tr!("AUFGABEN" | "TASKS")).font(widgets::ui_font(10.0)).color(pal.dim).extra_letter_spacing(1.0));
                    ui.label(egui::RichText::new(trf!("{done} von {total} erledigt" | "{done} of {total} done")).font(widgets::bold_font(13.5)).color(pal.bright));
                });
            });
            // words
            stat(ui, &pal, |ui| {
                let goal = self.plan.word_goal.unwrap_or(0);
                let f = if goal == 0 { 0.0 } else { words as f32 / goal as f32 };
                ring(ui, f, &format!("{:.0}%", (f * 100.0).min(999.0)), pal.accent, &pal);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(tr!("WÖRTER IN DER ARBEIT" | "WORDS IN THE THESIS")).font(widgets::ui_font(10.0)).color(pal.dim).extra_letter_spacing(1.0));
                    if let Some(s) = &mut self.plan_ui.goal_edit {
                        let r = ui.add(egui::TextEdit::singleline(s).hint_text("20000").desired_width(90.0));
                        r.request_focus();
                        if r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.plan.word_goal = s.chars().filter(char::is_ascii_digit).collect::<String>().parse().ok().filter(|g| *g > 0);
                            self.plan_ui.goal_edit = None;
                            changed = true;
                        }
                    } else {
                        let t = if goal > 0 { format!("{words} / {goal}") } else { format!("{words}  ·  {}", tr!("Ziel setzen" | "set a goal")) };
                        if ui.add(egui::Label::new(egui::RichText::new(t).font(widgets::bold_font(13.5)).color(pal.bright)).sense(Sense::click())).on_hover_text(tr!("Klicken: Wortziel setzen" | "Click: set a word goal")).clicked() {
                            self.plan_ui.goal_edit = Some(goal.to_string());
                        }
                    }
                });
            });
            // pages of the compiled thesis
            let pages = self.thesis.viewer.page_count();
            stat(ui, &pal, |ui| {
                let goal = self.plan.page_goal.unwrap_or(0);
                let f = if goal == 0 { 0.0 } else { pages as f32 / goal as f32 };
                ring(ui, f, &format!("{:.0}%", (f * 100.0).min(999.0)), pal.cyan, &pal);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(tr!("SEITEN (PDF)" | "PAGES (PDF)")).font(widgets::ui_font(10.0)).color(pal.dim).extra_letter_spacing(1.0));
                    if let Some(s) = &mut self.plan_ui.page_goal_edit {
                        let r = ui.add(egui::TextEdit::singleline(s).hint_text("80").desired_width(70.0));
                        r.request_focus();
                        if r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.plan.page_goal = s.chars().filter(char::is_ascii_digit).collect::<String>().parse().ok().filter(|g| *g > 0);
                            self.plan_ui.page_goal_edit = None;
                            changed = true;
                        }
                    } else {
                        let t = if goal > 0 { format!("{pages} / {goal}") } else { format!("{pages}  ·  {}", tr!("Ziel setzen" | "set a goal")) };
                        if ui.add(egui::Label::new(egui::RichText::new(t).font(widgets::bold_font(13.5)).color(pal.bright)).sense(Sense::click())).on_hover_text(tr!("Klicken: Seitenziel setzen" | "Click: set a page goal")).clicked() {
                            self.plan_ui.page_goal_edit = Some(goal.to_string());
                        }
                    }
                });
            });
            // pomodoro tomato
            self.pomodoro_card(ui, &pal, now);
        });
        ui.add_space(14.0);

        // ── quick capture ──
        let w = ui.available_width();
        egui::Frame::new()
            .fill(mix(pal.surface, pal.base, 0.25))
            .stroke(Stroke::new(1.0, with_alpha(pal.accent, 90)))
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(w - 30.0);
                ui.horizontal(|ui| {
                    for (note, icon, label) in [(false, ic::CHECK_CIRCLE, tr!("Aufgabe" | "Task")), (true, ic::STICKY, tr!("Notiz" | "Note"))] {
                        if widgets::chip(ui, icon, label, self.plan_ui.note_mode == note, pal.accent, &pal).clicked() {
                            self.plan_ui.note_mode = note;
                        }
                    }
                    ui.add_space(6.0);
                    let hint = if self.plan_ui.note_mode {
                        tr!("Notiz … (erste Zeile = Titel)" | "Note … (first line = title)")
                    } else {
                        tr!("Neue Aufgabe … z. B. „Kapitel 3 überarbeiten bis Freitag #methodik !“" | "New task … e.g. “revise chapter 3 by friday #method !”")
                    };
                    // Enter is taken before the field sees it (works with input methods, too)
                    let focused = ui.ctx().memory(|m| m.has_focus(Id::new("plan-capture")));
                    let enter_key = focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                    let add_clicked = ui
                        .with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            let b = widgets::button(ui, ic::PLUS, tr!("Hinzufügen" | "Add"), &pal, widgets::BtnKind::Primary).clicked();
                            ui.add(egui::TextEdit::singleline(&mut self.plan_ui.input).id(Id::new("plan-capture")).hint_text(hint).font(widgets::ui_font(14.0)).frame(egui::Frame::NONE).desired_width(f32::INFINITY));
                            b
                        })
                        .inner;
                    let r = ui.ctx().memory(|m| m.has_focus(Id::new("plan-capture")));
                    let _ = r;
                    // a newline committed by an input method counts as Enter, too
                    let newline = self.plan_ui.input.contains('\n');
                    if newline {
                        self.plan_ui.input = self.plan_ui.input.replace('\n', " ");
                    }
                    let enter = enter_key || add_clicked || newline;
                    if enter && !self.plan_ui.input.trim().is_empty() {
                        let input = std::mem::take(&mut self.plan_ui.input);
                        if self.plan_ui.note_mode {
                            let (title, body) = input.split_once(" – ").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((input.trim().to_string(), String::new()));
                            self.plan.add_note(&title, &body);
                        } else {
                            self.plan.add_task(plan::parse_capture(&input, today));
                        }
                        changed = true;
                        self.plan_ui.focus_later = Some(Id::new("plan-capture"));
                    }
                });
                // live preview of what was understood
                if !self.plan_ui.note_mode && !self.plan_ui.input.trim().is_empty() {
                    let c = plan::parse_capture(&self.plan_ui.input, today);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(egui::RichText::new(format!("{}  {}", ic::MAGIC, c.text)).font(widgets::ui_font(12.0)).color(pal.subtext));
                        if let Some(d) = c.due {
                            chip(ui, &format!("{}  {}", ic::CLOCK, plan::due_label(d, today)), pal.cyan);
                        }
                        for t in &c.tags {
                            chip(ui, &format!("#{t}"), pal.magenta);
                        }
                        if c.important {
                            chip(ui, &format!("{} {}", ic::STAR, tr!("wichtig" | "important")), pal.yellow);
                        }
                    });
                }
            });
        ui.add_space(16.0);

        // ── two columns: tasks | notes ──
        let total_w = ui.available_width();
        let col_w = ((total_w - 24.0) / 2.0).max(300.0);
        let mut toggle: Option<u64> = None;
        let mut delete_task: Option<u64> = None;
        let mut jump: Option<(String, usize)> = None;
        let mut open_paper: Option<String> = None;
        egui::ScrollArea::vertical().id_salt("plan-scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 24.0;
                // tasks
                ui.allocate_ui_with_layout(vec2(col_w, 10.0), egui::Layout::top_down(Align::Min), |ui| {
                    ui.set_width(col_w);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tr!("Aufgaben" | "Tasks")).font(widgets::display_font(20.0)).color(pal.bright));
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            let n_done = self.plan.tasks.iter().filter(|t| t.done).count();
                            if n_done > 0 {
                                let label = if self.plan.hide_done { trf!("{n_done} erledigte anzeigen" | "show {n_done} done") } else { trf!("{n_done} erledigte ausblenden" | "hide {n_done} done") };
                                if widgets::chip(ui, if self.plan.hide_done { ic::EYE } else { ic::CHECK }, &label, false, pal.green, &pal).clicked() {
                                    self.plan.hide_done = !self.plan.hide_done;
                                    changed = true;
                                }
                            }
                            for t in self.plan.all_tags().into_iter().rev() {
                                let sel = self.plan_ui.tag_filter.as_deref() == Some(t.as_str());
                                if widgets::chip(ui, "", &format!("#{t}"), sel, pal.magenta, &pal).clicked() {
                                    self.plan_ui.tag_filter = if sel { None } else { Some(t.clone()) };
                                }
                            }
                        });
                    });
                    ui.add_space(6.0);
                    let groups = [
                        (Group::Overdue, tr!("Überfällig" | "Overdue"), pal.red),
                        (Group::Today, tr!("Heute" | "Today"), pal.accent),
                        (Group::Tomorrow, tr!("Morgen" | "Tomorrow"), pal.yellow),
                        (Group::Week, tr!("Diese Woche" | "This week"), pal.cyan),
                        (Group::Later, tr!("Später" | "Later"), pal.blue),
                        (Group::NoDate, tr!("Ohne Datum" | "No date"), pal.subtext),
                    ];
                    let mut any = false;
                    for (g, label, col) in groups {
                        let mut items: Vec<plan::Task> = self
                            .plan
                            .tasks
                            .iter()
                            .filter(|t| (!t.done || !self.plan.hide_done) && task_group(t, today) == g)
                            .filter(|t| self.plan_ui.tag_filter.as_ref().is_none_or(|f| t.tags.contains(f)))
                            .cloned()
                            .collect();
                        if items.is_empty() {
                            continue;
                        }
                        any = true;
                        // open first, finished ones (struck through) at the end of their group
                        items.sort_by(|a, b| a.done.cmp(&b.done).then(b.important.cmp(&a.important)).then(a.due.cmp(&b.due)).then(a.id.cmp(&b.id)));
                        let open_n = items.iter().filter(|t| !t.done).count();
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.0, col);
                            ui.label(egui::RichText::new(format!("{}  {}", label.to_uppercase(), open_n)).font(widgets::ui_font(10.5)).color(col).extra_letter_spacing(1.0));
                        });
                        ui.add_space(2.0);
                        for t in items {
                            self.task_row(ui, &pal, &t, today, col_w, &mut toggle, &mut delete_task, &mut open_paper, &mut changed);
                        }
                        ui.add_space(10.0);
                    }
                    if !any {
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(tr!("Keine offenen Aufgaben – oben einfach lostippen." | "No open tasks – just start typing above.")).color(pal.dim));
                        ui.add_space(10.0);
                    }
                    // TODOs found in the sources
                    if !source_todos.is_empty() {
                        let open = self.plan_ui.show_source;
                        if ui.add(egui::Label::new(egui::RichText::new(format!("{}  {}  {}", if open { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT }, tr!("IM TEXT GEFUNDEN" | "FOUND IN THE TEXT"), source_todos.len())).font(widgets::ui_font(10.5)).color(pal.orange).extra_letter_spacing(1.0)).sense(Sense::click())).on_hover_text("% TODO …  \\todo{…}").clicked() {
                            self.plan_ui.show_source = !open;
                        }
                        if open {
                            for s in &source_todos {
                                let (r, resp) = ui.allocate_exact_size(vec2(col_w, 34.0), Sense::click());
                                if resp.hovered() {
                                    ui.painter().rect_filled(r, 8.0, with_alpha(pal.text, 10));
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                ui.painter().text(pos2(r.min.x + 14.0, r.center().y), Align2::CENTER_CENTER, ic::PENCIL, widgets::ui_font(11.0), pal.orange);
                                ui.painter().text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, crate::app::truncate(&s.text, 60), widgets::ui_font(13.0), pal.text);
                                ui.painter().text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, format!("{}:{}", s.file.rsplit('/').next().unwrap_or(&s.file), s.line), widgets::mono_font(10.5), pal.dim);
                                if resp.on_hover_text(tr!("Zur Stelle springen" | "Jump to the place")).clicked() {
                                    jump = Some((s.file.clone(), s.line));
                                }
                            }
                        }
                        ui.add_space(10.0);
                    }
                });
                // notes
                ui.allocate_ui_with_layout(vec2(col_w, 10.0), egui::Layout::top_down(Align::Min), |ui| {
                    ui.set_width(col_w);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tr!("Notizen" | "Notes")).font(widgets::display_font(20.0)).color(pal.bright));
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            if widgets::button(ui, ic::PLUS, tr!("Notiz" | "Note"), &pal, widgets::BtnKind::Ghost).clicked() {
                                let id = self.plan.add_note("", "");
                                changed = true;
                                self.plan_ui.focus_later = Some(Id::new(("note-title", id)));
                            }
                        });
                    });
                    ui.add_space(6.0);
                    self.notes_grid(ui, &pal, col_w, &mut changed);
                });
            });
        });

        if let Some(id) = toggle {
            self.plan.toggle(id);
            changed = true;
        }
        if let Some(id) = delete_task {
            self.plan.tasks.retain(|t| t.id != id);
            changed = true;
        }
        if changed {
            self.plan.save();
        }
        if let Some((f, l)) = jump {
            self.tab = Tab::Thesis;
            self.jump_to(&f, l, Tab::Thesis);
        }
        if let Some(k) = open_paper {
            self.shelf_ui.plan_view = false;
            self.shelf_ui.selected = Some(k);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn task_row(&mut self, ui: &mut Ui, pal: &Palette, t: &plan::Task, today: chrono::NaiveDate, w: f32, toggle: &mut Option<u64>, delete: &mut Option<u64>, open_paper: &mut Option<String>, changed: &mut bool) {
        let (row, resp) = ui.allocate_exact_size(vec2(w, 38.0), Sense::click());
        let hov = ui.rect_contains_pointer(row);
        if hov {
            ui.painter().rect_filled(row, 9.0, with_alpha(pal.text, 9));
        }
        // check circle
        let c = pos2(row.min.x + 16.0, row.center().y);
        let cr = Rect::from_center_size(c, vec2(22.0, 22.0));
        let cresp = ui.interact(cr, Id::new(("task-check", t.id)), Sense::click());
        let anim = ui.ctx().animate_bool(Id::new(("task-anim", t.id)), t.done);
        let ring_col = if t.important && !t.done { pal.yellow } else { pal.accent };
        ui.painter().circle_stroke(c, 8.5, Stroke::new(1.8, if cresp.hovered() { ring_col } else { with_alpha(ring_col, 170) }));
        if anim > 0.0 {
            ui.painter().circle_filled(c, 8.5 * anim, with_alpha(pal.green, (255.0 * anim) as u8));
            ui.painter().text(c, Align2::CENTER_CENTER, ic::CHECK, widgets::ui_font(9.0), with_alpha(pal.on_accent, (255.0 * anim) as u8));
        }
        if cresp.clicked() {
            *toggle = Some(t.id);
        }
        // text (double click edits)
        let x = row.min.x + 34.0;
        let editing = self.plan_ui.editing.as_ref().is_some_and(|e| e.0 == t.id);
        if editing {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(x, row.min.y + 7.0), pos2(row.max.x - 120.0, row.max.y - 5.0))));
            if let Some((_, s)) = &mut self.plan_ui.editing {
                let r = child.add(egui::TextEdit::singleline(s).font(widgets::ui_font(13.5)).desired_width(f32::INFINITY));
                r.request_focus();
                if r.lost_focus() {
                    let c = plan::parse_capture(s, today);
                    if let Some(task) = self.plan.tasks.iter_mut().find(|x| x.id == t.id) {
                        task.text = c.text;
                        if c.due.is_some() {
                            task.due = c.due.map(|d| d.to_string());
                        }
                        for tag in c.tags {
                            if !task.tags.contains(&tag) {
                                task.tags.push(tag);
                            }
                        }
                        task.important |= c.important;
                    }
                    self.plan_ui.editing = None;
                    *changed = true;
                }
            }
        } else {
            let col = if t.done { pal.dim } else { pal.text };
            let mut job = egui::text::LayoutJob::default();
            for word in t.text.split(' ') {
                let is_cite = word.starts_with('@') && word.len() > 1;
                let f = egui::TextFormat {
                    font_id: widgets::ui_font(13.5),
                    color: if is_cite { pal.accent } else { col },
                    ..Default::default()
                };
                job.append(&format!("{word} "), 0.0, f);
            }
            let g = ui.painter().layout_job(job);
            let tr_ = Rect::from_min_size(pos2(x, row.center().y - g.size().y / 2.0), g.size());
            let text_w = g.size().x.min(row.max.x - 150.0 - x);
            ui.painter().with_clip_rect(Rect::from_min_max(row.min, pos2(row.max.x - 150.0, row.max.y))).galley(tr_.min, g, col);
            // strike-through drawn from left to right when the task gets done
            if anim > 0.0 {
                let y = tr_.center().y + 1.0;
                ui.painter().line_segment([pos2(x - 2.0, y), pos2(x - 2.0 + (text_w + 4.0) * anim, y)], Stroke::new(1.6, with_alpha(pal.dim, 220)));
            }
            // @cite → open the paper
            if resp.clicked() {
                if let Some(k) = t.text.split_whitespace().find(|w| w.starts_with('@')).map(|w| w[1..].trim_matches(|c: char| !c.is_alphanumeric() && c != ':' && c != '-').to_string()) {
                    if self.shelf.papers.iter().any(|p| p.entry.key == k) {
                        *open_paper = Some(k);
                    }
                }
            }
            if resp.double_clicked() {
                let mut s = t.text.clone();
                for tag in &t.tags {
                    s.push_str(&format!(" #{tag}"));
                }
                self.plan_ui.editing = Some((t.id, s));
            }
        }
        // right side: chips, star, due quick picks, delete
        let mut rx = row.max.x - 8.0;
        let mut right = |ui: &mut Ui, text: &str, color: Color32, id: (&str, u64), tip: &str| -> bool {
            let g = ui.painter().layout_no_wrap(text.to_string(), widgets::ui_font(11.0), color);
            let r = Rect::from_min_max(pos2(rx - g.size().x - 12.0, row.center().y - 11.0), pos2(rx, row.center().y + 11.0));
            rx = r.min.x - 4.0;
            let resp = ui.interact(r, Id::new(id), Sense::click()).on_hover_text(tip);
            ui.painter().rect_filled(r, 8.0, with_alpha(color, if resp.hovered() { 45 } else { 24 }));
            ui.painter().galley(r.center() - g.size() / 2.0, g, color);
            resp.clicked()
        };
        if hov && right(ui, ic::TRASH, pal.red, ("task-del", t.id), tr!("Löschen" | "Delete")) {
            *delete = Some(t.id);
        }
        let star_col = if t.important { pal.yellow } else { pal.dim };
        if (hov || t.important) && right(ui, ic::STAR, star_col, ("task-star", t.id), tr!("Wichtig" | "Important")) {
            if let Some(x) = self.plan.tasks.iter_mut().find(|x| x.id == t.id) {
                x.important = !x.important;
                *changed = true;
            }
        }
        if !t.done {
            let due = t.due.as_deref().and_then(plan::parse_iso);
            let label = due.map(|d| plan::due_label(d, today)).unwrap_or_else(|| if hov { format!("{} {}", ic::CLOCK, tr!("Termin" | "Date")) } else { String::new() });
            if !label.is_empty() {
                let col = match plan::group_of(t, today) {
                    Group::Overdue => pal.red,
                    Group::Today => pal.accent,
                    _ => pal.cyan,
                };
                let clicked = right(ui, &label, col, ("task-due", t.id), tr!("Termin: Klick wechselt heute → morgen → nächste Woche → keiner" | "Date: click cycles today → tomorrow → next week → none"));
                if clicked {
                    // cycle: none → today → tomorrow → next monday → none
                    let next = match plan::group_of(t, today) {
                        Group::NoDate | Group::Overdue => Some(today),
                        Group::Today => Some(today + chrono::Duration::days(1)),
                        Group::Tomorrow => Some(today + chrono::Duration::days(7 - chrono::Datelike::weekday(&today).num_days_from_monday() as i64)),
                        _ => None,
                    };
                    if let Some(x) = self.plan.tasks.iter_mut().find(|x| x.id == t.id) {
                        x.due = next.map(|d| d.to_string());
                        *changed = true;
                    }
                }
            }
        }
        for tag in t.tags.iter().rev() {
            if right(ui, &format!("#{tag}"), pal.magenta, ("task-tag", t.id ^ (tag.len() as u64) << 32), tr!("Nach Schlagwort filtern" | "Filter by tag")) {
                self.plan_ui.tag_filter = Some(tag.clone());
            }
        }
    }

    fn notes_grid(&mut self, ui: &mut Ui, pal: &Palette, w: f32, changed: &mut bool) {
        if self.plan.notes.is_empty() {
            ui.label(egui::RichText::new(tr!("Gedanken, Feedback der Betreuung, Ideen – alles, was nicht in den Text gehört." | "Thoughts, supervisor feedback, ideas – everything that doesn't belong in the text.")).color(pal.dim));
            return;
        }
        let mut order: Vec<usize> = (0..self.plan.notes.len()).collect();
        order.sort_by(|a, b| {
            let (x, y) = (&self.plan.notes[*a], &self.plan.notes[*b]);
            // stable while typing: pinned first, then newest by creation
            y.pinned.cmp(&x.pinned).then(y.created.cmp(&x.created)).then(y.id.cmp(&x.id))
        });
        let gap = 12.0;
        let cw = (w - gap) / 2.0;
        let mut delete: Option<u64> = None;
        // two columns, alternating – a note never jumps to the other column while typing
        let mut cols: [Vec<usize>; 2] = [vec![], vec![]];
        for (k, i) in order.into_iter().enumerate() {
            cols[k % 2].push(i);
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for col in cols {
                ui.allocate_ui_with_layout(vec2(cw, 10.0), egui::Layout::top_down(Align::Min), |ui| {
                    ui.set_width(cw);
                    for i in col {
                        let id = self.plan.notes[i].id;
                        let color = note_color(self.plan.notes[i].color, pal);
                        let start = ui.cursor().min;
                        let bg = ui.painter().add(egui::Shape::Noop);
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            let n = &mut self.plan.notes[i];
                            let r = ui.add(egui::TextEdit::singleline(&mut n.title).id(Id::new(("note-title", id))).hint_text(tr!("Titel" | "Title")).font(widgets::bold_font(14.0)).text_color(pal.bright).frame(egui::Frame::NONE).desired_width(cw - 70.0));
                            if r.changed() {
                                n.updated = plan::now();
                                *changed = true;
                            }
                            if n.pinned {
                                ui.label(egui::RichText::new(ic::BOOKMARK).font(widgets::ui_font(11.0)).color(color));
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            let n = &mut self.plan.notes[i];
                            let r = ui.add(egui::TextEdit::multiline(&mut n.body).id(Id::new(("note-body", id))).hint_text(tr!("Notiz …" | "Note …")).font(widgets::ui_font(13.0)).text_color(pal.text).frame(egui::Frame::NONE).desired_width(cw - 26.0).desired_rows(2));
                            if r.changed() {
                                n.updated = plan::now();
                                *changed = true;
                            }
                        });
                        ui.add_space(4.0);
                        let foot_y = ui.cursor().min.y;
                        let (fr, _) = ui.allocate_exact_size(vec2(cw, 26.0), Sense::hover());
                        let card = Rect::from_min_max(start, pos2(start.x + cw, fr.max.y + 4.0));
                        let hov = ui.rect_contains_pointer(card);
                        ui.painter().set(
                            bg,
                            egui::Shape::Vec(vec![
                                egui::Shape::rect_filled(card, 12.0, mix(pal.surface, color, 0.07)),
                                egui::Shape::rect_stroke(card, 12.0, Stroke::new(1.0, with_alpha(color, if hov { 120 } else { 60 })), StrokeKind::Inside),
                                egui::Shape::rect_filled(Rect::from_min_size(card.min + vec2(12.0, 0.0), vec2(cw - 24.0, 3.0)), CornerRadius { nw: 0, ne: 0, sw: 2, se: 2 }, color),
                            ]),
                        );
                        // footer: colors, pin, delete (on hover), relative time
                        let n = &self.plan.notes[i];
                        let age = plan::now() - n.updated;
                        let when = if age < 3600 { tr!("gerade eben" | "just now").to_string() } else if age < 86400 { trf!("vor {} Std." | "{} h ago", age / 3600) } else if age < 2 * 86400 { tr!("gestern" | "yesterday").to_string() } else { trf!("vor {} Tagen" | "{} days ago", age / 86400) };
                        ui.painter().text(pos2(card.min.x + 12.0, foot_y + 13.0), Align2::LEFT_CENTER, when, widgets::ui_font(10.5), pal.dim);
                        if hov {
                            let mut x = card.max.x - 12.0;
                            let mut btn = |ui: &mut Ui, icon: &str, col: Color32, key: &str| -> bool {
                                let r = Rect::from_center_size(pos2(x - 10.0, foot_y + 13.0), vec2(20.0, 20.0));
                                x -= 24.0;
                                let resp = ui.interact(r, Id::new(("note-btn", id, key.to_string())), Sense::click());
                                ui.painter().rect_filled(r, 6.0, with_alpha(col, if resp.hovered() { 50 } else { 18 }));
                                ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, widgets::ui_font(10.0), col);
                                resp.clicked()
                            };
                            if btn(ui, ic::TRASH, pal.red, "del") {
                                delete = Some(id);
                            }
                            if btn(ui, ic::BOOKMARK, if n.pinned { color } else { pal.dim }, "pin") {
                                self.plan.notes[i].pinned = !self.plan.notes[i].pinned;
                                *changed = true;
                            }
                            for k in (0..plan::NOTE_COLORS).rev() {
                                let r = Rect::from_center_size(pos2(x - 7.0, foot_y + 13.0), vec2(14.0, 14.0));
                                x -= 16.0;
                                let resp = ui.interact(r, Id::new(("note-color", id, k)), Sense::click());
                                let c = note_color(k as u8, pal);
                                ui.painter().circle_filled(r.center(), if resp.hovered() { 6.5 } else { 5.0 }, c);
                                if self.plan.notes[i].color as usize == k {
                                    ui.painter().circle_stroke(r.center(), 7.5, Stroke::new(1.2, pal.bright));
                                }
                                if resp.clicked() {
                                    self.plan.notes[i].color = k as u8;
                                    *changed = true;
                                }
                            }
                        }
                        ui.add_space(gap);
                    }
                });
            }
        });
        if let Some(id) = delete {
            self.plan.notes.retain(|n| n.id != id);
            *changed = true;
        }
    }
}

impl App {
    /// Screenshot tests: "planview-cap" types an example into the quick capture.
    pub fn plan_ui_debug_input(&mut self, name: &str) {
        if name.contains("cap") && self.plan_ui.input.is_empty() {
            self.plan_ui.input = "Kapitel 3 überarbeiten bis Freitag #methodik !".into();
        }
    }
}

/// Group of a task in the list; finished tasks are never "overdue".
fn task_group(t: &plan::Task, today: chrono::NaiveDate) -> Group {
    match plan::group_of(t, today) {
        Group::Overdue if t.done => Group::Today,
        g => g,
    }
}

/// Load the plan when a project opens.
pub fn load_for(app: &mut App) {
    app.plan = Plan::load(&app.project.root);
    app.plan_ui = PlanUi::default();
}
