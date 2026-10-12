//! Visual slide editor ("like PowerPoint").
//!
//! The current Beamer frame is edited as a working copy (draft) of the slide model; every
//! change rewrites only that frame with clean LaTeX (see `slides.rs`). Elements can be
//! selected (outline + floating tool bar), text fields behave like a real editor (Enter
//! splits, Backspace joins, arrows move between fields), and all structural edits are pure,
//! unit-tested functions in `slides.rs`.

use crate::app::{App, Tab};
use crate::icons as ic;
use crate::slides::{self, BlockKind, Deck, Elem, Item, ListResult, Rules, Slide, Template};
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
const SEL: Color32 = Color32::from_rgb(0x3b, 0x82, 0xf6);

/// Caret value for focus requests: select the whole text (placeholder titles).
const SELECT_ALL: usize = usize::MAX - 1;

// field sub-ids
const SUB_MAIN: usize = 0;
const SUB_BLOCK_TITLE: usize = 1;
const SUB_TITLE: usize = 9;
const SUB_META: usize = 100;
const SUB_ITEM: usize = 1000;
const SUB_CELL: usize = 100_000;

#[derive(Clone, Debug)]
struct TextFocus {
    id: Id,
    sel: (usize, usize),
}

#[derive(Default)]
pub struct SlideEditor {
    pub debug_focus: Option<Id>,
    key: u64,
    deck: Deck,
    /// Working copy: (frame index, slide, hash of the source written from it).
    draft: Option<(usize, Slide, u64)>,
    /// Selected element on the current slide.
    sel: Option<Vec<usize>>,
    /// Focus a text field on the next frame: (field, caret; usize::MAX = end).
    focus_req: Option<(Id, usize)>,
    /// Last focused text field (tool bar actions use it after the click took the focus).
    last_text: Option<TextFocus>,
    /// Wrap the selection of a field (prefix, suffix); an empty suffix inserts at the caret.
    pending_wrap: Option<(Id, String, String)>,
    clipboard: Option<Elem>,
    images: HashMap<PathBuf, (u64, Option<egui::TextureHandle>)>,
    undo: Vec<String>,
    redo: Vec<String>,
    last_snapshot: f64,
    last_frame: Option<usize>,
    /// Caret on the first/last visual row of a field (previous frame) – for ↑/↓ between fields.
    caret_rows: HashMap<Id, (bool, bool)>,
    /// Text fields in visual order (previous frame).
    field_order: Vec<Id>,
    /// Element rectangles (previous frame) for click-to-select.
    elem_rects: HashMap<Vec<usize>, Rect>,
    /// Navigator drag: frame index.
    nav_drag: Option<usize>,
    cite_query: String,
    /// Section name being edited in the slide header (name, new section starting here).
    section_edit: Option<String>,
    section_new: bool,
    /// Element whose text field had the focus in the previous frame (egui drops the focus on
    /// Esc before the field sees the key – Esc then selects this element).
    editing: Option<Vec<usize>>,
    /// PDF the stage was last synced with (page count + load stamp)
    synced_pdf: (usize, u64),
}

impl SlideEditor {
    fn working_slide(&mut self, frame: usize, source_hash: u64, parsed: &Slide) -> Slide {
        match &self.draft {
            Some((f, s, h)) if *f == frame && *h == source_hash => s.clone(),
            _ => {
                self.draft = None;
                parsed.clone()
            }
        }
    }

    fn remember(&mut self, frame: usize, slide: Slide, source_hash: u64) {
        self.draft = Some((frame, slide, source_hash));
    }

    pub fn debug_section(&mut self, s: &str) {
        self.section_edit = Some(s.to_string());
    }

    pub fn debug_select(&mut self, p: Vec<usize>) {
        self.sel = Some(p);
    }

    pub fn debug_state(&self) -> String {
        format!("sel={:?} editing={:?} fields={} undo={}", self.sel, self.editing, self.field_order.len(), self.undo.len())
    }

    fn focus(&mut self, id: Id, caret: usize) {
        self.focus_req = Some((id, caret));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NewKind {
    Text,
    Bullets,
    Numbered,
    Block,
    Image,
    Columns,
    Table,
    Math,
    Pause,
}

impl NewKind {
    fn all() -> [NewKind; 9] {
        use NewKind::*;
        [Text, Bullets, Numbered, Block, Image, Columns, Table, Math, Pause]
    }
    fn icon(self) -> &'static str {
        match self {
            NewKind::Text => ic::ALIGN_LEFT,
            NewKind::Bullets => ic::LIST_UL,
            NewKind::Numbered => ic::LIST_OL,
            NewKind::Block => ic::STICKY,
            NewKind::Image => ic::IMAGE,
            NewKind::Columns => ic::COLUMNS,
            NewKind::Table => ic::TABLE,
            NewKind::Math => ic::FUNCTION,
            NewKind::Pause => ic::STEP,
        }
    }
    fn label(self) -> &'static str {
        match self {
            NewKind::Text => tr!("Text" | "Text"),
            NewKind::Bullets => tr!("Liste" | "List"),
            NewKind::Numbered => tr!("Nummeriert" | "Numbered"),
            NewKind::Block => tr!("Block" | "Block"),
            NewKind::Image => tr!("Bild" | "Image"),
            NewKind::Columns => tr!("Spalten" | "Columns"),
            NewKind::Table => tr!("Tabelle" | "Table"),
            NewKind::Math => tr!("Formel" | "Formula"),
            NewKind::Pause => tr!("Pause" | "Pause"),
        }
    }
}

#[derive(Clone, Debug)]
enum TableOp {
    AddRow,
    AddCol,
    DelRow,
    DelCol,
    Header,
}

/// Changes requested while drawing; applied afterwards in one place.
#[derive(Clone, Debug)]
enum Op {
    Select(Vec<usize>),
    Deselect,
    TextEnter(Vec<usize>, usize),
    TextBackspace(Vec<usize>),
    ItemEnter(Vec<usize>, usize, usize),
    ItemBackspace(Vec<usize>, usize),
    ItemIndent(Vec<usize>, usize, i32),
    /// toggle `\pause` before an item
    ItemPause(Vec<usize>, usize),
    /// Ctrl+Enter: new item that appears one step later
    ItemEnterStep(Vec<usize>, usize, usize),
    TocCols(Vec<usize>, u8),
    /// table cell navigation: (path, row, col, drow, dcol)
    CellMove(Vec<usize>, usize, usize, i32, i32),
    FocusSibling(Id, i32),
    Move(Vec<usize>, i32),
    Delete(Vec<usize>),
    Duplicate(Vec<usize>),
    InsertBefore(Vec<usize>, NewKind),
    InsertAfter(Option<Vec<usize>>, NewKind),
    PickImage(Vec<usize>),
    ImageSize(Vec<usize>, f32),
    ImageAlign(Vec<usize>, slides::ImgAlign),
    Meta(&'static str, String),
    Table(Vec<usize>, TableOp),
    ListNumbered(Vec<usize>),
    ListStep(Vec<usize>),
    BlockKind(Vec<usize>, BlockKind),
    TextStyle(Vec<usize>, Option<bool>, i32),
    ColumnsCount(Vec<usize>, i32),
}

struct Cx<'a> {
    /// element path of the focused text field (this frame)
    editing: Option<Vec<usize>>,
    /// scale: canvas width / 640
    k: f32,
    tug: bool,
    frame: usize,
    ops: Vec<Op>,
    ed: &'a mut SlideEditor,
    dirs: (PathBuf, PathBuf),
    toc: Vec<(u8, String)>,
    meta: HashMap<&'static str, String>,
    fields: Vec<Id>,
    rects: HashMap<Vec<usize>, Rect>,
    /// a text field of the slide has the keyboard focus this frame
    text_focus: bool,
    /// content area of the slide (for the "+" handles in the margin)
    content: Rect,
    /// new section name entered in the slide header (name, start a new section here)
    section_result: Option<(String, bool)>,
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

pub fn field_id(frame: usize, path: &[usize], sub: usize) -> Id {
    Id::new(("slide-field", frame, path.to_vec(), sub))
}

fn new_elem(kind: NewKind, tug: bool, booktabs: bool) -> Elem {
    let (ul, ol, cols) = if tug { ("tugitemize", "tugenumerate", "tugcolumns") } else { ("itemize", "enumerate", "columns") };
    let item = || Item { level: 0, overlay: String::new(), text: String::new(), pause: false };
    match kind {
        NewKind::Text => Elem::Text(String::new()),
        NewKind::Bullets => Elem::List { env: ul.into(), numbered: false, step: false, items: vec![item()] },
        NewKind::Numbered => Elem::List { env: ol.into(), numbered: true, step: false, items: vec![item()] },
        NewKind::Block => Elem::Block { kind: BlockKind::Block, title: tr!("Merke" | "Note").into(), body: vec![Elem::Text(String::new())] },
        NewKind::Image => Elem::Image { path: String::new(), height: 0.6, align: slides::ImgAlign::Center },
        NewKind::Columns => Elem::Columns { env: cols.into(), cols: vec![vec![Elem::Text(String::new())], vec![Elem::Text(String::new())]] },
        NewKind::Table => slides::new_table(booktabs),
        NewKind::Math => Elem::Math(String::new()),
        NewKind::Pause => Elem::Pause,
    }
}

/// The first text field of an element (to focus after inserting it).
fn first_field(frame: usize, path: &[usize], e: &Elem) -> Option<Id> {
    match e {
        Elem::Text(_) | Elem::Math(_) | Elem::Raw(_) => Some(field_id(frame, path, SUB_MAIN)),
        Elem::List { .. } => Some(field_id(frame, path, SUB_ITEM)),
        Elem::Block { body, .. } => {
            let mut p = path.to_vec();
            p.push(0);
            body.first().and_then(|b| first_field(frame, &p, b))
        }
        Elem::Columns { cols, .. } => {
            let mut p = path.to_vec();
            p.extend([0, 0]);
            cols.first().and_then(|c| c.first()).and_then(|b| first_field(frame, &p, b))
        }
        Elem::Table { .. } => Some(field_id(frame, path, SUB_CELL)),
        _ => None,
    }
}

// ───────────────────────────── fields ─────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnterMode {
    /// Enter inserts a line break (math, LaTeX code)
    Newline,
    /// Enter splits / creates the next element (paragraph, list item, title)
    Split,
}

struct FieldStyle {
    size: f32,
    color: Color32,
    bold: bool,
    mono: bool,
    hint: &'static str,
    enter: EnterMode,
    /// Tab / Shift+Tab are reported (list items, table cells)
    tabs: bool,
}

#[derive(Default)]
struct FieldOut {
    /// Enter at char position
    enter: Option<usize>,
    /// Ctrl+Enter at char position
    ctrl_enter: Option<usize>,
    /// Backspace at position 0 (no selection)
    backspace_start: bool,
    tab: bool,
    shift_tab: bool,
}

