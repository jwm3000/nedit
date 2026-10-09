//! Visual slide editor ("like PowerPoint"): the current Beamer frame is shown as an editable
//! slide. Every change rewrites only that frame with clean LaTeX (see `slides.rs`).

use crate::app::{App, Tab};
use crate::icons as ic;
use crate::slides::{self, BlockKind, Deck, Elem, Item, Slide, Template};
use crate::theme::{mix, with_alpha};
use crate::widgets;
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{pos2, vec2, Align, Align2, Color32, FontFamily, FontId, Id, Key, Modifiers, Rect, Sense, Stroke, StrokeKind, Ui};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

// TU Graz corporate colors
const TUG_RED: Color32 = Color32::from_rgb(0xe4, 0x15, 0x4b);
const TUG_BLUE: Color32 = Color32::from_rgb(0x1f, 0x6e, 0xa8);
const INK: Color32 = Color32::from_rgb(0x22, 0x24, 0x28);
const SLATE: Color32 = Color32::from_rgb(0x3e, 0x58, 0x6b);

#[derive(Default)]
pub struct SlideEditor {
    pub debug_focus: Option<Id>,
    key: u64,
    deck: Deck,
    /// Field to focus on the next frame (after inserting an item or element).
    focus_next: Option<Id>,
    /// Last focused field, its element path and selection (toolbar actions use it).
    last_focus: Option<(Id, Vec<usize>, (usize, usize))>,
    /// Wrap the selection of a field: (field, prefix, suffix).
    pending_wrap: Option<(Id, &'static str, &'static str)>,
    images: HashMap<PathBuf, (u64, Option<egui::TextureHandle>)>,
    undo: Vec<String>,
    redo: Vec<String>,
    last_snapshot: f64,
    last_sel: Option<usize>,
}

/// Structural changes requested while drawing; applied afterwards.
enum Op {
    MoveElem(Vec<usize>, i32),
    DeleteElem(Vec<usize>),
    InsertItem { path: Vec<usize>, after: usize, level: u8 },
    DeleteItem { path: Vec<usize>, idx: usize },
    ItemLevel { path: Vec<usize>, idx: usize, delta: i32 },
    PickImage(Vec<usize>),
    Meta(&'static str, String),
}

struct Cx<'a> {
    /// scale: canvas width / 640
    k: f32,
    tug: bool,
    frame: usize,
    ops: Vec<Op>,
    focused: Option<Vec<usize>>,
    ed: &'a mut SlideEditor,
    dirs: (PathBuf, PathBuf),
    sections: Vec<String>,
    meta: HashMap<&'static str, String>,
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

pub fn field_id(frame: usize, path: &[usize], sub: usize) -> Id {
    Id::new(("slide-field", frame, path.to_vec(), sub))
}

fn get_elem<'a>(body: &'a mut Vec<Elem>, path: &[usize]) -> Option<&'a mut Elem> {
    let (first, rest) = path.split_first()?;
    let e = body.get_mut(*first)?;
    if rest.is_empty() {
        return Some(e);
    }
    match e {
        Elem::Block { body, .. } => get_elem(body, rest),
        Elem::Columns { cols, .. } => {
            let (c, rest2) = rest.split_first()?;
            get_elem(cols.get_mut(*c)?, rest2)
        }
        _ => None,
    }
}

/// The element list that contains `path` (and the index inside it).
fn get_parent<'a>(body: &'a mut Vec<Elem>, path: &[usize]) -> Option<(&'a mut Vec<Elem>, usize)> {
    match path.len() {
        0 => None,
        1 => Some((body, path[0])),
        _ => {
            let (first, rest) = path.split_first()?;
            match body.get_mut(*first)? {
                Elem::Block { body, .. } => get_parent(body, rest),
                Elem::Columns { cols, .. } => {
                    let (c, rest2) = rest.split_first()?;
                    get_parent(cols.get_mut(*c)?, rest2)
                }
                _ => None,
            }
        }
    }
}

// ───────────────────────────── inline rendering ─────────────────────────────

#[derive(Clone, Copy)]
struct St {
    bold: bool,
    italic: bool,
    color: Color32,
    mono: bool,
}

/// Render inline LaTeX: markup is hidden (or dimmed while editing), \textbf / \emph / \alert /
/// math / citations are styled. Leading `\centering` and size commands change the paragraph.
fn inline_job(text: &str, size: f32, base: St, accent: Color32, editing: bool, wrap: f32, center: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    if center {
        job.halign = Align::Center;
    }
    let hidden = |size: f32| TextFormat { font_id: FontId::new(if editing { size * 0.82 } else { 0.5 }, FontFamily::Monospace), color: if editing { Color32::from_gray(150) } else { Color32::TRANSPARENT }, ..Default::default() };
    let fmt = |st: St, size: f32| {
        let family = if st.mono {
            FontFamily::Monospace
        } else if st.bold {
            FontFamily::Name("ui-bold".into())
        } else {
            FontFamily::Proportional
        };
        TextFormat { font_id: FontId::new(if st.mono { size * 0.92 } else { size }, family), color: st.color, italics: st.italic, ..Default::default() }
    };
    let b = text.as_bytes();
    let mut stack: Vec<St> = vec![base];
    let mut math = false;
    let mut i = 0;
    let mut scale = 1.0f32;
    let push = |job: &mut LayoutJob, s: usize, e: usize, f: TextFormat| {
        if e > s {
            job.append(&text[s..e], 0.0, f);
        }
    };
    while i < b.len() {
        let st = *stack.last().unwrap();
        let sz = size * scale;
        match b[i] {
            b'\\' => {
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_alphabetic() {
                    j += 1;
                }
                if j == i + 1 {
                    // escaped char: \% \& \_ \# \, \\ …
                    let n = (i + 2).min(b.len());
                    let c = text[i + 1..].chars().next().unwrap_or(' ');
                    let end = i + 1 + c.len_utf8();
                    if c == '\\' && !editing {
                        // \\ is a line break: same char count, so cursor positions still match
                        job.append("\u{200b}", 0.0, hidden(sz));
                        job.append("\n", 0.0, hidden(sz));
                        i = end;
                        continue;
                    }
                    push(&mut job, i, i + 1, hidden(sz));
                    if matches!(c, ',' | '\\' | ' ' | ';') {
                        push(&mut job, i + 1, end, hidden(sz));
                    } else {
                        push(&mut job, i + 1, end, fmt(st, sz));
                    }
                    let _ = n;
                    i = end;
                    continue;
                }
                let name = &text[i + 1..j];
                let mut k2 = j;
                if k2 < b.len() && b[k2] == b'*' {
                    k2 += 1;
                }
                let has_arg = k2 < b.len() && b[k2] == b'{';
                let new = match name {
                    "textbf" | "structure" => Some(St { bold: true, ..st }),
                    "textit" | "emph" | "textsl" => Some(St { italic: true, ..st }),
                    "alert" => Some(St { color: accent, bold: st.bold, ..st }),
                    "texttt" | "url" => Some(St { mono: true, ..st }),
                    n if n.contains("cite") || n.contains("ref") => Some(St { color: mix(accent, base.color, 0.35), ..st }),
                    _ => None,
                };
                match (new, has_arg) {
                    (Some(ns), true) => {
                        push(&mut job, i, k2 + 1, hidden(sz));
                        stack.push(ns);
                        i = k2 + 1;
                    }
                    _ => {
                        match name {
                            "Large" | "LARGE" => scale = 1.4,
                            "large" => scale = 1.2,
                            "huge" | "Huge" => scale = 1.8,
                            "small" | "footnotesize" => scale = 0.85,
                            _ => {}
                        }
                        // unknown commands with no style are hidden (\centering, \Large …)
                        let mut e = k2;
                        if e < b.len() && b[e] == b' ' {
                            e += 1;
                        }
                        push(&mut job, i, e, hidden(sz));
                        i = e;
                    }
                }
            }
            b'{' => {
                push(&mut job, i, i + 1, hidden(sz));
                stack.push(st);
                i += 1;
            }
            b'}' => {
                push(&mut job, i, i + 1, hidden(sz));
                if stack.len() > 1 {
                    stack.pop();
                }
                i += 1;
            }
            b'$' => {
                push(&mut job, i, i + 1, hidden(sz));
                math = !math;
                if math {
                    stack.push(St { italic: true, color: mix(base.color, TUG_BLUE, 0.6), ..st });
                } else if stack.len() > 1 {
                    stack.pop();
                }
                i += 1;
            }
            b'-' if !editing && !st.mono && b.get(i + 1) == Some(&b'-') => {
                // -- → en dash (same char count keeps cursor positions valid)
                job.append("–", 0.0, fmt(st, sz));
                job.append("\u{200b}", 0.0, hidden(sz));
                i += 2;
            }
            _ => {
                let mut e = i + 1;
                while e < b.len() && !matches!(b[e], b'\\' | b'{' | b'}' | b'$' | b'-') {
                    e += 1;
                }
                // stay on char boundaries
                while e < b.len() && !text.is_char_boundary(e) {
                    e += 1;
                }
                push(&mut job, i, e, fmt(st, sz));
                i = e;
            }
        }
    }
    if job.sections.is_empty() {
        job.append("", 0.0, fmt(base, size));
    }
    job
}