fn field(ui: &mut Ui, cx: &mut Cx, id: Id, path: &[usize], text: &mut String, fs: &FieldStyle, width: f32) -> FieldOut {
    let ctx = ui.ctx().clone();
    let mut out = FieldOut::default();
    cx.fields.push(id);
    let focused = ctx.memory(|m| m.has_focus(id));
    if focused {
        cx.text_focus = true;
        let sel = selection(&ctx, id, text);
        let collapsed = sel.0 == sel.1;
        let (first_row, last_row) = cx.ed.caret_rows.get(&id).copied().unwrap_or((true, true));
        let mut up = false;
        let mut down = false;
        let mut esc = false;
        ctx.input_mut(|i| {
            if fs.enter == EnterMode::Split && i.consume_key(Modifiers::NONE, Key::Enter) {
                out.enter = Some(sel.0);
            }
            if fs.tabs && fs.enter == EnterMode::Split && i.consume_key(Modifiers::COMMAND, Key::Enter) {
                out.ctrl_enter = Some(sel.0);
            }
            if collapsed && sel.0 == 0 && i.key_pressed(Key::Backspace) && !i.modifiers.any() {
                i.consume_key(Modifiers::NONE, Key::Backspace);
                out.backspace_start = true;
            }
            if fs.tabs {
                if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                    out.shift_tab = true;
                }
                if i.consume_key(Modifiers::NONE, Key::Tab) {
                    out.tab = true;
                }
            }
            if first_row && i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                up = true;
            }
            if last_row && i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                down = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                esc = true;
            }
        });
        // Enter with a selection replaces it
        if out.enter.is_some() && !collapsed {
            let (a, b) = sel;
            let c: Vec<char> = text.chars().collect();
            *text = c[..a].iter().chain(c[b.min(c.len())..].iter()).collect();
        }
        if up {
            cx.ops.push(Op::FocusSibling(id, -1));
        }
        if down {
            cx.ops.push(Op::FocusSibling(id, 1));
        }
        if esc {
            ctx.memory_mut(|m| m.surrender_focus(id));
            cx.ops.push(Op::Select(path.to_vec()));
        }
        let bold = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::B));
        let ital = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::I));
        if (bold || ital) && !fs.mono {
            let (pre, post) = if bold { ("\\textbf{", "}") } else { ("\\emph{", "}") };
            let (a, b) = wrap_sel(text, sel, pre, post);
            set_cursor(&ctx, id, a, b);
        }
        // LaTeX-special characters typed or pasted into a visual field are escaped
        if !fs.mono {
            let prev = if sel.0 > 0 { text.chars().nth(sel.0 - 1) } else { None };
            ctx.input_mut(|i| {
                for e in i.events.iter_mut() {
                    match e {
                        egui::Event::Text(t) => {
                            let mut cs = t.chars();
                            if let (Some(c), None) = (cs.next(), cs.next()) {
                                if let Some(esc) = slides::escape_typed(c, prev) {
                                    *t = esc.to_string();
                                }
                            }
                        }
                        egui::Event::Paste(t) => {
                            // pasted text: one line, specials escaped
                            let one: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
                            let mut o = String::new();
                            let mut p = ' ';
                            for ch in one.chars() {
                                match slides::escape_typed(ch, Some(p)) {
                                    Some(e) => o.push_str(e),
                                    None => o.push(ch),
                                }
                                p = ch;
                            }
                            *t = o;
                        }
                        _ => {}
                    }
                }
            });
        }
    }
    // tool bar actions on this field (the click took the focus away)
    if let Some((wid, pre, post)) = cx.ed.pending_wrap.clone() {
        if wid == id {
            cx.ed.pending_wrap = None;
            let n = text.chars().count();
            let sel = cx.ed.last_text.as_ref().filter(|l| l.id == id).map(|l| l.sel).unwrap_or((n, n));
            let (a, b) = wrap_sel(text, sel, &pre, &post);
            if post.is_empty() {
                set_cursor(&ctx, id, b, b);
            } else {
                set_cursor(&ctx, id, a, b);
            }
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }
    if let Some((fid, caret)) = cx.ed.focus_req {
        if fid == id {
            cx.ed.focus_req = None;
            ctx.memory_mut(|m| m.request_focus(id));
            let n = text.chars().count();
            if caret == SELECT_ALL {
                set_cursor(&ctx, id, 0, n);
            } else {
                let c = caret.min(n);
                set_cursor(&ctx, id, c, c);
            }
        }
    }
    let base = St { bold: fs.bold, italic: false, color: fs.color, mono: fs.mono };
    let accent = if cx.tug { TUG_RED } else { Color32::from_rgb(0xc0, 0x30, 0x30) };
    let center = slides::text_style(text).0;
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
        .hint_text(egui::RichText::new(crate::i18n::t(fs.hint)).color(with_alpha(fs.color, 80)).size(fs.size))
        .layouter(&mut layouter)
        .lock_focus(true);
    let output = te.show(ui);
    // (the response of a TextEdit with a layouter doesn't report the focus – ask by id)
    let has_focus = ctx.memory(|m| m.has_focus(id));
    if has_focus {
        cx.text_focus = true;
        cx.editing = Some(path.to_vec());
        let sel = selection(&ctx, id, text);
        cx.ed.last_text = Some(TextFocus { id, sel });
        if !focused || output.response.clicked() {
            cx.ops.push(Op::Select(path.to_vec()));
        }
        // remember whether the caret is on the first / last visual row
        if let Some(r) = output.cursor_range {
            let g = &output.galley;
            let cr = g.pos_from_cursor(r.primary);
            let first = g.rows.first().is_none_or(|row| cr.center().y <= row.pos.y + row.size.y);
            let last = g.rows.last().is_none_or(|row| cr.center().y >= row.pos.y);
            cx.ed.caret_rows.insert(id, (first, last));
        }
    }
    out
}

fn body_style(cx: &Cx) -> FieldStyle {
    FieldStyle { size: 15.0 * cx.k, color: INK, bold: false, mono: false, hint: "Text …", enter: EnterMode::Split, tabs: false }
}
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


// ───────────────────────────── elements ─────────────────────────────

/// Small "+" in the left margin between elements: opens the insert menu.
fn insert_handle(ui: &mut Ui, cx: &mut Cx, y: f32, id: Id, op: impl Fn(NewKind) -> Op) {
    let ctx = ui.ctx().clone();
    let pid = id.with("menu");
    let open = egui::Popup::is_id_open(&ctx, pid);
    let zone = Rect::from_min_max(pos2(cx.content.min.x - 34.0, y - 7.0), pos2(cx.content.max.x, y + 4.0));
    let hover = ui.rect_contains_pointer(zone);
    if !hover && !open {
        return;
    }
    let c = pos2(cx.content.min.x - 18.0, y - 1.5);
    let r = Rect::from_center_size(c, vec2(18.0, 18.0));
    let resp = ui.interact(r, id, Sense::click()).on_hover_text(tr!("Element hier einfügen" | "Insert element here"));
    ui.painter().line_segment([pos2(cx.content.min.x - 6.0, y - 1.5), pos2(cx.content.max.x, y - 1.5)], Stroke::new(1.5, with_alpha(SEL, 140)));
    ui.painter().circle_filled(c, 9.0, if resp.hovered() || open { SEL } else { mix(SEL, Color32::WHITE, 0.25) });
    ui.painter().text(c, Align2::CENTER_CENTER, ic::PLUS, FontId::proportional(10.0), Color32::WHITE);
    egui::Popup::menu(&resp).id(pid).show(|ui| {
        ui.set_min_width(170.0);
        for k in NewKind::all() {
            if ui.button(format!("{}  {}", k.icon(), k.label())).clicked() {
                cx.ops.push(op(k));
                ui.close();
            }
        }
    });
}

fn elems(ui: &mut Ui, cx: &mut Cx, list: &mut Vec<Elem>, prefix: &[usize], width: f32) {
    let n = list.len();
    for i in 0..n {
        let mut path = prefix.to_vec();
        path.push(i);
        let top = ui.cursor().min.y;
        // click on the element (outside its text fields) selects it – registered first so the
        // text fields on top still get their clicks
        if let Some(r) = cx.ed.elem_rects.get(&path).copied() {
            let resp = ui.interact(r, Id::new(("slide-elem", cx.frame, path.clone())), Sense::click());
            if resp.clicked() {
                if let Some(f) = ui.ctx().memory(|m| m.focused()) {
                    ui.ctx().memory_mut(|m| m.surrender_focus(f));
                }
                cx.ops.push(Op::Select(path.clone()));
            }
            if resp.double_clicked() {
                if let Some(Elem::Image { .. }) = list.get(i) {
                    cx.ops.push(Op::PickImage(path.clone()));
                }
            }
        }
        let p2 = path.clone();
        insert_handle(ui, cx, top, Id::new(("slide-ins", cx.frame, path.clone())), move |k| Op::InsertBefore(p2.clone(), k));
        elem(ui, cx, &mut list[i], &path, width);
        let bottom = ui.cursor().min.y;
        let x0 = ui.max_rect().min.x;
        cx.rects.insert(path.clone(), Rect::from_min_max(pos2(x0 - 4.0, top - 3.0), pos2(x0 + width + 4.0, bottom + 1.0)));
        ui.add_space(7.0 * cx.k);
        if i + 1 == n {
            let y = ui.cursor().min.y;
            let p3 = path.clone();
            insert_handle(ui, cx, y, Id::new(("slide-ins-end", cx.frame, prefix.to_vec())), move |k| Op::InsertAfter(Some(p3.clone()), k));
        }
    }
}

fn elem(ui: &mut Ui, cx: &mut Cx, e: &mut Elem, path: &[usize], width: f32) {
    let k = cx.k;
    let frame = cx.frame;
    let fid = |sub: usize| field_id(frame, path, sub);
    match e {
        Elem::Text(t) => {
            let o = field(ui, cx, fid(SUB_MAIN), path, t, &body_style(cx), width);
            if let Some(at) = o.enter {
                cx.ops.push(Op::TextEnter(path.to_vec(), at));
            }
            if o.backspace_start {
                cx.ops.push(Op::TextBackspace(path.to_vec()));
            }
        }
        Elem::List { numbered, items, step, .. } => {
            let mut counters = [0usize; 6];
            let any_pause = items.iter().any(|i| i.pause);
            let mut step_no = 1usize;
            let row_w = width;
            for idx in 0..items.len() {
                let level = items[idx].level.min(5) as usize;
                counters[level] += 1;
                for c in counters.iter_mut().skip(level + 1) {
                    *c = 0;
                }
                if items[idx].pause {
                    step_no += 1;
                    // divider: the following items appear one click later
                    let (r, _) = ui.allocate_exact_size(vec2(row_w, 13.0 * k), Sense::hover());
                    let y = r.center().y;
                    let mut x = r.min.x + level as f32 * 20.0 * k;
                    while x < r.max.x - 70.0 * k {
                        ui.painter().line_segment([pos2(x, y), pos2((x + 5.0).min(r.max.x), y)], Stroke::new(1.0, Color32::from_gray(195)));
                        x += 9.0;
                    }
                    let pill = Rect::from_min_size(pos2(r.max.x - 66.0 * k, y - 6.5 * k), vec2(64.0 * k, 13.0 * k));
                    let presp = ui.interact(pill, Id::new(("pause-pill", frame, path.to_vec(), idx)), Sense::click()).on_hover_text(tr!("Klick: Pause entfernen" | "Click: remove the pause"));
                    ui.painter().rect_filled(pill, 6.0, if presp.hovered() { mix(SEL, Color32::WHITE, 0.8) } else { Color32::from_gray(240) });
                    ui.painter().text(pill.center(), Align2::CENTER_CENTER, format!("⏸ {} {step_no}", tr!("Schritt" | "Step")), FontId::proportional(8.5 * k.max(0.9)), if presp.hovered() { SEL } else { Color32::from_gray(110) });
                    if presp.clicked() {
                        cx.ops.push(Op::ItemPause(path.to_vec(), idx));
                    }
                }
                let indent = level as f32 * 20.0 * k;
                let size = (15.0 - level as f32 * 1.5) * k;
                let numbered = *numbered;
                let step_all = *step;
                let row_top = ui.cursor().min.y;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(indent);
                    let (r, _) = ui.allocate_exact_size(vec2(18.0 * k, size * 1.35), Sense::hover());
                    let c = pos2(r.min.x + 6.0 * k, r.center().y);
                    let accent = if cx.tug { TUG_RED } else { TUG_BLUE };
                    if numbered {
                        let n = counters[level];
                        let label = match level % 3 {
                            0 => format!("{n}."),
                            1 => format!("{}.", (b'a' + ((n - 1) % 26) as u8) as char),
                            _ => format!("{}.", roman(n)),
                        };
                        ui.painter().text(pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(size * 0.95), accent);
                    } else if cx.tug {
                        let s = (5.5 - level as f32) * k;
                        ui.painter().rect_filled(Rect::from_center_size(c, vec2(s, s)), 0.0, if level == 0 { accent } else { mix(accent, Color32::WHITE, 0.35) });
                    } else {
                        ui.painter().circle_filled(c, 2.6 * k, accent);
                    }
                    let badge = if step_all {
                        format!("{} {}", ic::STEP, idx + 1)
                    } else if any_pause {
                        format!("{} {step_no}", ic::STEP)
                    } else if !items[idx].overlay.is_empty() {
                        format!("{} {}", ic::STEP, items[idx].overlay.trim_matches(['<', '>']))
                    } else {
                        String::new()
                    };
                    let fs = FieldStyle { size, color: INK, bold: false, mono: false, hint: "Aufzählungspunkt …", enter: EnterMode::Split, tabs: true };
                    let reserve = 30.0 * k + 22.0 * k;
                    let w = (width - indent - 18.0 * k - reserve).max(40.0);
                    let o = field(ui, cx, fid(SUB_ITEM + idx), path, &mut items[idx].text, &fs, w);
                    let (br, _) = ui.allocate_exact_size(vec2(30.0 * k, size * 1.2), Sense::hover());
                    if !badge.is_empty() {
                        let pill = Rect::from_center_size(br.center(), vec2(26.0 * k, size * 0.95));
                        ui.painter().rect_filled(pill, 6.0, Color32::from_gray(234));
                        ui.painter().text(pill.center(), Align2::CENTER_CENTER, badge, FontId::proportional(size * 0.55), Color32::from_gray(100));
                    }
                    // pause toggle (shown on hover / while editing the item)
                    let (pr, _) = ui.allocate_exact_size(vec2(22.0 * k, size * 1.2), Sense::hover());
                    let row = Rect::from_min_max(pos2(ui.max_rect().min.x, row_top), pos2(pr.max.x, pr.max.y));
                    let editing = ui.ctx().memory(|m| m.has_focus(fid(SUB_ITEM + idx)));
                    if (ui.rect_contains_pointer(row) || editing) && idx > 0 && !step_all {
                        let presp = ui.interact(pr, Id::new(("pause-btn", frame, path.to_vec(), idx)), Sense::click()).on_hover_text(if items[idx].pause { tr!("Pause vor diesem Punkt entfernen" | "Remove the pause before this item") } else { tr!("Pause: dieser Punkt erscheint einen Schritt später (Strg+Enter beim Anlegen)" | "Pause: this item appears one step later (Ctrl+Enter when creating)") });
                        let on = items[idx].pause;
                        ui.painter().rect_filled(pr.shrink(2.0), 5.0, if on { with_alpha(SEL, 60) } else if presp.hovered() { Color32::from_gray(228) } else { Color32::from_gray(242) });
                        ui.painter().text(pr.center(), Align2::CENTER_CENTER, "⏸", FontId::proportional(size * 0.6), if on { SEL } else { Color32::from_gray(110) });
                        if presp.clicked() {
                            cx.ops.push(Op::ItemPause(path.to_vec(), idx));
                        }
                    }
                    if let Some(at) = o.enter {
                        cx.ops.push(Op::ItemEnter(path.to_vec(), idx, at));
                    }
                    if let Some(at) = o.ctrl_enter {
                        cx.ops.push(Op::ItemEnterStep(path.to_vec(), idx, at));
                    }
                    if o.backspace_start {
                        cx.ops.push(Op::ItemBackspace(path.to_vec(), idx));
                    }
                    if o.tab {
                        cx.ops.push(Op::ItemIndent(path.to_vec(), idx, 1));
                    }
                    if o.shift_tab {
                        cx.ops.push(Op::ItemIndent(path.to_vec(), idx, -1));
                    }
                });
                ui.add_space(3.0 * k);
            }
        }
        Elem::Block { kind, title, body } => {
            let (head, fill) = block_colors(*kind);
            let pad = 7.0 * k;
            let start = ui.cursor().min;
            let bg = ui.painter().add(egui::Shape::Noop);
            ui.add_space(3.0 * k);
            ui.horizontal(|ui| {
                ui.add_space(pad);
                let fs = FieldStyle { size: 14.0 * k, color: Color32::WHITE, bold: false, mono: false, hint: "Titel", enter: EnterMode::Split, tabs: false };
                let o = field(ui, cx, fid(SUB_BLOCK_TITLE), path, title, &fs, width - 2.0 * pad);
                if o.enter.is_some() {
                    cx.ops.push(Op::FocusSibling(fid(SUB_BLOCK_TITLE), 1));
                }
            });
            ui.add_space(5.0 * k);
            let head_bottom = ui.cursor().min.y;
            ui.horizontal_top(|ui| {
                ui.add_space(pad);
                ui.vertical(|ui| {
                    if body.is_empty() {
                        body.push(Elem::Text(String::new()));
                    }
                    let saved = cx.content;
                    cx.content = Rect::from_min_max(pos2(ui.max_rect().min.x, saved.min.y), pos2(ui.max_rect().min.x + width - 2.0 * pad, saved.max.y));
                    elems(ui, cx, body, path, width - 2.0 * pad);
                    cx.content = saved;
                });
            });
            let r = Rect::from_min_max(start, pos2(start.x + width, ui.cursor().min.y));
            let hr = Rect::from_min_max(start, pos2(start.x + width, head_bottom));
            let rad = (3.0 * k) as u8;
            ui.painter().set(
                bg,
                egui::Shape::Vec(vec![
                    egui::Shape::rect_filled(r, rad, fill),
                    egui::Shape::rect_filled(hr, egui::CornerRadius { nw: rad, ne: rad, sw: 0, se: 0 }, head),
                ]),
            );
        }
        Elem::Image { path: p, height, align } => {
            let align = *align;
            let (dir, root) = cx.dirs.clone();
            let max_h = 300.0 * k * *height;
            let tex = load_image(ui.ctx(), cx.ed, &dir, &root, p);
            match &tex {
                Some(t) => {
                    let s = t.size_vec2();
                    let h = max_h.min(width * s.y / s.x);
                    let w = h * s.x / s.y;
                    let (row, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
                    let r = place(row, vec2(w, h), align);
                    ui.painter().image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    let (row, resp) = ui.allocate_exact_size(vec2(width, max_h.max(60.0 * k)), Sense::click());
                    let r = place(row, vec2((max_h * 1.5).min(width), row.height()), align);
                    let hov = resp.hovered();
                    ui.painter().rect_filled(r, 6.0, if hov { Color32::from_gray(232) } else { Color32::from_gray(242) });
                    ui.painter().rect_stroke(r, 6.0, Stroke::new(1.0, Color32::from_gray(200)), StrokeKind::Inside);
                    let label = if p.is_empty() { crate::i18n::t("Klicken, um ein Bild zu wählen").to_string() } else { format!("{p}  ({})", crate::i18n::t("nicht gefunden")) };
                    ui.painter().text(r.center() - vec2(0.0, 10.0 * k), Align2::CENTER_CENTER, ic::IMAGE, FontId::proportional(22.0 * k), Color32::from_gray(150));
                    ui.painter().text(r.center() + vec2(0.0, 14.0 * k), Align2::CENTER_CENTER, label, FontId::proportional(10.0 * k), Color32::from_gray(110));
                    if resp.clicked() {
                        cx.ops.push(Op::Select(path.to_vec()));
                        cx.ops.push(Op::PickImage(path.to_vec()));
                    }
                }
            }
        }
        Elem::Columns { cols, .. } => {
            let n = cols.len().max(1) as f32;
            let gap = 16.0 * k;
            let cw = (width - gap * (n - 1.0)) / n;
            let saved = cx.content;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (c, col) in cols.iter_mut().enumerate() {
                    ui.allocate_ui_with_layout(vec2(cw, 10.0), egui::Layout::top_down(Align::Min), |ui| {
                        ui.set_width(cw);
                        if col.is_empty() {
                            col.push(Elem::Text(String::new()));
                        }
                        // faint column guide
                        let r = ui.max_rect();
                        ui.painter().rect_stroke(Rect::from_min_size(r.min - vec2(3.0, 2.0), vec2(cw + 6.0, 4.0 + cx.ed.elem_rects.get(path).map_or(40.0, |x| x.height() - 6.0))), 4.0, Stroke::new(1.0, Color32::from_gray(236)), StrokeKind::Inside);
                        let mut p = path.to_vec();
                        p.push(c);
                        cx.content = Rect::from_min_max(pos2(r.min.x, saved.min.y), pos2(r.min.x + cw, saved.max.y));
                        elems(ui, cx, col, &p, cw);
                    });
                }
            });
            cx.content = saved;
        }
        Elem::Table { rows, header, rules, center, .. } => {
            let nr = rows.len();
            let nc = rows.first().map_or(1, Vec::len).max(1);
            let tw = (width * if *center { 0.9 } else { 1.0 }).min(width);
            let cw = tw / nc as f32;
            let x0 = ui.cursor().min.x + if *center { (width - tw) / 2.0 } else { 0.0 };
            let start_y = ui.cursor().min.y;
            let line = |ui: &Ui, y: f32, w: f32| ui.painter().line_segment([pos2(x0, y), pos2(x0 + tw, y)], Stroke::new(w, Color32::from_gray(60)));
            ui.add_space(4.0 * k);
            let mut row_ys = vec![];
            for r in 0..nr {
                let y = ui.cursor().min.y;
                row_ys.push(y);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(x0 - ui.cursor().min.x);
                    for c in 0..nc {
                        let bold = *header && r == 0;
                        let fs = FieldStyle { size: 13.0 * k, color: INK, bold, mono: false, hint: if r == 0 { "Spalte" } else { "" }, enter: EnterMode::Split, tabs: true };
                        ui.allocate_ui_with_layout(vec2(cw, 10.0), egui::Layout::top_down(Align::Min), |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(5.0 * k);
                                let o = field(ui, cx, fid(SUB_CELL + r * 256 + c), path, &mut rows[r][c], &fs, cw - 10.0 * k);
                                if o.enter.is_some() {
                                    cx.ops.push(Op::CellMove(path.to_vec(), r, c, 1, 0));
                                }
                                if o.tab {
                                    cx.ops.push(Op::CellMove(path.to_vec(), r, c, 0, 1));
                                }
                                if o.shift_tab {
                                    cx.ops.push(Op::CellMove(path.to_vec(), r, c, 0, -1));
                                }
                            });
                        });
                    }
                });
                ui.add_space(3.0 * k);
            }
            let end_y = ui.cursor().min.y;
            match rules {
                Rules::Booktabs => {
                    line(ui, start_y + 1.0, 1.6);
                    if *header && nr > 1 {
                        line(ui, row_ys[1] - 2.0, 0.8);
                    }
                    line(ui, end_y, 1.6);
                }
                Rules::Hline => {
                    for y in row_ys.iter().copied().chain([end_y]) {
                        line(ui, y - 1.0, 0.8);
                    }
                }
                Rules::None => {}
            }
            ui.add_space(3.0 * k);
        }
        Elem::Math(m) => {
            let start = ui.cursor().min;
            let bg = ui.painter().add(egui::Shape::Noop);
            ui.horizontal(|ui| {
                ui.add_space(8.0 * k);
                ui.label(egui::RichText::new("∑").size(14.0 * k).color(TUG_BLUE));
                let fs = FieldStyle { size: 13.0 * k, color: INK, bold: false, mono: true, hint: "Formel, z. B. E = mc^2", enter: EnterMode::Newline, tabs: false };
                field(ui, cx, fid(SUB_MAIN), path, m, &fs, width - 40.0 * k);
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
        Elem::Toc { cols } => {
            // like Beamer: numbered sections, subsections indented below them
            let toc = cx.toc.clone();
            if toc.is_empty() {
                ui.label(egui::RichText::new(crate::i18n::t("Inhaltsverzeichnis – Abschnitte erscheinen hier")).size(12.0 * k).color(Color32::from_gray(140)));
            }
            let ncols = (*cols).clamp(1, 3) as usize;
            // split into columns at section boundaries
            let n_sections = toc.iter().filter(|t| t.0 == 1).count().max(1);
            let per_col = n_sections.div_ceil(ncols);
            let mut columns: Vec<Vec<(u8, String, usize)>> = vec![vec![]; ncols];
            let mut sec_no = 0;
            for (lvl, t) in &toc {
                if *lvl == 1 {
                    sec_no += 1;
                }
                let col = ((sec_no.max(1) - 1) / per_col).min(ncols - 1);
                columns[col].push((*lvl, t.clone(), sec_no));
            }
            let cw = width / ncols as f32;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for col in columns {
                    ui.allocate_ui_with_layout(vec2(cw, 10.0), egui::Layout::top_down(Align::Min), |ui| {
                        ui.set_width(cw);
                        for (lvl, t, n) in col {
                            ui.horizontal(|ui| {
                                if lvl == 1 {
                                    let (r, _) = ui.allocate_exact_size(vec2(16.0 * k, 16.0 * k), Sense::hover());
                                    ui.painter().rect_filled(r, 0.0, TUG_BLUE);
                                    ui.painter().text(r.center(), Align2::CENTER_CENTER, n.to_string(), FontId::proportional(10.0 * k), Color32::WHITE);
                                    ui.add_space(6.0 * k);
                                    ui.label(egui::RichText::new(slides_plain(&t)).size(14.0 * k).color(INK));
                                } else {
                                    ui.add_space(26.0 * k);
                                    let (r, _) = ui.allocate_exact_size(vec2(5.0 * k, 12.0 * k), Sense::hover());
                                    ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(4.0 * k, 4.0 * k)), 0.0, mix(TUG_BLUE, Color32::WHITE, 0.3));
                                    ui.add_space(6.0 * k);
                                    ui.label(egui::RichText::new(slides_plain(&t)).size(11.5 * k).color(Color32::from_gray(70)));
                                }
                            });
                            ui.add_space(if lvl == 1 { 3.0 } else { 1.0 } * k);
                        }
                    });
                }
            });
        }
        Elem::TitlePage => {
            for (n, (cmd, size, color, hint)) in [("title", 21.0, TUG_BLUE, "Titel der Präsentation"), ("subtitle", 14.0, TUG_BLUE, ""), ("author", 13.0, INK, "Name"), ("institute", 10.0, Color32::from_gray(90), ""), ("date", 11.0, Color32::from_gray(90), "Datum")]
                .into_iter()
                .enumerate()
            {
                let Some(v) = cx.meta.get(cmd).cloned() else { continue };
                let mut t = v.clone();
                let fs = FieldStyle { size: size * k, color, bold: false, mono: false, hint, enter: EnterMode::Split, tabs: false };
                let id = field_id(frame, path, SUB_META + n);
                let o = field(ui, cx, id, path, &mut t, &fs, width);
                if o.enter.is_some() {
                    cx.ops.push(Op::FocusSibling(id, 1));
                }
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
                let fs = FieldStyle { size: 10.5 * k, color: Color32::from_gray(70), bold: false, mono: true, hint: "LaTeX", enter: EnterMode::Newline, tabs: false };
                field(ui, cx, fid(SUB_MAIN), path, r, &fs, width - 12.0 * k);
            });
            ui.add_space(4.0 * k);
            let rr = Rect::from_min_max(start, pos2(start.x + width, ui.cursor().min.y));
            ui.painter().set(
                bg,
                egui::Shape::Vec(vec![egui::Shape::rect_filled(rr, 5.0, Color32::from_gray(246)), egui::Shape::rect_stroke(rr, 5.0, Stroke::new(1.0, Color32::from_gray(225)), StrokeKind::Inside)]),
            );
            ui.painter().text(pos2(rr.max.x - 6.0, rr.min.y + 3.0), Align2::RIGHT_TOP, "LaTeX", FontId::monospace(8.0 * k.max(0.9)), Color32::from_gray(160));
        }
    }
}

fn block_colors(kind: BlockKind) -> (Color32, Color32) {
    match kind {
        BlockKind::Block => (SLATE, Color32::from_rgb(0xec, 0xeb, 0xe4)),
        BlockKind::Alert => (TUG_RED, Color32::from_rgb(0xfb, 0xe7, 0xea)),
        BlockKind::Example => (Color32::from_rgb(0x2e, 0x7d, 0x4f), Color32::from_rgb(0xe4, 0xf2, 0xe8)),
    }
}

// ───────────────────────────── floating tool bar ─────────────────────────────