// ───────────────────────────── fields ─────────────────────────────

struct FieldOut {
    enter: bool,
    backspace_empty: bool,
    tab: bool,
    shift_tab: bool,
    rect: Rect,
}

struct FieldStyle {
    size: f32,
    color: Color32,
    bold: bool,
    mono: bool,
    hint: &'static str,
    /// Enter creates a new item instead of a line break.
    enter_new: bool,
}

fn selection(ctx: &egui::Context, id: Id, text: &str) -> (usize, usize) {
    let n = text.chars().count();
    egui::TextEdit::load_state(ctx, id)
        .and_then(|s| s.cursor.char_range())
        .map(|r| {
            let (a, b) = (r.primary.index.0.min(n), r.secondary.index.0.min(n));
            (a.min(b), a.max(b))
        })
        .unwrap_or((n, n))
}

fn set_cursor(ctx: &egui::Context, id: Id, a: usize, b: usize) {
    let mut st = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
    st.store(ctx, id);
}

fn wrap_sel(text: &mut String, (a, b): (usize, usize), pre: &str, post: &str) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = (a.min(chars.len()), b.min(chars.len()));
    let mid: String = chars[a..b].iter().collect();
    let mut t: String = chars[..a].iter().collect();
    t.push_str(pre);
    t.push_str(&mid);
    t.push_str(post);
    t.extend(chars[b..].iter());
    *text = t;
    let s = a + pre.chars().count();
    (s, s + mid.chars().count())
}

fn field(ui: &mut Ui, cx: &mut Cx, id: Id, path: &[usize], text: &mut String, fs: &FieldStyle, width: f32) -> FieldOut {
    let ctx = ui.ctx().clone();
    let mut out = FieldOut { enter: false, backspace_empty: false, tab: false, shift_tab: false, rect: Rect::NOTHING };
    let focused = ctx.memory(|m| m.has_focus(id));
    if focused {
        let sel = selection(&ctx, id, text);
        // keys handled before the TextEdit sees them
        ctx.input_mut(|i| {
            if fs.enter_new && i.consume_key(Modifiers::NONE, Key::Enter) {
                out.enter = true;
            }
            if text.is_empty() && i.key_pressed(Key::Backspace) {
                i.consume_key(Modifiers::NONE, Key::Backspace);
                out.backspace_empty = true;
            }
            if fs.enter_new {
                if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                    out.shift_tab = true;
                }
                if i.consume_key(Modifiers::NONE, Key::Tab) {
                    out.tab = true;
                }
            }
        });
        let bold = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::B));
        let ital = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::I));
        if (bold || ital) && !fs.mono {
            let (pre, post) = if bold { ("\\textbf{", "}") } else { ("\\emph{", "}") };
            let (a, b) = wrap_sel(text, sel, pre, post);
            set_cursor(&ctx, id, a, b);
        }
        // LaTeX-special characters typed in a visual field are escaped
        if !fs.mono {
            let prev = if sel.0 > 0 { text.chars().nth(sel.0 - 1) } else { None };
            ctx.input_mut(|i| {
                for e in i.events.iter_mut() {
                    if let egui::Event::Text(t) = e {
                        let mut cs = t.chars();
                        if let (Some(c), None) = (cs.next(), cs.next()) {
                            if let Some(esc) = slides::escape_typed(c, prev) {
                                *t = esc.to_string();
                            }
                        }
                    }
                }
            });
        }
    }
    if let Some((wid, pre, post)) = cx.ed.pending_wrap {
        if wid == id {
            cx.ed.pending_wrap = None;
            let sel = cx.ed.last_focus.as_ref().filter(|l| l.0 == id).map(|l| l.2).unwrap_or((0, text.chars().count()));
            let (a, b) = wrap_sel(text, sel, pre, post);
            set_cursor(&ctx, id, a, b);
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }
    if cx.ed.focus_next == Some(id) {
        cx.ed.focus_next = None;
        ctx.memory_mut(|m| m.request_focus(id));
        let n = text.chars().count();
        set_cursor(&ctx, id, n, n);
    }
    let base = St { bold: fs.bold, italic: false, color: fs.color, mono: fs.mono };
    let accent = if cx.tug { TUG_RED } else { Color32::from_rgb(0xc0, 0x30, 0x30) };
    let center = text.trim_start().starts_with("\\centering");
    let size = fs.size;
    let mono = fs.mono;
    let mut layouter = move |ui: &Ui, tb: &dyn egui::TextBuffer, wrap: f32| {
        let s = tb.as_str();
        let job = if mono {
            LayoutJob::simple(s.to_string(), FontId::new(size, FontFamily::Monospace), base.color, wrap)
        } else {
            inline_job(s, size, base, accent, focused, wrap, center && !focused)
        };
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let te = egui::TextEdit::multiline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .desired_width(width)
        .desired_rows(1)
        .hint_text(egui::RichText::new(crate::i18n::t(fs.hint)).color(with_alpha(fs.color, 90)).size(fs.size))
        .layouter(&mut layouter)
        .lock_focus(fs.mono);
    let resp = ui.add(te);
    if resp.has_focus() {
        cx.focused = Some(path.to_vec());
        let sel = selection(&ctx, id, text);
        cx.ed.last_focus = Some((id, path.to_vec(), sel));
    }
    out.rect = resp.rect;
    out
}

// ───────────────────────────── elements ─────────────────────────────

fn elems(ui: &mut Ui, cx: &mut Cx, list: &mut [Elem], path: &[usize], width: f32) {
    let n = list.len();
    for (i, e) in list.iter_mut().enumerate() {
        let mut p = path.to_vec();
        p.push(i);
        let top = ui.cursor().min.y;
        elem(ui, cx, e, &p, width);
        let rect = Rect::from_min_max(pos2(ui.min_rect().min.x, top), pos2(ui.min_rect().min.x + width, ui.min_rect().max.y));
        controls_column(ui, cx, rect, &p, n);
        ui.add_space(7.0 * cx.k);
    }
}

/// ↑ ↓ ✕ buttons stacked at the right edge of an element when hovered or focused.
fn controls_column(ui: &mut Ui, cx: &mut Cx, rect: Rect, path: &[usize], n: usize) {
    let show = ui.rect_contains_pointer(rect.expand2(vec2(40.0, 2.0))) || cx.focused.as_deref() == Some(path);
    if !show {
        return;
    }
    let bs = (14.0 * cx.k).clamp(17.0, 22.0);
    let mut y = rect.min.y;
    let idx = *path.last().unwrap_or(&0);
    let items: Vec<(&str, &str, u8)> = [(ic::ARROW_UP, "Nach oben", 0u8), (ic::ARROW_DOWN, "Nach unten", 1), (ic::TRASH, "Element löschen", 2)]
        .into_iter()
        .filter(|(_, _, w)| !(*w == 0 && idx == 0) && !(*w == 1 && idx + 1 >= n))
        .collect();
    for (icon, tip, what) in items {
        let r = Rect::from_min_size(pos2(rect.max.x + 6.0, y), vec2(bs, bs));
        let resp = ui.interact(r, Id::new(("slide-ctl", cx.frame, path.to_vec(), what)), Sense::click()).on_hover_text(crate::i18n::t(tip));
        let col = if what == 2 { Color32::from_rgb(0xc0, 0x39, 0x2b) } else { Color32::from_gray(80) };
        ui.painter().rect_filled(r, 4.0, if resp.hovered() { Color32::from_gray(222) } else { Color32::from_gray(238) });
        ui.painter().rect_stroke(r, 4.0, Stroke::new(1.0, Color32::from_gray(210)), StrokeKind::Inside);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, FontId::proportional(bs * 0.5), col);
        if resp.clicked() {
            cx.ops.push(match what {
                0 => Op::MoveElem(path.to_vec(), -1),
                1 => Op::MoveElem(path.to_vec(), 1),
                _ => Op::DeleteElem(path.to_vec()),
            });
        }
        y += bs + 3.0;
    }
}