/// One button of the floating tool bar; returns true when clicked.
fn fbtn(ui: &mut Ui, icon: &str, tip: &str, active: bool) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(28.0, 26.0), Sense::click());
    let resp = resp.on_hover_text(tip);
    let bg = if active { with_alpha(SEL, 90) } else if resp.hovered() { Color32::from_gray(70) } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(r, 6.0, bg);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, FontId::proportional(12.0), Color32::from_gray(235));
    resp.clicked()
}

fn fsep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, 20.0), Sense::hover());
    ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, Color32::from_gray(80)));
}

fn floating_toolbar(ctx: &egui::Context, cx: &mut Cx, slide: &Slide, path: &[usize], rect: Rect, canvas: Rect) {
    let Some(e) = slides::elem_ref(&slide.body, path) else { return };
    let above = rect.min.y - 38.0 > canvas.min.y;
    let pos = if above { pos2(rect.min.x, rect.min.y - 36.0) } else { pos2(rect.min.x, rect.max.y + 6.0) };
    let mut ops = vec![];
    egui::Area::new(Id::new(("slide-float", cx.frame)))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(0x2a, 0x2c, 0x33))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(4, 3))
                .shadow(egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: with_alpha(Color32::BLACK, 90) })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        let p = path.to_vec();
                        match e {
                            Elem::Text(t) => {
                                let (center, size, _) = slides::text_style(t);
                                if fbtn(ui, ic::ALIGN_LEFT, tr!("Linksbündig" | "Align left"), !center) {
                                    ops.push(Op::TextStyle(p.clone(), Some(false), size));
                                }
                                if fbtn(ui, ic::ALIGN_CENTER, tr!("Zentriert" | "Centered"), center) {
                                    ops.push(Op::TextStyle(p.clone(), Some(true), size));
                                }
                                if fbtn(ui, ic::MINUS, tr!("Kleiner" | "Smaller"), false) {
                                    ops.push(Op::TextStyle(p.clone(), None, (size - 1).max(-2)));
                                }
                                if fbtn(ui, ic::TEXT_HEIGHT, tr!("Größer" | "Larger"), size > 0) {
                                    ops.push(Op::TextStyle(p.clone(), None, (size + 1).min(4)));
                                }
                                fsep(ui);
                            }
                            Elem::List { numbered, step, .. } => {
                                if fbtn(ui, ic::LIST_UL, tr!("Aufzählung" | "Bullets"), !numbered) && *numbered {
                                    ops.push(Op::ListNumbered(p.clone()));
                                }
                                if fbtn(ui, ic::LIST_OL, tr!("Nummeriert" | "Numbered"), *numbered) && !numbered {
                                    ops.push(Op::ListNumbered(p.clone()));
                                }
                                if fbtn(ui, ic::PLAY, tr!("Punkte schrittweise aufdecken" | "Reveal items step by step"), *step) {
                                    ops.push(Op::ListStep(p.clone()));
                                }
                                fsep(ui);
                            }
                            Elem::Block { kind, .. } => {
                                for (bk, tip) in [(BlockKind::Block, tr!("Block" | "Block")), (BlockKind::Alert, tr!("Warnung" | "Alert")), (BlockKind::Example, tr!("Beispiel" | "Example"))] {
                                    let (r, resp) = ui.allocate_exact_size(vec2(24.0, 26.0), Sense::click());
                                    let resp = resp.on_hover_text(tip);
                                    ui.painter().circle_filled(r.center(), 7.0, block_colors(bk).0);
                                    if *kind == bk {
                                        ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.5, Color32::WHITE));
                                    }
                                    if resp.clicked() {
                                        ops.push(Op::BlockKind(p.clone(), bk));
                                    }
                                }
                                fsep(ui);
                            }
                            Elem::Image { align, .. } => {
                                for (a, icon, tip) in [
                                    (slides::ImgAlign::Left, ic::ALIGN_LEFT, tr!("Linksbündig" | "Align left")),
                                    (slides::ImgAlign::Center, ic::ALIGN_CENTER, tr!("Zentriert" | "Centered")),
                                    (slides::ImgAlign::Right, ic::ALIGN_RIGHT, tr!("Rechtsbündig" | "Align right")),
                                ] {
                                    if fbtn(ui, icon, tip, *align == a) {
                                        ops.push(Op::ImageAlign(p.clone(), a));
                                    }
                                }
                                fsep(ui);
                                if fbtn(ui, ic::IMAGE, tr!("Bild ersetzen" | "Replace image"), false) {
                                    ops.push(Op::PickImage(p.clone()));
                                }
                                if fbtn(ui, ic::MINUS, tr!("Kleiner" | "Smaller"), false) {
                                    ops.push(Op::ImageSize(p.clone(), -0.1));
                                }
                                if fbtn(ui, ic::PLUS, tr!("Größer" | "Larger"), false) {
                                    ops.push(Op::ImageSize(p.clone(), 0.1));
                                }
                                fsep(ui);
                            }
                            Elem::Toc { cols } => {
                                if fbtn(ui, "1", tr!("Einspaltig" | "One column"), *cols <= 1) {
                                    ops.push(Op::TocCols(p.clone(), 1));
                                }
                                if fbtn(ui, "2", tr!("Zweispaltig" | "Two columns"), *cols == 2) {
                                    ops.push(Op::TocCols(p.clone(), 2));
                                }
                                fsep(ui);
                            }
                            Elem::Columns { cols, .. } => {
                                if cols.len() < 3 && fbtn(ui, ic::PLUS, tr!("Spalte hinzufügen" | "Add column"), false) {
                                    ops.push(Op::ColumnsCount(p.clone(), 1));
                                }
                                if cols.len() > 1 && fbtn(ui, ic::MINUS, tr!("Letzte Spalte entfernen" | "Remove last column"), false) {
                                    ops.push(Op::ColumnsCount(p.clone(), -1));
                                }
                                fsep(ui);
                            }
                            Elem::Table { header, rows, .. } => {
                                let nc = rows.first().map_or(0, Vec::len);
                                for (icon, tip, op, show) in [
                                    ("+≡", tr!("Zeile hinzufügen" | "Add row"), TableOp::AddRow, true),
                                    ("+⫼", tr!("Spalte hinzufügen" | "Add column"), TableOp::AddCol, nc < 8),
                                    ("−≡", tr!("Letzte Zeile löschen" | "Delete last row"), TableOp::DelRow, rows.len() > 1),
                                    ("−⫼", tr!("Letzte Spalte löschen" | "Delete last column"), TableOp::DelCol, nc > 1),
                                ] {
                                    if show && fbtn(ui, icon, tip, false) {
                                        ops.push(Op::Table(p.clone(), op));
                                    }
                                }
                                if fbtn(ui, ic::BOLD, tr!("Kopfzeile" | "Header row"), *header) {
                                    ops.push(Op::Table(p.clone(), TableOp::Header));
                                }
                                fsep(ui);
                            }
                            _ => {}
                        }
                        if fbtn(ui, ic::ARROW_UP, tr!("Nach oben (Alt+↑)" | "Move up (Alt+↑)"), false) {
                            ops.push(Op::Move(p.clone(), -1));
                        }
                        if fbtn(ui, ic::ARROW_DOWN, tr!("Nach unten (Alt+↓)" | "Move down (Alt+↓)"), false) {
                            ops.push(Op::Move(p.clone(), 1));
                        }
                        if fbtn(ui, ic::COPY, tr!("Duplizieren (Strg+D)" | "Duplicate (Ctrl+D)"), false) {
                            ops.push(Op::Duplicate(p.clone()));
                        }
                        if fbtn(ui, ic::TRASH, tr!("Löschen (Entf)" | "Delete (Del)"), false) {
                            ops.push(Op::Delete(p.clone()));
                        }
                    });
                });
        });
    cx.ops.extend(ops);
}

// ───────────────────────────── slide canvas ─────────────────────────────

fn canvas(ui: &mut Ui, cx: &mut Cx, slide: &mut Slide, number: usize, section: &str, rect: Rect) {
    let k = cx.k;
    let p = ui.painter().clone();
    p.rect_filled(rect.translate(vec2(0.0, 6.0)).expand(2.0), 6.0, with_alpha(Color32::BLACK, 70));
    p.rect_filled(rect, 4.0, Color32::WHITE);
    let title_page = slide.body.iter().any(|e| matches!(e, Elem::TitlePage));
    let pad = 30.0 * k;
    let footer_h = 26.0 * k;
    let author = cx.meta.get("author").cloned().unwrap_or_default();
    let date = cx.meta.get("date").cloned().unwrap_or_default();
    let strip = |s: &str| s.replace("\\\\", " ").replace("\\today", "").replace(['{', '}'], "");
    if cx.tug {
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
        let nb = Rect::from_min_size(pos2(rect.min.x, rect.min.y + 10.0 * k), vec2(12.0 * k, 12.0 * k));
        p.rect_filled(nb, 0.0, TUG_BLUE);
        p.text(nb.center(), Align2::CENTER_CENTER, number.to_string(), FontId::proportional(7.0 * k), Color32::WHITE);
        // section breadcrumb – click to rename / start a new section here
        let sr = Rect::from_min_size(pos2(rect.min.x + pad - 3.0, rect.min.y + 6.0 * k), vec2(220.0 * k, 13.0 * k));
        if let Some(mut s) = cx.ed.section_edit.clone() {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sr));
            let id = Id::new(("slide-section", cx.frame));
            let r = child.add(egui::TextEdit::singleline(&mut s).id(id).font(FontId::proportional(8.5 * k)).text_color(INK).hint_text(tr!("Abschnitt (leer = keiner)" | "Section (empty = none)")).frame(egui::Frame::NONE.fill(Color32::from_gray(242))).desired_width(sr.width()));
            if !r.has_focus() && cx.ed.section_edit.as_deref() == Some("\u{0}") {
                // just opened
            }
            if r.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter)) {
                cx.section_result = Some((s.trim().to_string(), cx.ed.section_new));
                cx.ed.section_edit = None;
                cx.ed.section_new = false;
            } else {
                cx.ed.section_edit = Some(s);
                if !r.has_focus() {
                    r.request_focus();
                }
            }
        } else {
            let font = FontId::proportional(7.5 * k);
            let (txt, empty) = if section.is_empty() { (format!("+ {}", tr!("Abschnitt" | "Section")), true) } else { (section.to_string(), false) };
            let g = p.layout_no_wrap(txt, font.clone(), INK);
            let lr = Rect::from_min_size(sr.min, vec2(g.size().x + 8.0, sr.height()));
            let resp = ui.interact(lr, Id::new(("slide-section-btn", cx.frame)), Sense::click()).on_hover_text(if empty { tr!("Abschnitt beginnen" | "Start a section") } else { tr!("Abschnitt umbenennen" | "Rename section") });
            if resp.hovered() {
                p.rect_filled(lr.expand(1.0), 3.0, Color32::from_gray(238));
            }
            let col = Color32::from_gray(match (empty, resp.hovered()) {
                (true, false) => 175,
                (true, true) => 110,
                (false, false) => 90,
                (false, true) => 30,
            });
            p.galley(pos2(lr.min.x + 4.0, lr.center().y - g.size().y / 2.0), g, col);
            if resp.clicked() {
                cx.ed.section_edit = Some(section.to_string());
                cx.ed.section_new = empty;
            }
            // start a new section at this slide
            if !empty {
                let nr = Rect::from_min_size(pos2(lr.max.x + 4.0, sr.min.y), vec2(14.0 * k, sr.height()));
                let hover_zone = sr.union(nr).expand(4.0);
                if ui.rect_contains_pointer(hover_zone) {
                    let nresp = ui.interact(nr, Id::new(("slide-section-new", cx.frame)), Sense::click()).on_hover_text(tr!("Neuen Abschnitt ab dieser Folie beginnen" | "Start a new section at this slide"));
                    p.rect_filled(nr, 3.0, if nresp.hovered() { Color32::from_gray(225) } else { Color32::from_gray(240) });
                    p.text(nr.center(), Align2::CENTER_CENTER, "+", font.clone(), Color32::from_gray(80));
                    if nresp.clicked() {
                        cx.ed.section_edit = Some(String::new());
                        cx.ed.section_new = true;
                    }
                }
            }
        }
        let fr = Rect::from_min_max(pos2(rect.min.x, rect.max.y - footer_h), rect.max);
        p.rect_filled(fr, egui::CornerRadius { nw: 0, ne: 0, sw: 4, se: 4 }, Color32::from_gray(236));
        p.text(pos2(fr.min.x + pad, fr.center().y), Align2::LEFT_CENTER, format!("{}\n{}", strip(&author), strip(&date)), FontId::proportional(6.5 * k), Color32::from_gray(90));
    } else {
        let back = Rect::from_min_max(pos2(rect.min.x, rect.center().y), rect.max);
        p.rect_filled(back, egui::CornerRadius { nw: 0, ne: 0, sw: 4, se: 4 }, Color32::from_gray(242));
        if cx.tug {
            p.text(pos2(rect.max.x - 14.0 * k, rect.min.y + 34.0 * k), Align2::RIGHT_TOP, "SCIENCE\nPASSION\nTECHNOLOGY", FontId::proportional(5.5 * k), Color32::from_gray(70));
        }
    }
    let inner_w = rect.width() - 2.0 * pad - 30.0 * k;
    let content_top = if title_page { rect.min.y + rect.height() * 0.33 } else { rect.min.y + 26.0 * k };
    let content = Rect::from_min_max(pos2(rect.min.x + pad, content_top), pos2(rect.max.x - pad, rect.max.y - footer_h - 6.0 * k));
    cx.content = Rect::from_min_max(content.min, pos2(content.min.x + inner_w, content.max.y));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(Align::Min)));
    child.set_clip_rect(rect.expand2(vec2(40.0, 0.0)));
    child.spacing_mut().item_spacing.y = 2.0;
    child.visuals_mut().selection.bg_fill = with_alpha(TUG_BLUE, 70);
    child.visuals_mut().text_cursor.stroke = Stroke::new(2.0, INK);
    if !title_page {
        let fs = FieldStyle { size: 21.0 * k, color: INK, bold: false, mono: false, hint: "Folientitel", enter: EnterMode::Split, tabs: false };
        let id = field_id(cx.frame, &[], SUB_TITLE);
        let o = field(&mut child, cx, id, &[], &mut slide.title, &fs, inner_w);
        if o.enter.is_some() {
            // Enter in the title: into the first element, or start a paragraph
            match slide.body.first().and_then(|e| first_field(cx.frame, &[0], e)) {
                Some(f) => cx.ed.focus(f, 0),
                None => cx.ops.push(Op::InsertAfter(None, NewKind::Text)),
            }
        }
        child.add_space(12.0 * k);
    }
    if slide.body.is_empty() {
        // empty slide: big, obvious choices
        child.add_space(16.0 * k);
        child.vertical_centered(|ui| {
            ui.label(egui::RichText::new(tr!("Leere Folie – was soll drauf?" | "Empty slide – what goes on it?")).size(13.0 * k).color(Color32::from_gray(140)));
            ui.add_space(10.0 * k);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for kind in NewKind::all() {
                    if kind == NewKind::Pause {
                        continue;
                    }
                    let (r, resp) = ui.allocate_exact_size(vec2(84.0 * k.max(0.8), 58.0 * k.max(0.8)), Sense::click());
                    let hov = resp.hovered();
                    ui.painter().rect_filled(r, 8.0, if hov { mix(SEL, Color32::WHITE, 0.88) } else { Color32::from_gray(246) });
                    ui.painter().rect_stroke(r, 8.0, Stroke::new(1.0, if hov { SEL } else { Color32::from_gray(222) }), StrokeKind::Inside);
                    ui.painter().text(r.center() - vec2(0.0, 9.0), Align2::CENTER_CENTER, kind.icon(), FontId::proportional(16.0), if hov { SEL } else { Color32::from_gray(110) });
                    ui.painter().text(r.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, kind.label(), FontId::proportional(11.0), Color32::from_gray(80));
                    if resp.clicked() {
                        cx.ops.push(Op::InsertAfter(None, kind));
                    }
                }
            });
        });
    } else {
        elems(&mut child, cx, &mut slide.body, &[], inner_w);
    }
}

// ───────────────────────────── frame-level text operations ─────────────────────────────

/// Range of a frame including the blank lines after it.
fn frame_span(text: &str, start: usize, end: usize) -> (usize, usize) {
    let mut e = end;
    while text[e..].starts_with('\n') {
        e += 1;
    }
    (start, e)
}

/// Insert a slide after frame `i` (or before \end{document} when there are none).
fn insert_frame_after(text: &str, deck: &Deck, i: Option<usize>, s: &str) -> (String, usize) {
    let at = match i.and_then(|i| deck.frames.get(i)) {
        Some(f) => f.end,
        None => deck.frames.last().map_or(deck.doc_end, |f| f.end),
    };
    let ins = format!("\n\n{s}");
    let mut t = text.to_string();
    t.insert_str(at, &ins);
    (t, at + 2)
}

fn delete_frame(text: &str, deck: &Deck, i: usize) -> Option<(String, usize)> {
    let f = deck.frames.get(i)?;
    let (a, b) = frame_span(text, f.start, f.end);
    let mut t = text.to_string();
    t.replace_range(a..b, "");
    let d = slides::parse_deck(&t);
    let cursor = d.frames.get(i.min(d.frames.len().saturating_sub(1))).map_or(0, |f| f.start);
    Some((t, cursor))
}

/// Move frame `from` so it ends up at position `to` (0-based, in the final order).
fn move_frame(text: &str, deck: &Deck, from: usize, to: usize) -> Option<(String, usize)> {
    if from == to || from >= deck.frames.len() {
        return None;
    }
    let f = &deck.frames[from];
    let body = text[f.start..f.end].to_string();
    let (a, b) = frame_span(text, f.start, f.end);
    let mut t = text.to_string();
    t.replace_range(a..b, "");
    let d = slides::parse_deck(&t);
    let at = if to >= d.frames.len() { d.frames.last().map_or(d.doc_end, |x| x.end) } else { d.frames[to].start };
    let ins = if to >= d.frames.len() { format!("\n\n{body}") } else { format!("{body}\n\n") };
    t.insert_str(at, &ins);
    let d2 = slides::parse_deck(&t);
    Some((t, d2.frames.get(to.min(d2.frames.len().saturating_sub(1))).map_or(0, |x| x.start)))
}

// ───────────────────────────── App integration ─────────────────────────────

impl App {
    fn slides_buffer(&mut self) -> Option<usize> {
        let rel = self.project.config.slides_main.clone();
        if self.buffer_idx(&rel).is_none() {
            self.open_file(&rel, Tab::Slides);
        }
        self.buffer_idx(&rel)
    }

    /// Parse the deck (cached by content) and return the buffer index.
    fn slides_deck(&mut self) -> Option<usize> {
        let bi = self.slides_buffer()?;
        let key = hash_str(&self.buffers[bi].text);
        if key != self.slide_ed.key {
            self.slide_ed.deck = slides::parse_deck(&self.buffers[bi].text);
            self.slide_ed.key = key;
        }
        Some(bi)
    }

    fn current_frame(&self, bi: usize) -> usize {
        let b = &self.buffers[bi];
        let c = b.target_cursor();
        let byte = b.text.char_indices().nth(c).map_or(b.text.len(), |(i, _)| i);
        self.slide_ed.deck.frames.iter().rposition(|f| f.start <= byte).unwrap_or(0)
    }

    fn goto_frame(&mut self, bi: usize, i: usize) {
        if let Some(f) = self.slide_ed.deck.frames.get(i) {
            let b = &mut self.buffers[bi];
            let c = b.text[..f.start.min(b.text.len())].chars().count();
            b.cursor = c;
            b.sel_end = c;
            b.place_cursor(c, c);
        }
    }

    /// Replace the slides source; `structural` changes always get their own undo step.
    fn commit_slides(&mut self, bi: usize, new_text: String, cursor_byte: usize, now: f64, structural: bool) {
        let ed = &mut self.slide_ed;
        let b = &mut self.buffers[bi];
        if b.text == new_text {
            return;
        }
        if structural || now - ed.last_snapshot > 1.2 || ed.undo.is_empty() {
            ed.undo.push(b.text.clone());
            if ed.undo.len() > 300 {
                ed.undo.remove(0);
            }
        }
        ed.last_snapshot = if structural { 0.0 } else { now };
        ed.redo.clear();
        b.text = new_text;
        let c = b.text[..cursor_byte.min(b.text.len())].chars().count();
        b.cursor = c;
        b.sel_end = c;
        b.place_cursor(c, c);
        b.last_edit = now;
        ed.key = hash_str(&b.text);
        ed.deck = slides::parse_deck(&b.text);
    }

    fn slides_undo(&mut self, bi: usize, redo: bool, now: f64) {
        let frame = self.current_frame(bi);
        let ed = &mut self.slide_ed;
        let (from, to) = if redo { (&mut ed.redo, &mut ed.undo) } else { (&mut ed.undo, &mut ed.redo) };
        if let Some(t) = from.pop() {
            let b = &mut self.buffers[bi];
            to.push(std::mem::replace(&mut b.text, t));
            b.last_edit = now;
            ed.last_snapshot = 0.0;
            ed.draft = None;
            ed.key = hash_str(&b.text);
            ed.deck = slides::parse_deck(&b.text);
            ed.sel = None;
            // stay on the same slide
            let i = frame.min(ed.deck.frames.len().saturating_sub(1));
            if let Some(f) = ed.deck.frames.get(i) {
                let c = b.text[..f.start].chars().count();
                b.cursor = c;
                b.sel_end = c;
                b.place_cursor(c, c);
            }
        }
    }

    // ── navigator (left panel) ──

    pub fn slide_navigator(&mut self, ui: &mut Ui, now: f64) {
        let pal = self.pal.clone();
        let Some(bi) = self.slides_deck() else { return };
        let deck = self.slide_ed.deck.clone();
        let cur = self.current_frame(bi);
        let text = self.buffers[bi].text.clone();
        let mut new_after: Option<usize> = None;
        let mut action: Option<(&str, usize)> = None;
        let mut goto: Option<usize> = None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr!("FOLIEN" | "SLIDES")).font(widgets::ui_font(11.0)).color(pal.dim).extra_letter_spacing(1.2));
            ui.label(egui::RichText::new(deck.frames.len().to_string()).font(widgets::ui_font(11.0)).color(pal.dim));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button_sized(ui, ic::PLUS, tr!("Neue Folie (Strg+M)" | "New slide (Ctrl+M)"), &pal, false, 24.0).clicked() {
                    new_after = Some(cur);
                }
            });
        });
        ui.add_space(6.0);
        let ratio = if deck.wide { 9.0 / 16.0 } else { 3.0 / 4.0 };
        let mut rects: Vec<Rect> = vec![];
        let drag = self.slide_ed.nav_drag;
        let pointer = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()));
        egui::ScrollArea::vertical().id_salt("slide-nav").auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width();
            for (i, f) in deck.frames.iter().enumerate() {
                if f.section_here.is_some() && !f.section.is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(f.section.to_uppercase()).font(widgets::ui_font(10.0)).color(pal.accent).extra_letter_spacing(1.0));
                }
                let tw = w - 26.0;
                let th = tw * ratio;
                let (rect, resp) = ui.allocate_exact_size(vec2(w, th + 10.0), Sense::click_and_drag());
                rects.push(rect);
                let card = Rect::from_min_size(pos2(rect.min.x + 22.0, rect.min.y + 3.0), vec2(tw - 2.0, th));
                let sel = i == cur;
                ui.painter().text(pos2(rect.min.x + 9.0, card.min.y + 8.0), Align2::CENTER_CENTER, (i + 1).to_string(), widgets::ui_font(11.0), if sel { pal.accent } else { pal.dim });
                if ui.is_rect_visible(rect) {
                    mini_slide(ui.painter(), card, &f.slide, deck.tug);
                    if sel {
                        ui.painter().rect_stroke(card.expand(3.0), 6.0, Stroke::new(2.0, pal.accent), StrokeKind::Outside);
                    } else if resp.hovered() {
                        ui.painter().rect_stroke(card.expand(3.0), 6.0, Stroke::new(1.5, with_alpha(pal.text, 60)), StrokeKind::Outside);
                    }
                }
                if resp.clicked() {
                    goto = Some(i);
                }
                if resp.drag_started() {
                    self.slide_ed.nav_drag = Some(i);
                }
                resp.context_menu(|ui| {
                    ui.set_min_width(190.0);
                    for (icon, label, act) in [
                        (ic::PLUS, tr!("Neue Folie danach" | "New slide after"), "new"),
                        (ic::COPY, tr!("Duplizieren" | "Duplicate"), "dup"),
                        (ic::ARROW_UP, tr!("Nach vorne" | "Move up"), "up"),
                        (ic::ARROW_DOWN, tr!("Nach hinten" | "Move down"), "down"),
                        (ic::TRASH, tr!("Löschen" | "Delete"), "del"),
                    ] {
                        if ui.button(format!("{icon}  {label}")).clicked() {
                            action = Some((act, i));
                            ui.close();
                        }
                    }
                });
            }
            ui.add_space(8.0);
            if widgets::button(ui, ic::PLUS, tr!("Neue Folie" | "New slide"), &pal, widgets::BtnKind::Ghost).clicked() {
                new_after = Some(deck.frames.len().saturating_sub(1));
            }
        });
        // drag & drop reordering: insertion line + drop
        let mut drop_to: Option<(usize, usize)> = None;
        if let (Some(from), Some(p)) = (drag, pointer) {
            let target = rects.iter().position(|r| p.y < r.center().y).unwrap_or(rects.len());
            if let Some(r) = rects.get(target).or(rects.last()) {
                let y = if target < rects.len() { r.min.y } else { r.max.y };
                ui.painter().line_segment([pos2(r.min.x + 18.0, y), pos2(r.max.x, y)], Stroke::new(2.5, pal.accent));
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if !ui.input(|i| i.pointer.any_down()) {
                self.slide_ed.nav_drag = None;
                let to = if target > from { target - 1 } else { target };
                if to != from {
                    drop_to = Some((from, to));
                }
            }
        }
        if let Some(i) = goto {
            self.goto_frame(bi, i);
        }
        let mut edit: Option<(String, usize)> = None;
        if let Some(i) = new_after {
            let s = slides::write_slide(&slides::new_slide(Template::Bullets, deck.tug));
            edit = Some(insert_frame_after(&text, &deck, if deck.frames.is_empty() { None } else { Some(i) }, &s));
            self.slide_ed.focus_req = Some((field_id(i + 1, &[], SUB_TITLE), SELECT_ALL));
        }
        if let Some((from, to)) = drop_to {
            edit = move_frame(&text, &deck, from, to);
        }
        if let Some((act, i)) = action {
            edit = match act {
                "new" => {
                    let s = slides::write_slide(&slides::new_slide(Template::Bullets, deck.tug));
                    Some(insert_frame_after(&text, &deck, Some(i), &s))
                }
                "dup" => {
                    let f = &deck.frames[i];
                    Some(insert_frame_after(&text, &deck, Some(i), &text[f.start..f.end]))
                }
                "up" if i > 0 => move_frame(&text, &deck, i, i - 1),
                "down" => move_frame(&text, &deck, i, i + 1),
                "del" => delete_frame(&text, &deck, i),
                _ => None,
            };
        }
        if let Some((t, c)) = edit {
            self.slide_ed.draft = None;
            self.commit_slides(bi, t, c, now, true);
        }
    }
}