fn body_style(cx: &Cx) -> FieldStyle {
    FieldStyle { size: 15.0 * cx.k, color: INK, bold: false, mono: false, hint: "Text …", enter_new: false }
}

fn elem(ui: &mut Ui, cx: &mut Cx, e: &mut Elem, path: &[usize], width: f32) {
    let k = cx.k;
    let frame = cx.frame;
    let fid = |sub: usize| field_id(frame, path, sub);
    match e {
        Elem::Text(t) => {
            let id = fid(0);
            let o = field(ui, cx, id, path, t, &body_style(cx), width);
            if o.backspace_empty {
                cx.ops.push(Op::DeleteElem(path.to_vec()));
            }
        }
        Elem::List { numbered, items, .. } => {
            let mut counters = [0usize; 6];
            for idx in 0..items.len() {
                let level = items[idx].level.min(5) as usize;
                counters[level] += 1;
                for c in counters.iter_mut().skip(level + 1) {
                    *c = 0;
                }
                let indent = level as f32 * 20.0 * k;
                let id = fid(1000 + idx);
                let size = (15.0 - level as f32 * 1.5) * k;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(indent);
                    let (r, _) = ui.allocate_exact_size(vec2(18.0 * k, size * 1.35), Sense::hover());
                    let c = pos2(r.min.x + 6.0 * k, r.center().y);
                    let accent = if cx.tug { TUG_RED } else { TUG_BLUE };
                    if *numbered {
                        let label = if level == 0 { format!("{}.", counters[0]) } else { format!("{}.", (b'a' + ((counters[level] - 1) % 26) as u8) as char) };
                        ui.painter().text(pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(size * 0.95), accent);
                    } else if cx.tug {
                        let s = (5.5 - level as f32) * k;
                        ui.painter().rect_filled(Rect::from_center_size(c, vec2(s, s)), 0.0, if level == 0 { accent } else { mix(accent, Color32::WHITE, 0.35) });
                    } else {
                        ui.painter().circle_filled(c, 2.6 * k, accent);
                    }
                    let overlay = items[idx].overlay.clone();
                    let fs = FieldStyle { size, color: INK, bold: false, mono: false, hint: "Aufzählungspunkt …", enter_new: true };
                    let w = (width - indent - 18.0 * k - if overlay.is_empty() { 0.0 } else { 34.0 * k }).max(40.0);
                    let o = field(ui, cx, id, path, &mut items[idx].text, &fs, w);
                    if !overlay.is_empty() {
                        // reveal step badge (<2->)
                        let t = overlay.trim_matches(['<', '>']).to_string();
                        let (br, _) = ui.allocate_exact_size(vec2(30.0 * k, size * 1.2), Sense::hover());
                        let pill = Rect::from_center_size(br.center(), vec2(26.0 * k, size * 0.95));
                        ui.painter().rect_filled(pill, 6.0, Color32::from_gray(232));
                        ui.painter().text(pill.center(), Align2::CENTER_CENTER, t, FontId::proportional(size * 0.6), Color32::from_gray(110));
                    }
                    if o.enter {
                        cx.ops.push(Op::InsertItem { path: path.to_vec(), after: idx, level: level as u8 });
                    }
                    if o.backspace_empty {
                        cx.ops.push(Op::DeleteItem { path: path.to_vec(), idx });
                    }
                    if o.tab {
                        cx.ops.push(Op::ItemLevel { path: path.to_vec(), idx, delta: 1 });
                    }
                    if o.shift_tab {
                        cx.ops.push(Op::ItemLevel { path: path.to_vec(), idx, delta: -1 });
                    }
                });
                ui.add_space(3.0 * k);
            }
        }
        Elem::Block { kind, title, body } => {
            let (head, fill) = match kind {
                BlockKind::Block => (SLATE, Color32::from_rgb(0xec, 0xeb, 0xe4)),
                BlockKind::Alert => (TUG_RED, Color32::from_rgb(0xfb, 0xe7, 0xea)),
                BlockKind::Example => (Color32::from_rgb(0x2e, 0x7d, 0x4f), Color32::from_rgb(0xe4, 0xf2, 0xe8)),
            };
            let pad = 7.0 * k;
            let start = ui.cursor().min;
            let head_idx = ui.painter().add(egui::Shape::Noop);
            let body_idx = ui.painter().add(egui::Shape::Noop);
            ui.add_space(3.0 * k);
            let mut title_rect = Rect::NOTHING;
            ui.horizontal(|ui| {
                ui.add_space(pad);
                let fs = FieldStyle { size: 14.0 * k, color: Color32::WHITE, bold: false, mono: false, hint: "Titel", enter_new: true };
                let o = field(ui, cx, fid(1), path, title, &fs, width - 2.0 * pad);
                title_rect = o.rect;
            });
            ui.add_space(5.0 * k);
            let head_bottom = ui.cursor().min.y;
            ui.horizontal_top(|ui| {
                ui.add_space(pad);
                ui.vertical(|ui| {
                    if body.is_empty() {
                        body.push(Elem::Text(String::new()));
                    }
                    let mut sub = path.to_vec();
                    sub.truncate(path.len());
                    elems(ui, cx, body, path, width - 2.0 * pad);
                });
            });
            let end_y = ui.cursor().min.y;
            let r = Rect::from_min_max(start, pos2(start.x + width, end_y));
            let hr = Rect::from_min_max(start, pos2(start.x + width, head_bottom));
            ui.painter().set(body_idx, egui::Shape::rect_filled(r, 3.0 * k, fill));
            ui.painter().set(head_idx, egui::Shape::Vec(vec![]));
            let mut v = vec![egui::Shape::rect_filled(r, 3.0 * k, fill), egui::Shape::rect_filled(hr, egui::CornerRadius { nw: (3.0 * k) as u8, ne: (3.0 * k) as u8, sw: 0, se: 0 }, head)];
            v.truncate(2);
            ui.painter().set(body_idx, egui::Shape::Vec(v));
            // kind switch on hover
            if ui.rect_contains_pointer(hr) || cx.focused.as_deref().is_some_and(|f| f.starts_with(path)) {
                let mut x = hr.max.x - 4.0;
                for (bk, col) in [(BlockKind::Example, Color32::from_rgb(0x2e, 0x7d, 0x4f)), (BlockKind::Alert, TUG_RED), (BlockKind::Block, SLATE)] {
                    let rr = Rect::from_center_size(pos2(x - 6.0 * k, hr.center().y), vec2(9.0 * k, 9.0 * k));
                    let resp = ui.interact(rr, Id::new(("blk-kind", cx.frame, path.to_vec(), bk.env())), Sense::click()).on_hover_text(bk.env());
                    ui.painter().circle_filled(rr.center(), 4.0 * k, mix(col, Color32::WHITE, 0.25));
                    ui.painter().circle_stroke(rr.center(), 4.0 * k, Stroke::new(if *kind == bk { 2.0 } else { 1.0 }, Color32::WHITE));
                    if resp.clicked() {
                        *kind = bk;
                    }
                    x -= 13.0 * k;
                }
            }
            let _ = title_rect;
        }
        Elem::Image { path: p, height } => {
            let (dir, root) = cx.dirs.clone();
            let max_h = 300.0 * k * *height;
            let tex = load_image(ui.ctx(), cx.ed, &dir, &root, p);
            let (r, resp) = match &tex {
                Some(t) => {
                    let s = t.size_vec2();
                    let h = max_h.min(width * s.y / s.x);
                    let w = h * s.x / s.y;
                    let (row, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
                    let r = Rect::from_center_size(row.center(), vec2(w, h));
                    ui.painter().image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    (r, resp)
                }
                None => {
                    let (row, resp) = ui.allocate_exact_size(vec2(width, max_h.max(60.0 * k)), Sense::click());
                    let r = Rect::from_center_size(row.center(), vec2((max_h * 1.5).min(width), row.height()));
                    ui.painter().rect_filled(r, 6.0, Color32::from_gray(240));
                    ui.painter().rect_stroke(r, 6.0, Stroke::new(1.0, Color32::from_gray(200)), StrokeKind::Inside);
                    let label = if p.is_empty() { crate::i18n::t("Bild wählen …").to_string() } else { p.clone() };
                    ui.painter().text(r.center() - vec2(0.0, 10.0 * k), Align2::CENTER_CENTER, ic::IMAGE, FontId::proportional(22.0 * k), Color32::from_gray(160));
                    ui.painter().text(r.center() + vec2(0.0, 14.0 * k), Align2::CENTER_CENTER, label, FontId::proportional(10.0 * k), Color32::from_gray(120));
                    (r, resp)
                }
            };
            if resp.double_clicked() || (tex.is_none() && resp.clicked()) {
                cx.ops.push(Op::PickImage(path.to_vec()));
            }
            if ui.rect_contains_pointer(r.expand(4.0)) {
                // size and replace controls
                let bar = Rect::from_center_size(pos2(r.center().x, r.max.y - 16.0 * k), vec2(150.0 * k.max(0.8), 22.0 * k.max(0.8)));
                ui.painter().rect_filled(bar, 8.0, with_alpha(Color32::BLACK, 170));
                let third = bar.width() / 3.0;
                for (j, (label, d)) in [("−", -0.1f32), ("+", 0.1), (ic::IMAGE, 0.0)].iter().enumerate() {
                    let rr = Rect::from_min_size(pos2(bar.min.x + j as f32 * third, bar.min.y), vec2(third, bar.height()));
                    let resp = ui.interact(rr, Id::new(("img-ctl", cx.frame, path.to_vec(), j)), Sense::click());
                    ui.painter().text(rr.center(), Align2::CENTER_CENTER, *label, FontId::proportional(12.0 * k.max(0.8)), if resp.hovered() { Color32::WHITE } else { Color32::from_gray(210) });
                    if resp.clicked() {
                        if *d == 0.0 {
                            cx.ops.push(Op::PickImage(path.to_vec()));
                        } else {
                            *height = ((*height + d) * 10.0).round() / 10.0;
                            *height = height.clamp(0.2, 0.9);
                        }
                    }
                }
            }
        }
        Elem::Columns { cols, .. } => {
            let n = cols.len().max(1) as f32;
            let gap = 14.0 * k;
            let cw = (width - gap * (n - 1.0)) / n;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (c, col) in cols.iter_mut().enumerate() {
                    ui.allocate_ui_with_layout(vec2(cw, 10.0), egui::Layout::top_down(Align::Min), |ui| {
                        ui.set_width(cw);
                        if col.is_empty() {
                            col.push(Elem::Text(String::new()));
                        }
                        let mut p = path.to_vec();
                        p.push(c);
                        elems(ui, cx, col, &p, cw - 24.0);
                    });
                }
            });
        }
        Elem::Math(m) => {
            let start = ui.cursor().min;
            let bg = ui.painter().add(egui::Shape::Noop);
            ui.horizontal(|ui| {
                ui.add_space(8.0 * k);
                ui.label(egui::RichText::new("∑").size(14.0 * k).color(TUG_BLUE));
                let fs = FieldStyle { size: 13.0 * k, color: INK, bold: false, mono: true, hint: "Formel, z. B. E = mc^2", enter_new: false };
                field(ui, cx, fid(0), path, m, &fs, width - 40.0 * k);
            });
            let r = Rect::from_min_max(start, pos2(start.x + width, ui.cursor().min.y + 2.0));
            ui.painter().set(bg, egui::Shape::rect_filled(r, 5.0, Color32::from_rgb(0xf1, 0xf4, 0xf8)));
        }
        Elem::Pause => {
            let (r, _) = ui.allocate_exact_size(vec2(width, 16.0 * k), Sense::hover());
            let y = r.center().y;
            let mut x = r.min.x;
            while x < r.max.x {
                ui.painter().line_segment([pos2(x, y), pos2((x + 6.0).min(r.max.x), y)], Stroke::new(1.0, Color32::from_gray(185)));
                x += 11.0;
            }
            let label = format!("{}  {}", ic::STEP, crate::i18n::t("Pause – nächster Schritt"));
            let g = ui.painter().layout_no_wrap(label, FontId::proportional(9.5 * k.max(0.9)), Color32::from_gray(120));
            let pill = Rect::from_center_size(r.center(), g.size() + vec2(14.0, 4.0));
            ui.painter().rect_filled(pill, 8.0, Color32::from_gray(244));
            ui.painter().galley(pill.center() - g.size() / 2.0, g, Color32::from_gray(120));
        }
        Elem::Toc => {
            let accent = if cx.tug { TUG_BLUE } else { TUG_BLUE };
            let secs = cx.sections.clone();
            if secs.is_empty() {
                ui.label(egui::RichText::new(crate::i18n::t("Inhaltsverzeichnis (Abschnitte erscheinen hier)")).size(12.0 * k).color(Color32::from_gray(140)));
            }
            for (i, s) in secs.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0 * k, 16.0 * k), Sense::hover());
                    ui.painter().rect_filled(r, 0.0, accent);
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, (i + 1).to_string(), FontId::proportional(10.0 * k), Color32::WHITE);
                    ui.add_space(6.0 * k);
                    ui.label(egui::RichText::new(s).size(14.0 * k).color(INK));
                });
                ui.add_space(4.0 * k);
            }
        }
        Elem::TitlePage => {
            // edits go to \title, \author, \date in the preamble
            for (cmd, size, color, hint) in [("title", 21.0, TUG_BLUE, "Titel der Präsentation"), ("subtitle", 14.0, TUG_BLUE, ""), ("author", 13.0, INK, "Name"), ("institute", 10.0, Color32::from_gray(90), ""), ("date", 11.0, Color32::from_gray(90), "Datum")] {
                let Some(v) = cx.meta.get(cmd).cloned() else { continue };
                let mut t = v.clone();
                let fs = FieldStyle { size: size * k, color, bold: false, mono: false, hint, enter_new: true };
                field(ui, cx, Id::new(("slide-meta", cmd)), path, &mut t, &fs, width);
                if t != v {
                    cx.ops.push(Op::Meta(cmd, t));
                }
                ui.add_space(if cmd == "title" || cmd == "subtitle" { 10.0 * k } else { 4.0 * k });
            }
        }
        Elem::Raw(r) => {
            let start = ui.cursor().min;
            let bg = ui.painter().add(egui::Shape::Noop);
            ui.add_space(4.0 * k);
            ui.horizontal(|ui| {
                ui.add_space(6.0 * k);
                let fs = FieldStyle { size: 10.5 * k, color: Color32::from_gray(70), bold: false, mono: true, hint: "LaTeX", enter_new: false };
                field(ui, cx, fid(0), path, r, &fs, width - 12.0 * k);
            });
            ui.add_space(4.0 * k);
            let rr = Rect::from_min_max(start, pos2(start.x + width, ui.cursor().min.y));
            ui.painter().set(bg, egui::Shape::Vec(vec![
                egui::Shape::rect_filled(rr, 5.0, Color32::from_gray(246)),
                egui::Shape::rect_stroke(rr, 5.0, Stroke::new(1.0, Color32::from_gray(225)), StrokeKind::Inside),
            ]));
            ui.painter().text(pos2(rr.max.x - 6.0, rr.min.y + 3.0), Align2::RIGHT_TOP, "LaTeX", FontId::monospace(8.0 * k.max(0.9)), Color32::from_gray(160));
        }
    }
}

fn load_image(ctx: &egui::Context, ed: &mut SlideEditor, dir: &Path, root: &Path, p: &str) -> Option<egui::TextureHandle> {
    if p.is_empty() {
        return None;
    }
    let mut found = None;
    for base in [dir, root] {
        for ext in ["", ".png", ".jpg", ".jpeg"] {
            let f = base.join(format!("{p}{ext}"));
            if f.is_file() && !f.extension().is_some_and(|e| e == "pdf") {
                found = Some(f);
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let f = found?;
    let stamp = crate::pdfview::file_stamp(&f);
    if let Some((s, t)) = ed.images.get(&f) {
        if *s == stamp {
            return t.clone();
        }
    }
    let tex = image::open(&f).ok().map(|img| {
        let img = img.thumbnail(1400, 1400).to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        ctx.load_texture(format!("slide-img-{}", f.display()), ci, egui::TextureOptions::LINEAR)
    });
    ed.images.insert(f, (stamp, tex.clone()));
    tex
}

// ───────────────────────────── slide canvas ─────────────────────────────

fn canvas(ui: &mut Ui, cx: &mut Cx, slide: &mut Slide, number: usize, section: &str, rect: Rect) {
    let k = cx.k;
    let p = ui.painter().clone();
    // shadow + paper
    p.rect_filled(rect.translate(vec2(0.0, 6.0)).expand(2.0), 6.0, with_alpha(Color32::BLACK, 70));
    p.rect_filled(rect, 4.0, Color32::WHITE);
    let title_page = slide.body.iter().any(|e| matches!(e, Elem::TitlePage));
    let pad = 30.0 * k;
    let footer_h = 26.0 * k;
    let author = cx.meta.get("author").cloned().unwrap_or_default();
    let date = cx.meta.get("date").cloned().unwrap_or_default();
    let strip = |s: &str| s.replace("\\\\", " ").replace("\\today", "").replace(['{', '}'], "");
    if cx.tug {
        // TU Graz logo (simplified) top right
        let lx = rect.max.x - 58.0 * k;
        let ly = rect.min.y + 12.0 * k;
        let s = 7.0 * k;
        for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (0.0, 1.0), (1.0, -0.6), (3.0, 0.0), (1.5, 1.0)] {
            p.rect_filled(Rect::from_min_size(pos2(lx + dx * s, ly + dy * s), vec2(s * 0.92, s * 0.92)), 0.0, TUG_RED);
        }
        p.text(pos2(lx + 4.3 * s, ly + 0.5 * s), Align2::LEFT_CENTER, "TU", FontId::new(10.0 * k, FontFamily::Name("ui-bold".into())), INK);
        p.text(pos2(lx + 4.3 * s, ly + 1.5 * s), Align2::LEFT_CENTER, "Graz", FontId::proportional(5.0 * k), INK);
    }
    if !title_page {
        // number box + section breadcrumb
        let nb = Rect::from_min_size(pos2(rect.min.x, rect.min.y + 10.0 * k), vec2(12.0 * k, 12.0 * k));
        p.rect_filled(nb, 0.0, TUG_BLUE);
        p.text(nb.center(), Align2::CENTER_CENTER, number.to_string(), FontId::proportional(7.0 * k), Color32::WHITE);
        if !section.is_empty() {
            p.text(pos2(rect.min.x + pad, rect.min.y + 10.0 * k), Align2::LEFT_TOP, section, FontId::proportional(7.5 * k), Color32::from_gray(90));
        }
        // footer
        let fr = Rect::from_min_max(pos2(rect.min.x, rect.max.y - footer_h), rect.max);
        p.rect_filled(fr, egui::CornerRadius { nw: 0, ne: 0, sw: 4, se: 4 }, Color32::from_gray(236));
        p.text(pos2(fr.min.x + pad, fr.center().y), Align2::LEFT_CENTER, format!("{}\n{}", strip(&author), strip(&date)), FontId::proportional(6.5 * k), Color32::from_gray(90));
    } else {
        // title page: soft backdrop
        let back = Rect::from_min_max(pos2(rect.min.x, rect.center().y), rect.max);
        p.rect_filled(back, egui::CornerRadius { nw: 0, ne: 0, sw: 4, se: 4 }, Color32::from_gray(242));
        p.text(pos2(rect.max.x - 14.0 * k, rect.min.y + 34.0 * k), Align2::RIGHT_TOP, "SCIENCE\nPASSION\nTECHNOLOGY", FontId::proportional(5.5 * k), Color32::from_gray(70));
    }
    let inner_w = rect.width() - 2.0 * pad - 30.0 * k;
    let content_top = if title_page { rect.min.y + rect.height() * 0.33 } else { rect.min.y + 26.0 * k };
    let content = Rect::from_min_max(pos2(rect.min.x + pad, content_top), pos2(rect.max.x - pad, rect.max.y - footer_h - 6.0 * k));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(Align::Min)));
    child.set_clip_rect(rect.expand2(vec2(60.0, 0.0)));
    child.spacing_mut().item_spacing.y = 2.0;
    child.visuals_mut().selection.bg_fill = with_alpha(TUG_BLUE, 70);
    child.visuals_mut().text_cursor.stroke = Stroke::new(2.0, INK);
    if !title_page {
        let fs = FieldStyle { size: 21.0 * k, color: INK, bold: false, mono: false, hint: "Folientitel", enter_new: true };
        let id = Id::new(("slide-title", cx.frame));
        field(&mut child, cx, id, &[], &mut slide.title, &fs, inner_w);
        child.add_space(12.0 * k);
    }
    elems(&mut child, cx, &mut slide.body, &[], inner_w);
    if slide.body.is_empty() {
        child.label(egui::RichText::new(crate::i18n::t("Leere Folie – oben Elemente hinzufügen")).size(12.0 * k).color(Color32::from_gray(160)));
    }
}