/// Miniature of a slide for the navigator.
fn mini_slide(p: &egui::Painter, r: Rect, s: &Slide, tug: bool) {
    p.rect_filled(r, 3.0, Color32::WHITE);
    let k = r.width() / 200.0;
    if tug {
        p.rect_filled(Rect::from_min_size(pos2(r.max.x - 18.0 * k, r.min.y + 5.0 * k), vec2(12.0 * k, 5.0 * k)), 0.0, TUG_RED);
    }
    if s.body.iter().any(|e| matches!(e, Elem::TitlePage)) {
        p.rect_filled(Rect::from_min_max(pos2(r.min.x, r.center().y), r.max), 3.0, Color32::from_gray(238));
        p.rect_filled(Rect::from_min_size(pos2(r.min.x + 12.0 * k, r.center().y - 14.0 * k), vec2(110.0 * k, 7.0 * k)), 2.0, TUG_BLUE);
        p.rect_filled(Rect::from_min_size(pos2(r.min.x + 12.0 * k, r.center().y - 3.0 * k), vec2(80.0 * k, 5.0 * k)), 2.0, mix(TUG_BLUE, Color32::WHITE, 0.4));
        return;
    }
    let mut y = r.min.y + 9.0 * k;
    let x = r.min.x + 10.0 * k;
    let w = r.width() - 20.0 * k;
    let title: String = s.title.chars().take(40).collect();
    let font = FontId::proportional((9.0 * k).max(6.0));
    if !title.is_empty() {
        p.with_clip_rect(r.shrink(2.0)).text(pos2(x, y), Align2::LEFT_TOP, slides_plain(&title), font.clone(), INK);
    }
    y += 16.0 * k;
    let line = |p: &egui::Painter, y: f32, frac: f32, c: Color32| p.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w * frac, 3.0 * k)), 1.5 * k, c);
    for e in s.body.iter().take(5) {
        if y > r.max.y - 10.0 * k {
            break;
        }
        match e {
            Elem::Text(_) | Elem::Math(_) | Elem::Raw(_) => {
                line(p, y, 0.8, Color32::from_gray(190));
                y += 7.0 * k;
            }
            Elem::List { items, .. } => {
                for it in items.iter().take(4) {
                    p.rect_filled(Rect::from_min_size(pos2(x + it.level as f32 * 6.0 * k, y), vec2(3.0 * k, 3.0 * k)), 0.0, if tug { TUG_RED } else { TUG_BLUE });
                    p.rect_filled(Rect::from_min_size(pos2(x + (6.0 + it.level as f32 * 6.0) * k, y), vec2(w * 0.6, 3.0 * k)), 1.5 * k, Color32::from_gray(190));
                    y += 7.0 * k;
                }
            }
            Elem::Block { kind, .. } => {
                let (h, f) = block_colors(*kind);
                p.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w, 6.0 * k)), 1.0, h);
                p.rect_filled(Rect::from_min_size(pos2(x, y + 6.0 * k), vec2(w, 10.0 * k)), 1.0, f);
                y += 20.0 * k;
            }
            Elem::Image { .. } => {
                p.rect_filled(Rect::from_min_size(pos2(x + w * 0.25, y), vec2(w * 0.5, 26.0 * k)), 2.0, Color32::from_gray(215));
                y += 30.0 * k;
            }
            Elem::Columns { .. } => {
                p.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w * 0.47, 22.0 * k)), 2.0, Color32::from_gray(232));
                p.rect_filled(Rect::from_min_size(pos2(x + w * 0.53, y), vec2(w * 0.47, 22.0 * k)), 2.0, Color32::from_gray(232));
                y += 26.0 * k;
            }
            Elem::Table { rows, .. } => {
                for _ in rows.iter().take(4) {
                    line(p, y, 1.0, Color32::from_gray(205));
                    y += 6.0 * k;
                }
            }
            Elem::Toc { .. } => {
                for _ in 0..3 {
                    p.rect_filled(Rect::from_min_size(pos2(x, y), vec2(4.0 * k, 4.0 * k)), 0.0, TUG_BLUE);
                    line(p, y, 0.5, Color32::from_gray(190));
                    y += 7.0 * k;
                }
            }
            Elem::Pause | Elem::TitlePage => {}
        }
    }
    p.rect_filled(Rect::from_min_max(pos2(r.min.x, r.max.y - 7.0 * k), r.max), 0.0, Color32::from_gray(236));
}