// ───────────────────────────── applying operations ─────────────────────────────

fn apply_ops(slide: &mut Slide, ops: Vec<Op>, frame: usize, ed: &mut SlideEditor, meta_out: &mut Vec<(&'static str, String)>, pick: &mut Option<Vec<usize>>) {
    for op in ops {
        match op {
            Op::MoveElem(path, d) => {
                if let Some((list, i)) = get_parent(&mut slide.body, &path) {
                    let j = i as i32 + d;
                    if j >= 0 && (j as usize) < list.len() {
                        list.swap(i, j as usize);
                    }
                }
            }
            Op::DeleteElem(path) => {
                if let Some((list, i)) = get_parent(&mut slide.body, &path) {
                    if i < list.len() {
                        list.remove(i);
                        // focus the previous text field, if any
                        if i > 0 {
                            let mut p = path.clone();
                            *p.last_mut().unwrap() = i - 1;
                            if matches!(list.get(i - 1), Some(Elem::Text(_))) {
                                ed.focus_next = Some(field_id(frame, &p, 0));
                            }
                        }
                    }
                }
            }
            Op::InsertItem { path, after, level } => {
                if let Some(Elem::List { items, .. }) = get_elem(&mut slide.body, &path) {
                    items.insert(after + 1, Item { level, overlay: String::new(), text: String::new() });
                    ed.focus_next = Some(field_id(frame, &path, 1000 + after + 1));
                }
            }
            Op::DeleteItem { path, idx } => {
                let mut remove_list = false;
                if let Some(Elem::List { items, .. }) = get_elem(&mut slide.body, &path) {
                    items.remove(idx);
                    if items.is_empty() {
                        remove_list = true;
                    } else {
                        ed.focus_next = Some(field_id(frame, &path, 1000 + idx.saturating_sub(1)));
                    }
                }
                if remove_list {
                    if let Some((list, i)) = get_parent(&mut slide.body, &path) {
                        list.remove(i);
                    }
                }
            }
            Op::ItemLevel { path, idx, delta } => {
                if let Some(Elem::List { items, .. }) = get_elem(&mut slide.body, &path) {
                    let max = if idx == 0 { 0 } else { items[idx - 1].level + 1 };
                    let l = (items[idx].level as i32 + delta).clamp(0, max.min(3) as i32) as u8;
                    items[idx].level = l;
                    ed.focus_next = Some(field_id(frame, &path, 1000 + idx));
                }
            }
            Op::PickImage(path) => *pick = Some(path),
            Op::Meta(cmd, v) => meta_out.push((cmd, v)),
        }
    }
}

/// Copy an image into `<slides dir>/figures/` (if outside the project) and return the path
/// to use in \includegraphics (relative to the slides directory, without extension).
fn import_image(file: &Path, dir: &Path, root: &Path) -> Option<String> {
    let stem = file.file_stem()?.to_string_lossy().replace(' ', "-");
    let target_dir = dir.join("figures");
    let target = if file.starts_with(root) {
        file.to_path_buf()
    } else {
        std::fs::create_dir_all(&target_dir).ok()?;
        let t = target_dir.join(format!("{stem}.{}", file.extension()?.to_string_lossy().to_lowercase()));
        std::fs::copy(file, &t).ok()?;
        t
    };
    let rel = target.strip_prefix(dir).or_else(|_| target.strip_prefix(root)).ok()?;
    let mut s = rel.with_extension("").to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        s = stem;
    }
    Some(s)
}

// ───────────────────────────── main entry ─────────────────────────────

impl App {
    fn slides_buffer(&mut self) -> Option<usize> {
        let rel = self.project.config.slides_main.clone();
        if self.buffer_idx(&rel).is_none() {
            self.open_file(&rel, Tab::Slides);
        }
        self.buffer_idx(&rel)
    }

    /// Replace the slides source and keep the editor state consistent.
    fn commit_slides(&mut self, bi: usize, new_text: String, cursor_byte: usize, now: f64) {
        let ed = &mut self.slide_ed;
        let b = &mut self.buffers[bi];
        if b.text == new_text {
            return;
        }
        if now - ed.last_snapshot > 1.2 || ed.undo.is_empty() {
            ed.undo.push(b.text.clone());
            if ed.undo.len() > 200 {
                ed.undo.remove(0);
            }
        }
        ed.last_snapshot = now;
        ed.redo.clear();
        b.text = new_text;
        let c = b.text[..cursor_byte.min(b.text.len())].chars().count();
        b.cursor = c;
        b.sel_end = c;
        b.place_cursor(c, c);
        b.last_edit = now;
    }

    fn slides_undo(&mut self, bi: usize, redo: bool, now: f64) {
        let ed = &mut self.slide_ed;
        let (from, to) = if redo { (&mut ed.redo, &mut ed.undo) } else { (&mut ed.undo, &mut ed.redo) };
        if let Some(t) = from.pop() {
            let b = &mut self.buffers[bi];
            to.push(std::mem::replace(&mut b.text, t));
            let n = b.text.chars().count();
            b.cursor = b.cursor.min(n);
            b.sel_end = b.cursor;
            b.last_edit = now;
            ed.last_snapshot = 0.0;
        }
    }