/// Title text without LaTeX markup (for thumbnails).
fn slides_plain(s: &str) -> String {
    let mut out = String::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' => {
                i += 1;
                if i < c.len() && !c[i].is_ascii_alphabetic() {
                    // \\ line break → space, \% → %
                    out.push(if c[i] == '\\' { ' ' } else { c[i] });
                    i += 1;
                } else {
                    while i < c.len() && c[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                }
            }
            '{' | '}' => i += 1,
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl App {
    pub fn slide_editor_ui(&mut self, ui: &mut Ui, now: f64) {
        let pal = self.pal.clone();
        let ctx = ui.ctx().clone();
        let Some(bi) = self.slides_deck() else { return };
        if let Some(id) = self.slide_ed.debug_focus.take() {
            self.slide_ed.focus_req = Some((id, usize::MAX));
        }
        let deck = self.slide_ed.deck.clone();
        let text = self.buffers[bi].text.clone();
        let key = self.slide_ed.key;
        let frame = self.current_frame(bi);
        if self.slide_ed.last_frame != Some(frame) {
            // new slide: reset selection, sync the PDF stage
            self.slide_ed.sel = None;
            self.slide_ed.section_edit = None;
            self.slide_ed.elem_rects.clear();
            if let Some(f) = deck.frames.get(frame) {
                // a line inside the frame (the \begin{frame} line itself maps to the page before)
                let line = (text[..f.start].matches('\n').count() + text[..f.end].matches('\n').count()) / 2 + 1;
                let rel = self.project.config.slides_main.clone();
                self.slides.sync_line = Some((rel, line, now - 1.0));
            }
            self.slide_ed.last_frame = Some(frame);
        }
        // re-sync the stage when a new PDF arrived (the first sync may have run before compiling)
        let pdf_key = (self.slides.viewer.page_count(), self.slides.viewer.doc.as_ref().map_or(0, |d| d.pages.len() as u64 ^ (self.slides.last_secs.to_bits() as u64)));
        if pdf_key.0 > 0 && pdf_key != self.slide_ed.synced_pdf {
            self.slide_ed.synced_pdf = pdf_key;
            if let Some(f) = deck.frames.get(frame) {
                let line = (text[..f.start].matches('\n').count() + text[..f.end].matches('\n').count()) / 2 + 1;
                let rel = self.project.config.slides_main.clone();
                self.slides.sync_line = Some((rel, line, now - 1.0));
            }
        }
        let fr = deck.frames.get(frame).cloned();
        let mut slide = fr.as_ref().map(|f| self.slide_ed.working_slide(frame, key, &f.slide)).unwrap_or(Slide { opts: String::new(), title: String::new(), body: vec![] });
        let base = slide.clone();
        let mut ops: Vec<Op> = vec![];
        let mut slide_op: Option<&'static str> = None;
        let mut new_tpl: Option<Template> = None;

        // ── keyboard (before the text fields see the keys) ──
        let our_focus = ctx.memory(|m| m.focused()).is_some_and(|f| self.slide_ed.field_order.contains(&f));
        let other_focus = ctx.memory(|m| m.focused()).is_some() && !our_focus;
        let (undo, redo) = ctx.input_mut(|i| {
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
            let undo = i.consume_key(Modifiers::COMMAND, Key::Z);
            (undo, redo)
        });
        if !other_focus && (undo || redo) {
            self.slides_undo(bi, redo, now);
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::M)) && !other_focus {
            new_tpl = Some(Template::Bullets);
        }
        // Esc while typing: egui already dropped the focus – select the element instead
        if !our_focus && !other_focus {
            if let Some(p) = self.slide_ed.editing.take() {
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    self.slide_ed.sel = Some(p);
                }
            }
        }
        if !our_focus && !other_focus {
            let sel = self.slide_ed.sel.clone();
            let mut copy = false;
            let mut cut = false;
            let mut paste = false;
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Copy => {
                        copy = true;
                        false
                    }
                    egui::Event::Cut => {
                        cut = true;
                        false
                    }
                    egui::Event::Paste(_) => {
                        paste = true;
                        false
                    }
                    _ => true,
                });
                if i.consume_key(Modifiers::COMMAND, Key::V) {
                    paste = true;
                }
            });
            if let Some(p) = &sel {
                let k = ctx.input_mut(|i| {
                    (
                        i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace),
                        i.consume_key(Modifiers::COMMAND, Key::D),
                        i.consume_key(Modifiers::ALT, Key::ArrowUp),
                        i.consume_key(Modifiers::ALT, Key::ArrowDown),
                        i.consume_key(Modifiers::NONE, Key::Enter),
                        i.consume_key(Modifiers::NONE, Key::Escape),
                    )
                });
                if k.0 {
                    ops.push(Op::Delete(p.clone()));
                }
                if k.1 {
                    ops.push(Op::Duplicate(p.clone()));
                }
                if k.2 {
                    ops.push(Op::Move(p.clone(), -1));
                }
                if k.3 {
                    ops.push(Op::Move(p.clone(), 1));
                }
                if k.4 {
                    if let Some(f) = slides::elem_ref(&slide.body, p).and_then(|e| first_field(frame, p, e)) {
                        self.slide_ed.focus(f, usize::MAX);
                    }
                }
                if k.5 {
                    ops.push(Op::Deselect);
                }
                if copy || cut {
                    self.slide_ed.clipboard = slides::elem_ref(&slide.body, p).cloned();
                    if cut {
                        ops.push(Op::Delete(p.clone()));
                    }
                }
            } else {
                // no selection: arrows / page keys switch slides
                let (prev, next) = ctx.input_mut(|i| {
                    (
                        i.consume_key(Modifiers::NONE, Key::PageUp) || i.consume_key(Modifiers::NONE, Key::ArrowUp),
                        i.consume_key(Modifiers::NONE, Key::PageDown) || i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    )
                });
                if prev && frame > 0 {
                    self.goto_frame(bi, frame - 1);
                }
                if next && frame + 1 < deck.frames.len() {
                    self.goto_frame(bi, frame + 1);
                }
                if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::D)) {
                    slide_op = Some("dup");
                }
            }
            if paste {
                if let Some(e) = self.slide_ed.clipboard.clone() {
                    let p = slides::insert_after(&mut slide.body, sel.as_deref(), e);
                    self.slide_ed.sel = Some(p);
                }
            }
        }

        // ── top tool bar ──
        let bar = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover()).0;
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 90)));
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(10.0, 5.0))).layout(egui::Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 4.0;
        widgets::chip(&mut right, ic::EYE, tr!("Visuell" | "Visual"), true, pal.accent, &pal);
        if widgets::chip(&mut right, ic::CODE, "Code", false, pal.accent, &pal).clicked() {
            self.settings.slides_visual = false;
            self.settings.save();
        }
        if widgets::icon_button_sized(&mut right, ic::FILE_PDF, tr!("Folienvorschau ein/aus (Strg+L)" | "Toggle slide preview (Ctrl+L)"), &pal, self.settings.show_stage, 28.0).clicked() {
            self.toggle_pdf_preview();
        }
        let left_rect = Rect::from_min_max(bar.min + vec2(10.0, 5.0), pos2(right.min_rect().min.x - 8.0, bar.max.y - 5.0));
        let mut bl = ui.new_child(egui::UiBuilder::new().max_rect(left_rect).layout(egui::Layout::left_to_right(Align::Center)));
        bl.set_clip_rect(left_rect.intersect(ui.clip_rect()));
        bl.spacing_mut().item_spacing.x = 2.0;
        let add = widgets::button(&mut bl, ic::PLUS, tr!("Folie" | "Slide"), &pal, widgets::BtnKind::Primary).on_hover_text(tr!("Neue Folie (Strg+M)" | "New slide (Ctrl+M)"));
        egui::Popup::menu(&add).show(|ui| {
            ui.set_min_width(210.0);
            for (tpl, icon, label) in [
                (Template::Bullets, ic::LIST_UL, tr!("Titel und Aufzählung" | "Title and bullets")),
                (Template::TwoColumns, ic::COLUMNS, tr!("Zwei Spalten" | "Two columns")),
                (Template::Image, ic::IMAGE, tr!("Bildfolie" | "Image slide")),
                (Template::Block, ic::STICKY, tr!("Kernaussage (Block)" | "Key message (block)")),
                (Template::Section, ic::QUOTE, tr!("Abschluss („Fragen?“)" | "Closing (“Questions?”)")),
                (Template::Blank, ic::SQUARE, tr!("Leere Folie" | "Blank slide")),
            ] {
                if ui.button(format!("{icon}  {label}")).clicked() {
                    new_tpl = Some(tpl);
                    ui.close();
                }
            }
        });
        let sep = |ui: &mut Ui| {
            let r = ui.allocate_exact_size(vec2(10.0, 20.0), Sense::hover()).0;
            ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, pal.border));
        };
        for (icon, tip, act) in [(ic::COPY, tr!("Folie duplizieren" | "Duplicate slide"), "dup"), (ic::TRASH, tr!("Folie löschen" | "Delete slide"), "del")] {
            if widgets::icon_button_sized(&mut bl, icon, tip, &pal, false, 28.0).clicked() {
                slide_op = Some(act);
            }
        }
        sep(&mut bl);
        let fmt_target = self.slide_ed.last_text.as_ref().map(|t| t.id);
        for (icon, tip, pre, post) in [
            (ic::BOLD, tr!("Fett (Strg+B)" | "Bold (Ctrl+B)"), "\\textbf{", "}"),
            (ic::ITALIC, tr!("Kursiv (Strg+I)" | "Italic (Ctrl+I)"), "\\emph{", "}"),
            (ic::BOLT, tr!("Hervorheben (\\alert)" | "Highlight (\\alert)"), "\\alert{", "}"),
            (ic::CODE_INLINE, tr!("Code / Schreibmaschine" | "Code / monospace"), "\\texttt{", "}"),
        ] {
            if widgets::icon_button_sized(&mut bl, icon, tip, &pal, false, 28.0).clicked() {
                if let Some(id) = fmt_target {
                    self.slide_ed.pending_wrap = Some((id, pre.into(), post.into()));
                }
            }
        }
        // citation picker
        let cite_btn = widgets::icon_button_sized(&mut bl, ic::QUOTE, tr!("Quelle zitieren" | "Cite a source"), &pal, false, 28.0);
        egui::Popup::menu(&cite_btn).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(300.0);
            ui.add(egui::TextEdit::singleline(&mut self.slide_ed.cite_query).hint_text(tr!("Suchen …" | "Search …")).desired_width(290.0)).request_focus();
            ui.add_space(4.0);
            let q = self.slide_ed.cite_query.to_lowercase();
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                let mut any = false;
                for c in self.cites.iter().filter(|c| q.is_empty() || c.label.to_lowercase().contains(&q) || c.detail.to_lowercase().contains(&q)) {
                    any = true;
                    if ui.add(egui::Button::new(egui::RichText::new(format!("{}\n{}", c.label, crate::app::truncate(&c.detail, 46))).size(12.0)).frame(false)).clicked() {
                        if let Some(id) = fmt_target {
                            self.slide_ed.pending_wrap = Some((id, format!("\\cite{{{}}}", c.label), String::new()));
                        }
                        ui.close();
                    }
                }
                if !any {
                    ui.label(egui::RichText::new(tr!("Keine Quellen in der Bibliothek" | "No sources in the library")).color(pal.dim));
                }
            });
        });
        sep(&mut bl);
        if widgets::icon_button_sized(&mut bl, ic::UNDO, tr!("Rückgängig (Strg+Z)" | "Undo (Ctrl+Z)"), &pal, false, 28.0).clicked() {
            self.slides_undo(bi, false, now);
            return;
        }
        if widgets::icon_button_sized(&mut bl, ic::REDO, tr!("Wiederholen (Strg+Umschalt+Z)" | "Redo (Ctrl+Shift+Z)"), &pal, false, 28.0).clicked() {
            self.slides_undo(bi, true, now);
            return;
        }

        // ── canvas ──
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, pal.crust);
        let ratio = if deck.wide { 9.0 / 16.0 } else { 3.0 / 4.0 };
        let avail = area.shrink2(vec2(64.0, 30.0));
        let mut w = avail.width().min(1100.0);
        if w * ratio > avail.height() - 70.0 {
            w = ((avail.height() - 70.0) / ratio).max(320.0);
        }
        let meta: HashMap<&'static str, String> = deck.meta.iter().map(|m| (m.cmd, m.value.clone())).collect();
        let slides_main = self.project.config.slides_main.clone();
        let dir = self.project.root.join(&slides_main).parent().map(Path::to_path_buf).unwrap_or_else(|| self.project.root.clone());
        let root = self.project.root.clone();
        let section = fr.as_ref().map(|f| f.section.clone()).unwrap_or_default();
        let mut section_result = None;
        let mut canvas_rect = Rect::NOTHING;
        if fr.is_none() {
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(40.0)).layout(egui::Layout::top_down(Align::Center)));
            c.add_space(40.0);
            c.label(egui::RichText::new(tr!("Noch keine Folien" | "No slides yet")).font(widgets::display_font(22.0)).color(pal.bright));
            c.add_space(10.0);
            if widgets::button(&mut c, ic::PLUS, tr!("Erste Folie anlegen" | "Create the first slide"), &pal, widgets::BtnKind::Primary).clicked() {
                new_tpl = Some(Template::Bullets);
            }
        } else {
            let mut cx = Cx {
                editing: None,
                k: w / 640.0,
                tug: deck.tug,
                frame,
                ops: vec![],
                ed: &mut self.slide_ed,
                dirs: (dir.clone(), root.clone()),
                toc: deck.toc.clone(),
                meta,
                fields: vec![],
                rects: HashMap::new(),
                text_focus: false,
                content: Rect::NOTHING,
                section_result: None,
            };
            egui::ScrollArea::vertical().id_salt("slide-canvas").auto_shrink([false, false]).show(ui, |ui| {
                let h = w * ratio;
                let (r, bg) = ui.allocate_exact_size(vec2(area.width(), h + 150.0), Sense::click());
                // click on the empty background: deselect
                if bg.clicked() {
                    cx.ops.push(Op::Deselect);
                    if let Some(f) = ui.ctx().memory(|m| m.focused()) {
                        ui.ctx().memory_mut(|m| m.surrender_focus(f));
                    }
                }
                let cr = Rect::from_min_size(pos2(r.center().x - w / 2.0, r.min.y + 16.0), vec2(w, h));
                canvas_rect = cr;
                canvas(ui, &mut cx, &mut slide, frame + 1, &section, cr);
                // insert bar under the slide: after the selection, or at the end
                let bar_y = cr.max.y + 20.0;
                let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(cr.min.x, bar_y), vec2(w, 90.0))).layout(egui::Layout::top_down(Align::Min)));
                let where_ = if cx.ed.sel.is_some() { tr!("Einfügen nach Auswahl:" | "Insert after selection:") } else { tr!("Einfügen:" | "Insert:") };
                bar.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
                    ui.label(egui::RichText::new(where_).font(widgets::ui_font(12.0)).color(pal.dim));
                    for kind in NewKind::all() {
                        if widgets::chip(ui, kind.icon(), kind.label(), false, pal.accent, &pal).clicked() {
                            cx.ops.push(Op::InsertAfter(cx.ed.sel.clone(), kind));
                        }
                    }
                });
                let hint = tr!(
                    "Klick wählt ein Element · Enter neuer Absatz/Punkt · Tab einrücken · ↑↓ zwischen Feldern · Esc auswählen · Entf löschen · Strg+D duplizieren · Alt+↑↓ verschieben · Strg+C/V kopieren · Strg+M neue Folie"
                        | "Click selects · Enter new paragraph/item · Tab indent · ↑↓ between fields · Esc select · Del delete · Ctrl+D duplicate · Alt+↑↓ move · Ctrl+C/V copy · Ctrl+M new slide"
                );
                let g = ui.painter().layout(hint.to_string(), widgets::ui_font(10.5), pal.dim, w);
                let gy = bar.min_rect().max.y + 10.0;
                ui.painter().galley(pos2(cr.center().x - g.size().x / 2.0, gy), g, pal.dim);
            });
            // selection outline + floating tool bar
            if let Some(p) = cx.ed.sel.clone() {
                if let Some(r) = cx.rects.get(&p).copied() {
                    ui.painter().rect_stroke(r.expand(2.0), 5.0, Stroke::new(1.5, SEL), StrokeKind::Outside);
                    floating_toolbar(&ctx, &mut cx, &slide, &p, r, canvas_rect);
                } else if slides::elem_ref(&slide.body, &p).is_none() {
                    cx.ed.sel = None;
                }
            }
            ops.extend(std::mem::take(&mut cx.ops));
            section_result = cx.section_result.take();
            cx.ed.field_order = std::mem::take(&mut cx.fields);
            cx.ed.editing = cx.editing.take();
            cx.ed.elem_rects = std::mem::take(&mut cx.rects);
        }

        // ── apply element operations ──
        let tug = deck.tug;
        let booktabs = deck.booktabs;
        let mut meta_edits: Vec<(&'static str, String)> = vec![];
        let mut pick: Option<Vec<usize>> = None;
        let mut structural = false;
        let field_order = self.slide_ed.field_order.clone();
        for op in ops {
            let ed = &mut self.slide_ed;
            match op {
                Op::Select(p) => ed.sel = Some(p),
                Op::Deselect => ed.sel = None,
                Op::TextEnter(p, at) => {
                    if let Some(np) = slides::split_text(&mut slide.body, &p, at) {
                        ed.focus(field_id(frame, &np, SUB_MAIN), 0);
                        ed.sel = Some(np);
                        structural = true;
                    }
                }
                Op::TextBackspace(p) => {
                    let empty = matches!(slides::elem_ref(&slide.body, &p), Some(Elem::Text(t)) if t.is_empty());
                    if let Some((prev, caret)) = slides::merge_text_prev(&mut slide.body, &p) {
                        ed.focus(field_id(frame, &prev, SUB_MAIN), caret);
                        ed.sel = Some(prev);
                        structural = true;
                    } else if empty {
                        // empty paragraph after something else: remove it, focus the previous field
                        let me = field_id(frame, &p, SUB_MAIN);
                        let prev_field = field_order.iter().position(|f| *f == me).and_then(|i| i.checked_sub(1)).map(|i| field_order[i]);
                        ed.sel = slides::delete(&mut slide.body, &p);
                        if let Some(f) = prev_field {
                            ed.focus(f, usize::MAX);
                        }
                        structural = true;
                    }
                }
                Op::ItemEnter(p, idx, at) => {
                    match slides::list_enter(&mut slide.body, &p, idx, at) {
                        Some(ListResult::Item(i, c)) => ed.focus(field_id(frame, &p, SUB_ITEM + i), c),
                        Some(ListResult::Exit(np)) => {
                            ed.focus(field_id(frame, &np, SUB_MAIN), 0);
                            ed.sel = Some(np);
                        }
                        Some(ListResult::Removed(s)) => ed.sel = s,
                        None => {}
                    }
                    structural = true;
                }
                Op::ItemBackspace(p, idx) => {
                    match slides::list_backspace(&mut slide.body, &p, idx) {
                        Some(ListResult::Item(i, c)) => ed.focus(field_id(frame, &p, SUB_ITEM + i), c),
                        Some(ListResult::Removed(s)) => {
                            ed.sel = s;
                            let me = field_id(frame, &p, SUB_ITEM + idx);
                            if let Some(f) = field_order.iter().position(|f| *f == me).and_then(|i| i.checked_sub(1)).map(|i| field_order[i]) {
                                ed.focus(f, usize::MAX);
                            }
                        }
                        Some(ListResult::Exit(np)) => ed.focus(field_id(frame, &np, SUB_MAIN), 0),
                        None => {}
                    }
                    structural = true;
                }
                Op::ItemPause(p, idx) => {
                    if let Some(Elem::List { items, .. }) = slides::elem_mut(&mut slide.body, &p) {
                        if let Some(it) = items.get_mut(idx) {
                            it.pause = !it.pause;
                            structural = true;
                        }
                    }
                }
                Op::ItemEnterStep(p, idx, at) => {
                    if let Some(ListResult::Item(i, c)) = slides::list_enter(&mut slide.body, &p, idx, at) {
                        if i == idx + 1 {
                            if let Some(Elem::List { items, .. }) = slides::elem_mut(&mut slide.body, &p) {
                                items[i].pause = true;
                            }
                        }
                        ed.focus(field_id(frame, &p, SUB_ITEM + i), c);
                    }
                    structural = true;
                }
                Op::TocCols(p, n) => {
                    if let Some(Elem::Toc { cols }) = slides::elem_mut(&mut slide.body, &p) {
                        *cols = n;
                        structural = true;
                    }
                }
                Op::ItemIndent(p, idx, d) => {
                    slides::list_indent(&mut slide.body, &p, idx, d);
                    ed.focus(field_id(frame, &p, SUB_ITEM + idx), usize::MAX);
                    structural = true;
                }
                Op::CellMove(p, r, c, dr, dc) => {
                    if let Some(Elem::Table { rows, .. }) = slides::elem_mut(&mut slide.body, &p) {
                        let nc = rows.first().map_or(1, Vec::len) as i32;
                        let mut ri = r as i32 + dr;
                        let mut ci = c as i32 + dc;
                        if ci >= nc {
                            ci = 0;
                            ri += 1;
                        }
                        if ci < 0 {
                            ci = nc - 1;
                            ri -= 1;
                        }
                        if ri >= rows.len() as i32 {
                            // Enter / Tab in the last row adds a row
                            rows.push(vec![String::new(); nc as usize]);
                            structural = true;
                        }
                        if ri >= 0 {
                            ed.focus(field_id(frame, &p, SUB_CELL + ri as usize * 256 + ci.max(0) as usize), usize::MAX);
                        }
                    }
                }
                Op::FocusSibling(id, d) => {
                    if let Some(i) = field_order.iter().position(|f| *f == id) {
                        let j = i as i32 + d;
                        if j >= 0 && (j as usize) < field_order.len() {
                            ed.focus(field_order[j as usize], if d < 0 { usize::MAX } else { 0 });
                        }
                    }
                }
                Op::Move(p, d) => {
                    if let Some(np) = slides::move_by(&mut slide.body, &p, d) {
                        ed.sel = Some(np);
                        structural = true;
                    }
                }
                Op::Delete(p) => {
                    ed.sel = slides::delete(&mut slide.body, &p);
                    structural = true;
                }
                Op::Duplicate(p) => {
                    if let Some(np) = slides::duplicate(&mut slide.body, &p) {
                        ed.sel = Some(np);
                        structural = true;
                    }
                }
                Op::InsertBefore(p, kind) => {
                    let e = new_elem(kind, tug, booktabs);
                    let np = slides::insert_before(&mut slide.body, &p, e.clone());
                    after_insert(ed, frame, &np, &e, kind, &mut pick);
                    structural = true;
                }
                Op::InsertAfter(p, kind) => {
                    let e = new_elem(kind, tug, booktabs);
                    let np = slides::insert_after(&mut slide.body, p.as_deref(), e.clone());
                    after_insert(ed, frame, &np, &e, kind, &mut pick);
                    structural = true;
                }
                Op::PickImage(p) => pick = Some(p),
                Op::ImageSize(p, d) => {
                    if let Some(Elem::Image { height, .. }) = slides::elem_mut(&mut slide.body, &p) {
                        *height = (((*height + d) * 10.0).round() / 10.0).clamp(0.2, 0.9);
                        structural = true;
                    }
                }
                Op::ImageAlign(p, a) => {
                    if let Some(Elem::Image { align, .. }) = slides::elem_mut(&mut slide.body, &p) {
                        *align = a;
                        structural = true;
                    }
                }
                Op::Meta(cmd, v) => meta_edits.push((cmd, v)),
                Op::Table(p, top) => {
                    if let Some(Elem::Table { rows, header, spec, .. }) = slides::elem_mut(&mut slide.body, &p) {
                        let nc = rows.first().map_or(1, Vec::len);
                        match top {
                            TableOp::AddRow => rows.push(vec![String::new(); nc]),
                            TableOp::AddCol => {
                                for r in rows.iter_mut() {
                                    r.push(String::new());
                                }
                                *spec = grow_spec(spec, 1);
                            }
                            TableOp::DelRow if rows.len() > 1 => {
                                rows.pop();
                            }
                            TableOp::DelCol if nc > 1 => {
                                for r in rows.iter_mut() {
                                    r.pop();
                                }
                                *spec = grow_spec(spec, -1);
                            }
                            TableOp::Header => *header = !*header,
                            _ => {}
                        }
                        structural = true;
                    }
                }
                Op::ListNumbered(p) => {
                    if let Some(Elem::List { env, numbered, .. }) = slides::elem_mut(&mut slide.body, &p) {
                        *numbered = !*numbered;
                        *env = match (env.as_str(), *numbered) {
                            ("tugitemize", true) => "tugenumerate",
                            ("tugenumerate", false) => "tugitemize",
                            ("compactitem", true) => "compactenum",
                            ("compactenum", false) => "compactitem",
                            (_, true) => "enumerate",
                            (_, false) => "itemize",
                        }
                        .into();
                        structural = true;
                    }
                }
                Op::ListStep(p) => {
                    if let Some(Elem::List { step, .. }) = slides::elem_mut(&mut slide.body, &p) {
                        *step = !*step;
                        structural = true;
                    }
                }
                Op::BlockKind(p, k) => {
                    if let Some(Elem::Block { kind, .. }) = slides::elem_mut(&mut slide.body, &p) {
                        *kind = k;
                        structural = true;
                    }
                }
                Op::TextStyle(p, center, size) => {
                    if let Some(Elem::Text(t)) = slides::elem_mut(&mut slide.body, &p) {
                        let (c0, _, _) = slides::text_style(t);
                        *t = slides::set_text_style(t, center.unwrap_or(c0), size);
                        structural = true;
                    }
                }
                Op::ColumnsCount(p, d) => {
                    if let Some(Elem::Columns { cols, .. }) = slides::elem_mut(&mut slide.body, &p) {
                        if d > 0 && cols.len() < 3 {
                            cols.push(vec![Elem::Text(String::new())]);
                        } else if d < 0 && cols.len() > 1 {
                            // the content of the removed column moves into the one before
                            let last = cols.pop().unwrap_or_default();
                            if let Some(prev) = cols.last_mut() {
                                prev.extend(last.into_iter().filter(|e| !matches!(e, Elem::Text(t) if t.is_empty())));
                            }
                        }
                        structural = true;
                    }
                }
            }
        }
        if let Some(p) = pick {
            let file = rfd::FileDialog::new().add_filter(tr!("Bilder" | "Images"), &["png", "jpg", "jpeg", "pdf"]).set_title(tr!("Bild für die Folie wählen" | "Choose an image for the slide")).pick_file();
            if let Some(f) = file.and_then(|f| import_image(&f, &dir, &root)) {
                if let Some(Elem::Image { path, .. }) = slides::elem_mut(&mut slide.body, &p) {
                    *path = f;
                    structural = true;
                }
            }
        }

        // ── write back ──
        let Some(fr) = fr else {
            if let Some(t) = new_tpl {
                let s = slides::write_slide(&slides::new_slide(t, tug));
                let (nt, c) = insert_frame_after(&text, &deck, None, &s);
                self.commit_slides(bi, nt, c, now, true);
            }
            return;
        };
        let mut new_text = text.clone();
        let mut cursor_at = fr.start;
        let edited = slide != base;
        if edited {
            let w = slides::write_slide(&slide);
            if w != text[fr.start..fr.end] {
                new_text.replace_range(fr.start..fr.end, &w);
            }
        }
        // preamble metadata and the section command – both lie before this frame
        let mut edits: Vec<(usize, usize, String)> = meta_edits.into_iter().filter_map(|(cmd, v)| deck.meta.iter().find(|m| m.cmd == cmd).map(|m| (m.range.0, m.range.1, v))).collect();
        if let Some((sname, new_here)) = section_result {
            // the frame where the current section starts (rename there)
            let owner = deck.frames[..=frame].iter().rev().find(|f| f.section_here.is_some()).and_then(|f| f.section_here);
            let here = fr.section_here;
            let target = if new_here { here } else { owner };
            match (target, sname.is_empty(), new_here) {
                (Some((_, _, t0, t1)), false, _) if sname != fr.section || new_here => edits.push((t0, t1, sname)),
                (Some((a, e, _, _)), true, false) => edits.push((a, e, String::new())),
                (None, false, _) => edits.push((fr.start, fr.start, format!("\\section{{{sname}}}\n\n"))),
                _ => {}
            }
            structural = true;
        }
        edits.sort_by(|a, b| b.0.cmp(&a.0));
        for (a, b, v) in edits {
            let delta = v.len() as isize - (b - a) as isize;
            new_text.replace_range(a..b, &v);
            if a <= cursor_at {
                cursor_at = (cursor_at as isize + delta) as usize;
            }
        }
        let mut structural_slide = false;
        match slide_op {
            Some("dup") => {
                let d = slides::parse_deck(&new_text);
                let f = &d.frames[frame.min(d.frames.len() - 1)];
                let body = new_text[f.start..f.end].to_string();
                let (t, c) = insert_frame_after(&new_text, &d, Some(frame), &body);
                new_text = t;
                cursor_at = c;
                structural_slide = true;
            }
            Some("del") => {
                let d = slides::parse_deck(&new_text);
                if let Some((t, c)) = delete_frame(&new_text, &d, frame) {
                    new_text = t;
                    cursor_at = c;
                    structural_slide = true;
                }
            }
            _ => {}
        }
        if let Some(t) = new_tpl {
            let s = slides::write_slide(&slides::new_slide(t, tug));
            let d = slides::parse_deck(&new_text);
            let (nt, c) = insert_frame_after(&new_text, &d, Some(frame), &s);
            new_text = nt;
            cursor_at = c;
            structural_slide = true;
            self.slide_ed.focus_req = Some((field_id(frame + 1, &[], SUB_TITLE), SELECT_ALL));
        }
        if edited && !structural_slide {
            // keep the typing state (trailing spaces …) of this slide
            self.slide_ed.remember(frame, slide.clone(), hash_str(&new_text));
        } else if structural_slide {
            self.slide_ed.draft = None;
            self.slide_ed.sel = None;
        }
        if new_text != text {
            self.commit_slides(bi, new_text, cursor_at, now, structural || structural_slide);
            if edited && !structural_slide {
                // commit doesn't change the text further: the draft stays valid
                let h = self.slide_ed.key;
                if let Some(d) = &mut self.slide_ed.draft {
                    d.2 = h;
                }
            }
        }
    }
}

/// After inserting an element: select it, focus its first field, open the image dialog.
fn after_insert(ed: &mut SlideEditor, frame: usize, path: &[usize], e: &Elem, kind: NewKind, pick: &mut Option<Vec<usize>>) {
    ed.sel = Some(path.to_vec());
    if kind == NewKind::Image {
        *pick = Some(path.to_vec());
    } else if let Some(f) = first_field(frame, path, e) {
        ed.focus(f, usize::MAX);
    }
}

/// Add / remove columns in a tabular spec like `lrr` (complex specs get `l` columns).
fn grow_spec(spec: &str, d: i32) -> String {
    let simple = spec.chars().all(|c| "lcr|".contains(c));
    if !simple {
        return spec.to_string();
    }
    if d > 0 {
        let last = spec.chars().rev().find(|c| "lcr".contains(*c)).unwrap_or('l');
        format!("{spec}{last}")
    } else {
        let mut s = spec.to_string();
        if let Some(p) = s.rfind(['l', 'c', 'r']) {
            s.remove(p);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\\begin{document}\n\\begin{frame}{A}\na\n\\end{frame}\n\n\\begin{frame}{B}\nb\n\\end{frame}\n\n\\begin{frame}{C}\nc\n\\end{frame}\n\\end{document}\n";

    fn titles(t: &str) -> Vec<String> {
        slides::parse_deck(t).frames.iter().map(|f| f.slide.title.clone()).collect()
    }

    #[test]
    fn frame_operations() {
        let d = slides::parse_deck(SRC);
        let (t, _) = move_frame(SRC, &d, 0, 2).unwrap();
        assert_eq!(titles(&t), ["B", "C", "A"]);
        let (t, _) = move_frame(SRC, &d, 2, 0).unwrap();
        assert_eq!(titles(&t), ["C", "A", "B"]);
        let (t, _) = move_frame(SRC, &d, 1, 2).unwrap();
        assert_eq!(titles(&t), ["A", "C", "B"]);
        let (t, c) = delete_frame(SRC, &d, 1).unwrap();
        assert_eq!(titles(&t), ["A", "C"]);
        assert!(t[c..].starts_with("\\begin{frame}{C}"));
        let (t, c) = insert_frame_after(SRC, &d, Some(0), "\\begin{frame}{N}\n\\end{frame}");
        assert_eq!(titles(&t), ["A", "N", "B", "C"]);
        assert!(t[c..].starts_with("\\begin{frame}{N}"));
        // empty deck
        let e = "\\begin{document}\n\\end{document}\n";
        let (t, _) = insert_frame_after(e, &slides::parse_deck(e), None, "\\begin{frame}{X}\n\\end{frame}");
        assert_eq!(titles(&t), ["X"]);
    }

    #[test]
    fn spec_and_inline() {
        assert_eq!(grow_spec("lrr", 1), "lrrr");
        assert_eq!(grow_spec("lrr", -1), "lr");
        assert_eq!(grow_spec("p{3cm}l", 1), "p{3cm}l");
        let base = St { bold: false, italic: false, color: INK, mono: false };
        let job = inline_job("a \\textbf{b} \\% $x$", 14.0, base, TUG_RED, false, 300.0, false);
        let visible: String = job.sections.iter().filter(|s| s.format.color != Color32::TRANSPARENT).map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0]).collect();
        assert_eq!(visible, "a b % x");
        assert_eq!(slides_plain("Hops\\\\Yeast \\textbf{x}"), "Hops Yeast x");
    }
}

fn roman(n: usize) -> String {
    let mut n = n;
    let mut s = String::new();
    for (v, r) in [(10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")] {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// Position a box of `size` inside `row` (left / centered / right).
fn place(row: Rect, size: egui::Vec2, align: slides::ImgAlign) -> Rect {
    let x = match align {
        slides::ImgAlign::Left => row.min.x,
        slides::ImgAlign::Center => row.center().x - size.x / 2.0,
        slides::ImgAlign::Right => row.max.x - size.x,
    };
    Rect::from_min_size(pos2(x, row.center().y - size.y / 2.0), size)
}