    pub fn slide_editor_ui(&mut self, ui: &mut Ui, now: f64) {
        let pal = self.pal.clone();
        let Some(bi) = self.slides_buffer() else {
            return;
        };
        if let Some(id) = self.slide_ed.debug_focus.take() {
            self.slide_ed.focus_next = Some(id);
        }
        // parse (cached by content)
        let key = hash_str(&self.buffers[bi].text);
        if key != self.slide_ed.key {
            self.slide_ed.deck = slides::parse_deck(&self.buffers[bi].text);
            self.slide_ed.key = key;
        }
        let deck = self.slide_ed.deck.clone();
        let text = self.buffers[bi].text.clone();
        let cur_char = self.buffers[bi].target_cursor();
        let cur_byte = text.char_indices().nth(cur_char).map_or(text.len(), |(b, _)| b);
        let sel = deck.frames.iter().rposition(|f| f.start <= cur_byte).unwrap_or(0);
        if self.slide_ed.last_sel != Some(sel) {
            if let Some(f) = deck.frames.get(sel) {
                let line = text[..f.start].matches('\n').count() + 1;
                let rel = self.project.config.slides_main.clone();
                self.slides.sync_line = Some((rel, line, now - 1.0));
            }
            self.slide_ed.last_sel = Some(sel);
        }

        // ── toolbar ──
        let mut action: Option<&'static str> = None;
        let mut new_tpl: Option<Template> = None;
        let bar = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover()).0;
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 90)));
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(10.0, 5.0))).layout(egui::Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 4.0;
        if widgets::chip(&mut right, ic::EYE, crate::i18n::t("Visuell"), true, pal.accent, &pal).clicked() {}
        if widgets::chip(&mut right, ic::CODE, "Code", false, pal.accent, &pal).clicked() {
            self.settings.slides_visual = false;
            self.settings.save();
        }
        let left_rect = Rect::from_min_max(bar.min + vec2(10.0, 5.0), pos2(right.min_rect().min.x - 8.0, bar.max.y - 5.0));
        let mut bl = ui.new_child(egui::UiBuilder::new().max_rect(left_rect).layout(egui::Layout::left_to_right(Align::Center)));
        bl.set_clip_rect(left_rect.intersect(ui.clip_rect()));
        bl.spacing_mut().item_spacing.x = 2.0;
        let add = widgets::button(&mut bl, ic::PLUS, crate::i18n::t("Folie"), &pal, widgets::BtnKind::Primary);
        egui::Popup::menu(&add).show(|ui| {
            ui.set_min_width(200.0);
            for (tpl, icon, label) in [
                (Template::Bullets, ic::LIST_UL, "Aufzählung"),
                (Template::TwoColumns, ic::COLUMNS, "Zwei Spalten"),
                (Template::Image, ic::IMAGE, "Bildfolie"),
                (Template::Block, ic::STICKY, "Block / Kernaussage"),
                (Template::Section, ic::QUOTE, "Abschluss („Fragen?“)"),
                (Template::Blank, ic::SQUARE, "Leere Folie"),
            ] {
                if ui.button(format!("{icon}  {}", crate::i18n::t(label))).clicked() {
                    new_tpl = Some(tpl);
                    ui.close();
                }
            }
        });
        let sep = |ui: &mut Ui| {
            let r = ui.allocate_exact_size(vec2(10.0, 20.0), Sense::hover()).0;
            ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, pal.border));
        };
        sep(&mut bl);
        for (icon, tip, act) in [(ic::COPY, "Folie duplizieren", "dup"), (ic::ARROW_UP, "Folie nach vorne", "up"), (ic::ARROW_DOWN, "Folie nach hinten", "down"), (ic::TRASH, "Folie löschen", "del")] {
            if widgets::icon_button_sized(&mut bl, icon, crate::i18n::t(tip), &pal, false, 28.0).clicked() {
                action = Some(act);
            }
        }
        sep(&mut bl);
        for (icon, tip, act) in [
            (ic::ALIGN_LEFT, "Text hinzufügen", "text"),
            (ic::LIST_UL, "Aufzählung hinzufügen", "ul"),
            (ic::LIST_OL, "Nummerierte Liste hinzufügen", "ol"),
            (ic::STICKY, "Block hinzufügen", "block"),
            (ic::IMAGE, "Bild hinzufügen", "img"),
            (ic::FUNCTION, "Formel hinzufügen", "math"),
            (ic::COLUMNS, "Zwei Spalten hinzufügen", "cols"),
            (ic::STEP, "Pause (schrittweise aufdecken)", "pause"),
        ] {
            if widgets::icon_button_sized(&mut bl, icon, crate::i18n::t(tip), &pal, false, 28.0).clicked() {
                action = Some(act);
            }
        }
        sep(&mut bl);
        for (icon, tip, act) in [(ic::BOLD, "Fett (Strg+B)", "b"), (ic::ITALIC, "Kursiv (Strg+I)", "i"), (ic::BOLT, "Hervorheben (\\alert)", "alert")] {
            if widgets::icon_button_sized(&mut bl, icon, crate::i18n::t(tip), &pal, false, 28.0).clicked() {
                action = Some(act);
            }
        }
        let step_on = self.slide_ed.last_focus.as_ref().and_then(|(_, p, _)| {
            let mut s = deck.frames.get(sel)?.slide.clone();
            match get_elem(&mut s.body, p)? {
                Elem::List { step, .. } => Some(*step),
                _ => None,
            }
        });
        if let Some(on) = step_on {
            if widgets::icon_button_sized(&mut bl, ic::PLAY, crate::i18n::t("Liste schrittweise aufdecken"), &pal, on, 28.0).clicked() {
                action = Some("step");
            }
        }
        sep(&mut bl);
        if widgets::icon_button_sized(&mut bl, ic::UNDO, crate::i18n::t("Rückgängig (Strg+Z)"), &pal, false, 28.0).clicked() {
            action = Some("undo");
        }

        // keyboard: undo / redo when no field has focus
        let ctx = ui.ctx().clone();
        if !ctx.egui_wants_keyboard_input() {
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)) {
                action = Some("redo");
            } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
                action = Some("undo");
            }
        }

        // ── canvas ──
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, pal.crust);
        let Some(fr) = deck.frames.get(sel).cloned() else {
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(40.0)).layout(egui::Layout::top_down(Align::Center)));
            c.label(egui::RichText::new(crate::i18n::t("Noch keine Folien – mit „+ Folie“ die erste anlegen.")).color(pal.dim));
            if let Some(t) = new_tpl {
                let s = slides::write_slide(&slides::new_slide(t, deck.tug));
                let at = deck.doc_end;
                let new_text = format!("{}{s}\n\n{}", &text[..at], &text[at..]);
                self.commit_slides(bi, new_text, at, now);
            }
            return;
        };
        let ratio = if deck.wide { 9.0 / 16.0 } else { 3.0 / 4.0 };
        let avail = area.shrink2(vec2(56.0, 30.0));
        let mut w = avail.width().min(1100.0);
        if w * ratio > avail.height() - 30.0 {
            w = ((avail.height() - 30.0) / ratio).max(320.0);
        }
        let canvas_rect = Rect::from_min_size(pos2(avail.center().x - w / 2.0, avail.min.y + 8.0), vec2(w, w * ratio));
        let mut slide = fr.slide.clone();
        let meta: HashMap<&'static str, String> = deck.meta.iter().map(|m| (m.cmd, m.value.clone())).collect();
        let mut sections: Vec<String> = vec![];
        for f in &deck.frames {
            if !f.section.is_empty() && sections.last() != Some(&f.section) {
                sections.push(f.section.clone());
            }
        }
        let slides_main = self.project.config.slides_main.clone();
        let dir = self.project.root.join(&slides_main).parent().map(Path::to_path_buf).unwrap_or_else(|| self.project.root.clone());
        let root = self.project.root.clone();
        let mut ops;
        let focused;
        {
            let mut cx = Cx { k: w / 640.0, tug: deck.tug, frame: sel, ops: vec![], focused: None, ed: &mut self.slide_ed, dirs: (dir.clone(), root.clone()), sections, meta };
            egui::ScrollArea::vertical().id_salt("slide-canvas").auto_shrink([false, false]).show(ui, |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(area.width(), canvas_rect.height() + 60.0), Sense::hover());
                let cr = Rect::from_min_size(pos2(canvas_rect.min.x, r.min.y + 18.0), canvas_rect.size());
                canvas(ui, &mut cx, &mut slide, sel + 1, &fr.section, cr);
                // hint line under the slide
                let hint = crate::i18n::t("Enter: neuer Punkt · Tab / Umschalt+Tab: einrücken · Strg+B / Strg+I · Doppelklick auf Bild: ersetzen");
                ui.painter().text(pos2(cr.center().x, cr.max.y + 22.0), Align2::CENTER_CENTER, hint, widgets::ui_font(11.0), pal.dim);
            });
            ops = std::mem::take(&mut cx.ops);
            focused = cx.focused.clone();
        }
        if focused.is_none() && !ctx.memory(|m| m.focused().is_some()) {
            // keep last_focus for toolbar actions (buttons take the focus away)
        }

        // ── toolbar actions on the current slide ──
        let insert_at = |slide: &mut Slide, e: Elem, last: &Option<(Id, Vec<usize>, (usize, usize))>| -> Vec<usize> {
            if let Some((_, p, _)) = last {
                if let Some((list, i)) = get_parent(&mut slide.body, p) {
                    list.insert(i + 1, e);
                    let mut q = p.clone();
                    *q.last_mut().unwrap() = i + 1;
                    return q;
                }
            }
            slide.body.push(e);
            vec![slide.body.len() - 1]
        };
        let tug = deck.tug;
        let (ul, cols_env) = if tug { ("tugitemize", "tugcolumns") } else { ("itemize", "columns") };
        let last = self.slide_ed.last_focus.clone().filter(|_| true);
        let empty_item = || Item { level: 0, overlay: String::new(), text: String::new() };
        let mut slide_op: Option<&str> = None;
        match action {
            Some("text") => {
                let p = insert_at(&mut slide, Elem::Text(String::new()), &last);
                self.slide_ed.focus_next = Some(field_id(sel, &p, 0));
            }
            Some("ul") | Some("ol") => {
                let numbered = action == Some("ol");
                let env = if numbered { if tug { "tugenumerate" } else { "enumerate" } } else { ul };
                let p = insert_at(&mut slide, Elem::List { env: env.into(), numbered, step: false, items: vec![empty_item()] }, &last);
                self.slide_ed.focus_next = Some(field_id(sel, &p, 1000));
            }
            Some("block") => {
                let p = insert_at(&mut slide, Elem::Block { kind: BlockKind::Block, title: crate::i18n::t("Merke").into(), body: vec![Elem::Text(String::new())] }, &last);
                let mut q = p.clone();
                q.push(0);
                self.slide_ed.focus_next = Some(field_id(sel, &q, 0));
            }
            Some("img") => {
                let p = insert_at(&mut slide, Elem::Image { path: String::new(), height: 0.6 }, &last);
                ops.push(Op::PickImage(p));
            }
            Some("math") => {
                let p = insert_at(&mut slide, Elem::Math(String::new()), &last);
                self.slide_ed.focus_next = Some(field_id(sel, &p, 0));
            }
            Some("cols") => {
                let p = insert_at(&mut slide, Elem::Columns { env: cols_env.into(), cols: vec![vec![Elem::Text(String::new())], vec![Elem::Text(String::new())]] }, &last);
                let mut q = p.clone();
                q.extend([0, 0]);
                self.slide_ed.focus_next = Some(field_id(sel, &q, 0));
            }
            Some("pause") => {
                insert_at(&mut slide, Elem::Pause, &last);
            }
            Some("b") | Some("i") | Some("alert") => {
                if let Some((id, _, _)) = &last {
                    let pre = match action {
                        Some("b") => "\\textbf{",
                        Some("i") => "\\emph{",
                        _ => "\\alert{",
                    };
                    self.slide_ed.pending_wrap = Some((*id, pre, "}"));
                }
            }
            Some("step") => {
                if let Some((_, p, _)) = &last {
                    if let Some(Elem::List { step, .. }) = get_elem(&mut slide.body, p) {
                        *step = !*step;
                    }
                }
            }
            Some(a @ ("dup" | "up" | "down" | "del" | "undo" | "redo")) => slide_op = Some(a),
            _ => {}
        }
        let mut meta_edits = vec![];
        let mut pick = None;
        apply_ops(&mut slide, ops, sel, &mut self.slide_ed, &mut meta_edits, &mut pick);
        if let Some(p) = pick {
            let files = rfd::FileDialog::new().add_filter(crate::i18n::t("Bilder"), &["png", "jpg", "jpeg", "pdf"]).set_title(crate::i18n::t("Bild für die Folie wählen")).pick_file();
            if let Some(f) = files.and_then(|f| import_image(&f, &dir, &root)) {
                if let Some(Elem::Image { path, .. }) = get_elem(&mut slide.body, &p) {
                    *path = f;
                }
            }
        }

        // ── write back ──
        let mut new_text = text.clone();
        let mut cursor_at = fr.start;
        if slide != fr.slide {
            new_text.replace_range(fr.start..fr.end, &slides::write_slide(&slide));
        }
        // preamble metadata (title page) – ranges are before every frame
        let mut metas: Vec<(usize, usize, String)> = meta_edits.into_iter().filter_map(|(cmd, v)| deck.meta.iter().find(|m| m.cmd == cmd).map(|m| (m.range.0, m.range.1, v))).collect();
        metas.sort_by(|a, b| b.0.cmp(&a.0));
        for (a, b, v) in metas {
            let delta = v.len() as isize - (b - a) as isize;
            new_text.replace_range(a..b, &v);
            cursor_at = (cursor_at as isize + delta) as usize;
        }
        match slide_op {
            Some("undo") => return self.slides_undo(bi, false, now),
            Some("redo") => return self.slides_undo(bi, true, now),
            Some("dup") => {
                let s = slides::write_slide(&slide);
                new_text.insert_str(fr.end.min(new_text.len()), &format!("\n\n{s}"));
                let _ = s;
                // the duplicate starts after this frame
                let fr_end = fr.start + slides::write_slide(&slide).len();
                cursor_at = fr_end + 2;
                let _ = cursor_at;
                let new_deck = slides::parse_deck(&new_text);
                cursor_at = new_deck.frames.get(sel + 1).map_or(cursor_at, |f| f.start);
            }
            Some("del") => {
                let end = new_text[fr.start..].find("\\end{frame}").map_or(new_text.len(), |p| fr.start + p + "\\end{frame}".len());
                let mut e = end;
                while new_text[e..].starts_with('\n') {
                    e += 1;
                }
                new_text.replace_range(fr.start..e, "");
                let nd = slides::parse_deck(&new_text);
                cursor_at = nd.frames.get(sel.saturating_sub(1).min(nd.frames.len().saturating_sub(1))).map_or(0, |f| f.start);
            }
            Some(dir_op @ ("up" | "down")) => {
                let nd = slides::parse_deck(&new_text);
                let j = if dir_op == "up" { sel.checked_sub(1) } else { (sel + 1 < nd.frames.len()).then_some(sel + 1) };
                if let Some(j) = j {
                    let (a, b) = if j < sel { (&nd.frames[j], &nd.frames[sel]) } else { (&nd.frames[sel], &nd.frames[j]) };
                    let (ta, tb) = (new_text[a.start..a.end].to_string(), new_text[b.start..b.end].to_string());
                    let (a0, a1, b0, b1) = (a.start, a.end, b.start, b.end);
                    new_text.replace_range(b0..b1, &ta);
                    new_text.replace_range(a0..a1, &tb);
                    let nd2 = slides::parse_deck(&new_text);
                    cursor_at = nd2.frames.get(j).map_or(a0, |f| f.start);
                }
            }
            _ => {}
        }
        if let Some(t) = new_tpl {
            let s = slides::write_slide(&slides::new_slide(t, tug));
            let at = new_text[fr.start..].find("\\end{frame}").map_or(new_text.len(), |p| fr.start + p + "\\end{frame}".len());
            new_text.insert_str(at, &format!("\n\n{s}"));
            let nd = slides::parse_deck(&new_text);
            cursor_at = nd.frames.get(sel + 1).map_or(at, |f| f.start);
            // focus the new title
            self.slide_ed.focus_next = Some(Id::new(("slide-title", sel + 1)));
        }
        if new_text != text {
            self.commit_slides(bi, new_text, cursor_at, now);
        }
        let _ = focused;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_paths() {
        let mut body = vec![
            Elem::Text("a".into()),
            Elem::Columns { env: "columns".into(), cols: vec![vec![Elem::Text("l".into())], vec![Elem::Block { kind: BlockKind::Block, title: "t".into(), body: vec![Elem::Text("x".into())] }]] },
        ];
        assert_eq!(get_elem(&mut body, &[1, 1, 0, 0]), Some(&mut Elem::Text("x".into())));
        let (list, i) = get_parent(&mut body, &[1, 0, 0]).unwrap();
        assert_eq!((list.len(), i), (1, 0));
        let mut t = "hello world".to_string();
        assert_eq!(wrap_sel(&mut t, (6, 11), "\\textbf{", "}"), (14, 19));
        assert_eq!(t, "hello \\textbf{world}");
    }

    #[test]
    fn inline_hides_markup() {
        let base = St { bold: false, italic: false, color: INK, mono: false };
        let job = inline_job("a \\textbf{b} \\% $x$", 14.0, base, TUG_RED, false, 300.0, false);
        let visible: String = job.sections.iter().filter(|s| s.format.color != Color32::TRANSPARENT).map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0]).collect();
        assert_eq!(visible, "a b % x");
    }
}
