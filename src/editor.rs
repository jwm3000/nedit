//! Code editor: file buffers, LaTeX syntax highlighting, gutter, completion.

use crate::compile::Level;
use crate::theme::{mix, with_alpha, Palette};
use egui::text::{CCursor, CCursorRange, LayoutJob, LayoutSection, TextFormat};
use egui::{pos2, vec2, Color32, FontFamily, FontId, Key, Modifiers, Rect, Shape, Stroke};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

pub struct Buffer {
    pub rel: String,
    pub abs: PathBuf,
    pub text: String,
    saved_hash: u64,
    pub disk_stamp: u64,
    pub id: egui::Id,
    pending_select: Option<(usize, usize)>,
    /// Scroll the new cursor into view (false for small in-place edits while typing).
    pending_scroll: bool,
    init_cursor: bool,
    pub cursor: usize,
    pub sel_end: usize,
    pub line: usize,
    pub col: usize,
    hl_cache: Option<(u64, LayoutJob)>,
    vis_cache: Option<(u64, Vec<crate::visual::Span>, Vec<crate::visual::Decor>)>,
    search_cache: Option<(u64, String, Vec<(usize, usize)>)>,
    search_pulse: (usize, f64),
    /// Vim block cursor: Some(Some(pos)) fixed position, Some(None) = at the live cursor.
    pub vim_block: Option<Option<usize>>,
    /// Vim visual-block selection (char ranges, one per line).
    pub vim_block_sel: Vec<(usize, usize)>,
    pub doc_height: f32,
    pub last_edit: f64,
    pub completion: Option<Completion>,
    pub request_focus: bool,
    /// Frames the focus request has been retried (gives up after a few).
    focus_tries: u8,
    pub cursor_moved: bool,
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

pub fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

pub fn byte_to_char(s: &str, bi: usize) -> usize {
    s[..bi.min(s.len())].chars().count()
}

impl Buffer {
    pub fn open(root: &std::path::Path, rel: &str) -> std::io::Result<Self> {
        let abs = root.join(rel);
        let text = std::fs::read_to_string(&abs)?.replace("\r\n", "\n");
        Ok(Buffer {
            rel: rel.to_string(),
            saved_hash: hash_str(&text),
            disk_stamp: crate::pdfview::file_stamp(&abs),
            abs,
            text,
            id: egui::Id::new(("buffer", rel.to_string())),
            pending_select: None,
            pending_scroll: false,
            init_cursor: true,
            cursor: 0,
            sel_end: 0,
            line: 1,
            col: 1,
            hl_cache: None,
            vis_cache: None,
            search_cache: None,
            search_pulse: (usize::MAX, 0.0),
            vim_block: None,
            vim_block_sel: vec![],
            doc_height: 0.0,
            last_edit: 0.0,
            completion: None,
            request_focus: false,
            focus_tries: 0,
            cursor_moved: false,
        })
    }

    pub fn has_pending_select(&self) -> bool {
        self.pending_select.is_some()
    }

    pub fn dirty(&self) -> bool {
        hash_str(&self.text) != self.saved_hash
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        if let Some(p) = self.abs.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&self.abs, &self.text)?;
        self.saved_hash = hash_str(&self.text);
        self.disk_stamp = crate::pdfview::file_stamp(&self.abs);
        Ok(())
    }

    /// Pick up external changes if the buffer has no unsaved edits.
    pub fn reload_if_changed(&mut self) -> bool {
        let st = crate::pdfview::file_stamp(&self.abs);
        if st != self.disk_stamp && st != 0 {
            self.disk_stamp = st;
            if !self.dirty() {
                if let Ok(t) = std::fs::read_to_string(&self.abs) {
                    self.text = t.replace("\r\n", "\n");
                    self.saved_hash = hash_str(&self.text);
                    return true;
                }
            }
        }
        false
    }

    /// Replace the buffer with the file on disk, discarding unsaved edits
    /// (used after git revert/restore so autosave can't write old text back).
    pub fn force_reload(&mut self) -> bool {
        match std::fs::read_to_string(&self.abs) {
            Ok(t) => {
                self.set_text_external(t.replace("\r\n", "\n"));
                self.completion = None;
                self.hl_cache = None;
                self.vis_cache = None;
                self.search_cache = None;
                true
            }
            Err(_) => false,
        }
    }

    pub fn set_text_external(&mut self, t: String) {
        self.text = t;
        self.saved_hash = hash_str(&self.text);
        self.disk_stamp = crate::pdfview::file_stamp(&self.abs);
    }

    /// Select a char range and scroll it into view on the next frame.
    pub fn select(&mut self, a: usize, b: usize) {
        self.pending_select = Some((a, b));
        self.pending_scroll = true;
        self.request_focus = true;
    }

    /// Where the cursor is (or will be after a pending jump).
    pub fn target_cursor(&self) -> usize {
        self.pending_select.map_or(self.cursor, |p| p.1)
    }

    /// Move the cursor without scrolling the view (used while typing).
    pub fn place_cursor(&mut self, a: usize, b: usize) {
        self.pending_select = Some((a, b));
        self.pending_scroll = false;
    }

    pub fn goto_line(&mut self, line: usize) {
        let mut ci = 0;
        let mut l = 1;
        for c in self.text.chars() {
            if l >= line {
                break;
            }
            if c == '\n' {
                l += 1;
            }
            ci += 1;
        }
        self.select(ci, ci);
    }

    fn replace_chars(&mut self, a: usize, b: usize, with: &str) {
        let ba = char_to_byte(&self.text, a);
        let bb = char_to_byte(&self.text, b);
        self.text.replace_range(ba..bb, with);
    }

    /// Insert at cursor (replacing selection). `$0` marks the final cursor position.
    pub fn insert_snippet(&mut self, snippet: &str) {
        let (a, b) = (self.cursor.min(self.sel_end), self.cursor.max(self.sel_end));
        let selected: String = self.text.chars().skip(a).take(b - a).collect();
        let s = snippet.replace("$SEL", &selected);
        let (clean, caret) = match s.find("$0") {
            Some(p) => (s.replacen("$0", "", 1), byte_to_char(&s, p)),
            None => (s.clone(), s.chars().count()),
        };
        self.replace_chars(a, b, &clean);
        self.select(a + caret, a + caret);
    }
}

// ───────────────────────────── highlighting ─────────────────────────────

#[derive(Clone)]
pub struct Syntax {
    pub text: Color32,
    pub command: Color32,
    pub keyword: Color32,
    pub env: Color32,
    pub math: Color32,
    pub math_cmd: Color32,
    pub comment: Color32,
    pub brace: Color32,
    pub special: Color32,
    pub cite: Color32,
    pub refc: Color32,
    pub heading: Color32,
}

impl Syntax {
    pub fn from_palette(p: &Palette) -> Self {
        Syntax {
            text: p.text,
            command: p.blue,
            keyword: p.magenta,
            env: p.cyan,
            math: p.green,
            math_cmd: mix(p.green, p.cyan, 0.5),
            comment: mix(p.dim, p.base, 0.1),
            brace: mix(p.subtext, p.base, 0.25),
            special: p.orange,
            cite: p.yellow,
            refc: p.cyan,
            heading: p.bright,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tok {
    Text,
    Command,
    Keyword,
    Env,
    Math,
    MathCmd,
    Comment,
    Brace,
    Special,
    Cite,
    Ref,
    Heading,
}

#[derive(Clone, Copy, PartialEq)]
enum Arg {
    Heading,
    Cite,
    Ref,
}

const MATH_ENVS: &[&str] = &[
    "equation", "equation*", "align", "align*", "gather", "gather*", "multline", "multline*", "flalign", "flalign*",
    "displaymath", "math", "eqnarray", "eqnarray*", "alignat", "alignat*",
];
const HEADINGS: &[&str] = &[
    "part", "chapter", "section", "subsection", "subsubsection", "paragraph", "title", "frametitle", "framesubtitle", "caption",
    "subtitle",
];

pub fn is_cite_cmd(n: &str) -> bool {
    n.contains("cite") || n == "nocite"
}

pub fn is_ref_cmd(n: &str) -> bool {
    matches!(n, "ref" | "eqref" | "cref" | "Cref" | "autoref" | "pageref" | "label" | "vref" | "nameref" | "input" | "include" | "includegraphics" | "bibliography" | "usetheme")
}

fn tokenize(text: &str) -> Vec<(usize, usize, Tok)> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out: Vec<(usize, usize, Tok)> = Vec::with_capacity(n / 6);
    let push = |s: usize, e: usize, t: Tok, out: &mut Vec<(usize, usize, Tok)>| {
        if e <= s {
            return;
        }
        if let Some(last) = out.last_mut() {
            if last.2 == t && last.1 == s {
                last.1 = e;
                return;
            }
        }
        out.push((s, e, t));
    };
    #[derive(PartialEq, Clone, Copy)]
    enum M {
        None,
        Dollar,
        DDollar,
        Paren,
        Bracket,
        Env,
    }
    let mut math = M::None;
    let mut pending: Option<Arg> = None;
    let is_alpha = |c: u8| c.is_ascii_alphabetic() || c == b'@';
    let mut i = 0;
    while i < n {
        let c = b[i];
        let in_math = math != M::None;
        match c {
            b'%' => {
                let e = text[i..].find('\n').map(|p| i + p).unwrap_or(n);
                push(i, e, Tok::Comment, &mut out);
                i = e;
            }
            b'\\' => {
                let mut j = i + 1;
                if j < n && is_alpha(b[j]) {
                    while j < n && is_alpha(b[j]) {
                        j += 1;
                    }
                    let name = &text[i + 1..j];
                    if j < n && b[j] == b'*' {
                        j += 1;
                    }
                    if name == "begin" || name == "end" {
                        push(i, j, Tok::Keyword, &mut out);
                        if j < n && b[j] == b'{' {
                            if let Some(close) = text[j..].find('}').map(|p| j + p) {
                                let env = &text[j + 1..close];
                                if !env.contains('\n') {
                                    push(j, j + 1, Tok::Brace, &mut out);
                                    push(j + 1, close, Tok::Env, &mut out);
                                    push(close, close + 1, Tok::Brace, &mut out);
                                    if MATH_ENVS.contains(&env) {
                                        math = if name == "begin" { M::Env } else { M::None };
                                    }
                                    j = close + 1;
                                }
                            }
                        }
                        i = j;
                        continue;
                    }
                    let t = if in_math { Tok::MathCmd } else { Tok::Command };
                    push(i, j, t, &mut out);
                    if !in_math {
                        if HEADINGS.contains(&name) {
                            pending = Some(Arg::Heading);
                        } else if is_cite_cmd(name) {
                            pending = Some(Arg::Cite);
                        } else if is_ref_cmd(name) {
                            pending = Some(Arg::Ref);
                        }
                    }
                    i = j;
                } else if j < n {
                    let ch = b[j];
                    let clen = text[j..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                    match ch {
                        b'(' if math == M::None => {
                            math = M::Paren;
                            push(i, j + 1, Tok::MathCmd, &mut out);
                        }
                        b'[' if math == M::None => {
                            math = M::Bracket;
                            push(i, j + 1, Tok::MathCmd, &mut out);
                        }
                        b')' if math == M::Paren => {
                            push(i, j + 1, Tok::MathCmd, &mut out);
                            math = M::None;
                        }
                        b']' if math == M::Bracket => {
                            push(i, j + 1, Tok::MathCmd, &mut out);
                            math = M::None;
                        }
                        _ => push(i, j + clen, if in_math { Tok::MathCmd } else { Tok::Special }, &mut out),
                    }
                    i = j + clen;
                } else {
                    push(i, n, Tok::Special, &mut out);
                    i = n;
                }
            }
            b'$' => {
                let dd = i + 1 < n && b[i + 1] == b'$';
                let w = if dd { 2 } else { 1 };
                push(i, i + w, Tok::MathCmd, &mut out);
                math = match (math, dd) {
                    (M::None, true) => M::DDollar,
                    (M::None, false) => M::Dollar,
                    (M::DDollar, true) | (M::Dollar, false) => M::None,
                    (m, _) => m,
                };
                i += w;
            }
            b'{' if pending.is_some() && !in_math => {
                // argument group with special styling
                let mut depth = 0;
                let mut j = i;
                while j < n {
                    match b[j] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        b'\n' if j + 1 < n && b[j + 1] == b'\n' => break,
                        _ => {}
                    }
                    j += 1;
                }
                let t = match pending.take().unwrap() {
                    Arg::Heading => Tok::Heading,
                    Arg::Cite => Tok::Cite,
                    Arg::Ref => Tok::Ref,
                };
                push(i, i + 1, Tok::Brace, &mut out);
                if t == Tok::Heading {
                    // allow nested commands inside headings to be colored as heading text
                    push(i + 1, j.min(n), t, &mut out);
                } else {
                    push(i + 1, j.min(n), t, &mut out);
                }
                if j < n {
                    push(j, j + 1, Tok::Brace, &mut out);
                    i = j + 1;
                } else {
                    i = n;
                }
            }
            b'[' if pending.is_some() => {
                let e = text[i..].find(']').map(|p| i + p + 1).unwrap_or(n);
                push(i, e, Tok::Brace, &mut out);
                i = e;
            }
            b'{' | b'}' | b'[' | b']' => {
                push(i, i + 1, if in_math { Tok::Math } else { Tok::Brace }, &mut out);
                pending = None;
                i += 1;
            }
            b'&' | b'~' => {
                push(i, i + 1, Tok::Special, &mut out);
                i += 1;
            }
            b'^' | b'_' if in_math => {
                push(i, i + 1, Tok::MathCmd, &mut out);
                i += 1;
            }
            _ => {
                let mut j = i + 1;
                while j < n && !matches!(b[j], b'%' | b'\\' | b'$' | b'{' | b'}' | b'[' | b']' | b'&' | b'~' | b'^' | b'_') {
                    j += 1;
                }
                if pending.is_some() && !text[i..j].trim().is_empty() {
                    pending = None;
                }
                push(i, j, if in_math { Tok::Math } else { Tok::Text }, &mut out);
                i = j;
            }
        }
    }
    out
}

pub fn highlight(text: &str, syn: &Syntax, size: f32) -> LayoutJob {
    let mono = FontId::new(size, FontFamily::Monospace);
    let bold = FontId::new(size, FontFamily::Name("mono-bold".into()));
    let mut job = LayoutJob { text: text.to_string(), ..Default::default() };
    for (s, e, t) in tokenize(text) {
        let (color, font, italics) = match t {
            Tok::Text => (syn.text, &mono, false),
            Tok::Command => (syn.command, &mono, false),
            Tok::Keyword => (syn.keyword, &mono, false),
            Tok::Env => (syn.env, &mono, false),
            Tok::Math => (syn.math, &mono, false),
            Tok::MathCmd => (syn.math_cmd, &mono, false),
            Tok::Comment => (syn.comment, &mono, true),
            Tok::Brace => (syn.brace, &mono, false),
            Tok::Special => (syn.special, &mono, false),
            Tok::Cite => (syn.cite, &mono, false),
            Tok::Ref => (syn.refc, &mono, false),
            Tok::Heading => (syn.heading, &bold, false),
        };
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: egui::text::ByteIndex(s)..egui::text::ByteIndex(e),
            format: TextFormat { font_id: font.clone(), color, italics, ..Default::default() },
        });
    }
    if job.sections.is_empty() {
        job.sections.push(LayoutSection { leading_space: 0.0, byte_range: egui::text::ByteIndex(0)..egui::text::ByteIndex(0), format: TextFormat { font_id: mono, color: syn.text, ..Default::default() } });
    }
    job
}

// ───────────────────────────── completion ─────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CompKind {
    Cite,
    Ref,
    Env,
    Command,
    File,
}

#[derive(Clone, Debug)]
pub struct CompItem {
    pub label: String,
    pub detail: String,
    pub insert: String,
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub kind: CompKind,
    pub start: usize, // char index where the typed prefix starts
    pub items: Vec<CompItem>,
    pub selected: usize,
}

pub struct CompletionSources<'a> {
    pub cites: &'a [CompItem],
    pub labels: &'a [String],
    pub files: &'a [String],
}

pub const ENVIRONMENTS: &[&str] = &[
    "figure", "table", "itemize", "enumerate", "description", "equation", "equation*", "align", "align*", "frame", "columns",
    "column", "block", "exampleblock", "alertblock", "tabular", "tabularx", "center", "minipage", "quote", "abstract",
    "tikzpicture", "axis", "subfigure", "lstlisting", "verbatim", "gather", "cases", "pmatrix", "bmatrix", "theorem", "proof",
];

/// Command completions: (name, inserted text with `$0` cursor, description DE, description EN).
pub const COMMANDS: &[(&str, &str, &str, &str)] = &[
    ("section", "section{$0}", "Abschnitt", "Section"),
    ("subsection", "subsection{$0}", "Unterabschnitt", "Subsection"),
    ("subsubsection", "subsubsection{$0}", "Unter-Unterabschnitt", "Subsubsection"),
    ("chapter", "chapter{$0}", "Kapitel", "Chapter"),
    ("paragraph", "paragraph{$0}", "Absatzüberschrift", "Paragraph heading"),
    ("textbf", "textbf{$0}", "Fett", "Bold"),
    ("textit", "textit{$0}", "Kursiv", "Italic"),
    ("emph", "emph{$0}", "Hervorhebung", "Emphasis"),
    ("texttt", "texttt{$0}", "Schreibmaschine", "Monospace"),
    ("textsc", "textsc{$0}", "Kapitälchen", "Small caps"),
    ("underline", "underline{$0}", "Unterstrichen", "Underline"),
    ("enquote", "enquote{$0}", "Anführungszeichen", "Quotation marks"),
    ("footnote", "footnote{$0}", "Fußnote", "Footnote"),
    ("cite", "cite{$0}", "Zitat", "Citation"),
    ("citep", "citep{$0}", "Zitat (Autor, Jahr)", "Citation (Author, Year)"),
    ("citet", "citet{$0}", "Zitat Autor (Jahr)", "Citation Author (Year)"),
    ("parencite", "parencite{$0}", "Zitat in Klammern", "Parenthetical citation"),
    ("textcite", "textcite{$0}", "Zitat im Text", "Textual citation"),
    ("ref", "ref{$0}", "Verweis (Nummer)", "Reference (number)"),
    ("cref", "cref{$0}", "Verweis mit Typ", "Reference with type"),
    ("Cref", "Cref{$0}", "Verweis mit Typ (Satzanfang)", "Reference with type (capitalised)"),
    ("eqref", "eqref{$0}", "Gleichungsverweis", "Equation reference"),
    ("pageref", "pageref{$0}", "Seitenverweis", "Page reference"),
    ("label", "label{$0}", "Marke für Verweise", "Label for references"),
    ("includegraphics", "includegraphics[width=0.8\\linewidth]{$0}", "Bild einbinden", "Include image"),
    ("caption", "caption{$0}", "Beschriftung", "Caption"),
    ("centering", "centering", "Zentrieren", "Center"),
    ("item", "item $0", "Listenpunkt", "List item"),
    ("input", "input{$0}", "Datei einfügen", "Input file"),
    ("include", "include{$0}", "Kapiteldatei einbinden", "Include chapter file"),
    ("usepackage", "usepackage{$0}", "Paket laden", "Load package"),
    ("newcommand", "newcommand{\\$0}{}", "Eigener Befehl", "New command"),
    ("frac", "frac{$0}{}", "Bruch", "Fraction"),
    ("sqrt", "sqrt{$0}", "Wurzel", "Square root"),
    ("sum", "sum_{$0}^{}", "Summe", "Sum"),
    ("prod", "prod_{$0}^{}", "Produkt", "Product"),
    ("int", "int_{$0}^{}", "Integral", "Integral"),
    ("lim", "lim_{$0}", "Grenzwert", "Limit"),
    ("mathbb", "mathbb{$0}", "Zahlenmengen (ℝ, ℕ …)", "Blackboard bold"),
    ("mathcal", "mathcal{$0}", "Kalligrafisch", "Calligraphic"),
    ("mathrm", "mathrm{$0}", "Aufrecht in Mathe", "Upright in math"),
    ("text", "text{$0}", "Text in Formel", "Text in math"),
    ("left", "left( $0 \\right)", "Wachsende Klammern", "Scaling brackets"),
    ("cdot", "cdot", "Malpunkt ·", "Center dot ·"),
    ("times", "times", "Kreuz ×", "Times ×"),
    ("approx", "approx", "≈", "≈"),
    ("leq", "leq", "≤", "≤"),
    ("geq", "geq", "≥", "≥"),
    ("neq", "neq", "≠", "≠"),
    ("infty", "infty", "∞", "∞"),
    ("rightarrow", "rightarrow", "→", "→"),
    ("Rightarrow", "Rightarrow", "⇒", "⇒"),
    ("alpha", "alpha", "α", "α"),
    ("beta", "beta", "β", "β"),
    ("gamma", "gamma", "γ", "γ"),
    ("delta", "delta", "δ", "δ"),
    ("epsilon", "epsilon", "ε", "ε"),
    ("lambda", "lambda", "λ", "λ"),
    ("mu", "mu", "μ", "μ"),
    ("pi", "pi", "π", "π"),
    ("sigma", "sigma", "σ", "σ"),
    ("theta", "theta", "θ", "θ"),
    ("omega", "omega", "ω", "ω"),
    ("url", "url{$0}", "Link", "URL"),
    ("href", "href{$0}{}", "Link mit Text", "Link with text"),
    ("num", "num{$0}", "Zahl (siunitx)", "Number (siunitx)"),
    ("SI", "SI{$0}{}", "Wert mit Einheit (siunitx)", "Value with unit (siunitx)"),
    ("toprule", "toprule", "Tabellenlinie oben", "Top rule"),
    ("midrule", "midrule", "Tabellenlinie Mitte", "Mid rule"),
    ("bottomrule", "bottomrule", "Tabellenlinie unten", "Bottom rule"),
    ("hline", "hline", "Horizontale Linie", "Horizontal line"),
    ("newpage", "newpage", "Neue Seite", "New page"),
    ("clearpage", "clearpage", "Neue Seite (Gleitobjekte ausgeben)", "Clear page"),
    ("tableofcontents", "tableofcontents", "Inhaltsverzeichnis", "Table of contents"),
    ("listoffigures", "listoffigures", "Abbildungsverzeichnis", "List of figures"),
    ("listoftables", "listoftables", "Tabellenverzeichnis", "List of tables"),
    ("printbibliography", "printbibliography", "Literaturverzeichnis", "Bibliography"),
    ("vspace", "vspace{$0}", "Vertikaler Abstand", "Vertical space"),
    ("hspace", "hspace{$0}", "Horizontaler Abstand", "Horizontal space"),
    ("noindent", "noindent", "Kein Einzug", "No indent"),
    ("textwidth", "textwidth", "Textbreite", "Text width"),
    ("linewidth", "linewidth", "Zeilenbreite", "Line width"),
    ("ldots", "ldots", "Auslassung …", "Ellipsis …"),
    ("dots", "dots", "Auslassung …", "Ellipsis …"),
    ("textdegree", "textdegree", "Grad °", "Degree °"),
    ("frametitle", "frametitle{$0}", "Folientitel", "Frame title"),
    ("framesubtitle", "framesubtitle{$0}", "Folienuntertitel", "Frame subtitle"),
    ("pause", "pause", "Schrittweise aufdecken", "Reveal step by step"),
    ("alert", "alert{$0}", "Hervorheben (Folie)", "Alert (slide)"),
    ("only", "only<$0>{}", "Nur auf Folie …", "Only on overlay …"),
    ("onslide", "onslide<$0>{}", "Ab Folie …", "On overlay …"),
    ("maketitle", "maketitle", "Titel setzen", "Make title"),
    ("appendix", "appendix", "Anhang beginnen", "Start appendix"),
    ("todo", "todo{$0}", "Notiz am Rand (todonotes)", "Margin note (todonotes)"),
];

/// Common packages for `\usepackage{…}`.
pub const PACKAGES: &[(&str, &str, &str)] = &[
    ("amsmath", "Mathematik-Umgebungen", "Math environments"),
    ("amssymb", "Mathe-Symbole", "Math symbols"),
    ("mathtools", "Erweiterungen zu amsmath", "amsmath extensions"),
    ("graphicx", "Bilder einbinden", "Include images"),
    ("booktabs", "Schöne Tabellenlinien", "Nice table rules"),
    ("tabularx", "Tabellen mit fester Breite", "Fixed-width tables"),
    ("siunitx", "Zahlen und Einheiten", "Numbers and units"),
    ("hyperref", "Links im PDF", "Links in the PDF"),
    ("cleveref", "Verweise mit Typ (\\cref)", "Typed references (\\cref)"),
    ("biblatex", "Literaturverzeichnis", "Bibliography"),
    ("natbib", "Zitierbefehle \\citep/\\citet", "Citation commands"),
    ("csquotes", "Anführungszeichen (\\enquote)", "Quotation marks"),
    ("babel", "Sprachen und Silbentrennung", "Languages and hyphenation"),
    ("geometry", "Seitenränder", "Page margins"),
    ("xcolor", "Farben", "Colors"),
    ("tikz", "Grafiken zeichnen", "Drawings"),
    ("pgfplots", "Diagramme", "Plots"),
    ("listings", "Quellcode", "Source code"),
    ("minted", "Quellcode mit Syntaxhervorhebung", "Highlighted source code"),
    ("subcaption", "Teilabbildungen", "Subfigures"),
    ("caption", "Beschriftungen anpassen", "Caption styles"),
    ("enumitem", "Listen anpassen", "List layout"),
    ("microtype", "Feinere Typografie", "Micro-typography"),
    ("todonotes", "Randnotizen", "Margin notes"),
    ("float", "Gleitobjekte fixieren [H]", "Float placement [H]"),
    ("acronym", "Abkürzungsverzeichnis", "Acronyms"),
    ("glossaries", "Glossar", "Glossary"),
    ("lipsum", "Blindtext", "Dummy text"),
];

fn desc(de: &'static str, en: &'static str) -> String {
    if crate::i18n::en() { en.to_string() } else { de.to_string() }
}

/// Kind of a label, guessed from its prefix (fig:, tab:, …).
fn label_kind(l: &str) -> String {
    let (de, en) = match l.split(':').next().unwrap_or("") {
        "fig" => ("Abbildung", "Figure"),
        "tab" => ("Tabelle", "Table"),
        "eq" => ("Gleichung", "Equation"),
        "ch" | "cha" | "chap" => ("Kapitel", "Chapter"),
        "sec" | "ssec" => ("Abschnitt", "Section"),
        "lst" => ("Quellcode", "Listing"),
        "app" | "appdx" => ("Anhang", "Appendix"),
        _ => ("Marke", "Label"),
    };
    desc(de, en)
}

/// How often `\name` is used in the text (for ranking).
fn usage(text: &str, name: &str) -> i32 {
    text.matches(&format!("\\{name}")).count().min(30) as i32
}


fn fuzzy_score(hay: &str, needle: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let h = hay.to_lowercase();
    let n = needle.to_lowercase();
    if h.starts_with(&n) {
        return Some(100 - h.len() as i32);
    }
    if let Some(p) = h.find(&n) {
        return Some(50 - p as i32);
    }
    // subsequence
    let mut it = h.chars();
    for c in n.chars() {
        it.by_ref().find(|&x| x == c)?;
    }
    Some(10)
}

fn compute_completion(text: &str, cursor: usize, src: &CompletionSources) -> Option<Completion> {
    let bcur = char_to_byte(text, cursor);
    let line_start = text[..bcur].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let before = &text[line_start..bcur];
    // inside \cmd[..]{prefix
    let re_arg = regex::Regex::new(r"\\([A-Za-z]+)\*?(?:\[[^\]]*\])*\{([^{}]*)$").unwrap();
    if let Some(c) = re_arg.captures(before) {
        let cmd = &c[1];
        let arg = c.get(2).unwrap();
        // for comma separated lists only the last part
        let part_off = arg.as_str().rfind(',').map(|p| p + 1).unwrap_or(0);
        let prefix = arg.as_str()[part_off..].trim_start();
        let prefix_start_b = line_start + arg.start() + part_off + (arg.as_str()[part_off..].len() - prefix.len());
        let start = byte_to_char(text, prefix_start_b);
        let (kind, mut items): (CompKind, Vec<(i32, CompItem)>) = if is_cite_cmd(cmd) {
            (
                CompKind::Cite,
                src.cites
                    .iter()
                    .filter_map(|it| fuzzy_score(&format!("{} {}", it.label, it.detail), prefix).map(|s| (s, it.clone())))
                    .collect(),
            )
        } else if matches!(cmd, "ref" | "eqref" | "cref" | "Cref" | "autoref" | "pageref" | "vref" | "nameref") {
            (
                CompKind::Ref,
                src.labels
                    .iter()
                    .filter_map(|l| fuzzy_score(l, prefix).map(|s| (s, CompItem { label: l.clone(), detail: label_kind(l), insert: l.clone() })))
                    .collect(),
            )
        } else if cmd == "begin" || cmd == "end" {
            (
                CompKind::Env,
                {
                    // environments used in the file first, then the common ones
                    let used = regex::Regex::new(r"\\begin\{([A-Za-z*]+)\}").unwrap();
                    let mut envs: Vec<String> = used.captures_iter(text).map(|c| c[1].to_string()).collect();
                    envs.extend(ENVIRONMENTS.iter().map(|e| e.to_string()));
                    let mut seen = std::collections::HashSet::new();
                    envs.retain(|e| seen.insert(e.clone()));
                    let indent: String = before.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                    envs.into_iter()
                        .filter_map(|e| {
                            fuzzy_score(&e, prefix).map(|s| {
                                let insert = if cmd == "begin" {
                                    let (block, caret) = crate::smart::env_block(&e, &indent);
                                    let mut b: Vec<char> = block.chars().collect();
                                    b.splice(caret..caret, "$0".chars());
                                    format!("{e}}}{}", b.into_iter().collect::<String>())
                                } else {
                                    format!("{e}}}")
                                };
                                let s = s + usage(text, &format!("begin{{{e}}}")) * 3;
                                (s, CompItem { label: e.clone(), detail: tr!("Umgebung" | "Environment").into(), insert })
                            })
                        })
                        .collect()
                },
            )
        } else if cmd == "usepackage" || cmd == "RequirePackage" {
            (
                CompKind::Command,
                PACKAGES
                    .iter()
                    .filter_map(|(p, de, en)| fuzzy_score(p, prefix).map(|s| (s, CompItem { label: p.to_string(), detail: desc(de, en), insert: format!("{p}}}") })))
                    .collect(),
            )
        } else if matches!(cmd, "input" | "include" | "includegraphics") {
            (
                CompKind::File,
                src.files
                    .iter()
                    .filter(|f| if cmd == "includegraphics" { !f.ends_with(".tex") } else { f.ends_with(".tex") })
                    .filter_map(|f| {
                        let ins = if cmd == "includegraphics" { f.clone() } else { f.trim_end_matches(".tex").to_string() };
                        fuzzy_score(f, prefix).map(|s| (s, CompItem { label: f.clone(), detail: String::new(), insert: ins }))
                    })
                    .collect(),
            )
        } else {
            return None;
        };
        if items.is_empty() {
            return None;
        }
        items.sort_by(|a, b| b.0.cmp(&a.0));
        return Some(Completion { kind, start, items: items.into_iter().take(40).map(|x| x.1).collect(), selected: 0 });
    }
    // \comm  (command name)
    let re_cmd = regex::Regex::new(r"\\([A-Za-z]+)$").unwrap();
    if let Some(c) = re_cmd.captures(before) {
        let m = c.get(1).unwrap();
        let prefix = m.as_str();
        let start = byte_to_char(text, line_start + m.start());
        let mut items: Vec<(i32, CompItem)> = COMMANDS
            .iter()
            .filter(|(n, ..)| *n != prefix && (n.starts_with(prefix) || prefix.len() >= 3))
            .filter_map(|(n, ins, de, en)| {
                fuzzy_score(n, prefix).map(|s| (s + usage(text, n) * 4, CompItem { label: format!("\\{n}"), detail: desc(de, en), insert: ins.to_string() }))
            })
            .collect();
        // commands defined or used in the project that aren't in the list
        let user = regex::Regex::new(r"\\([A-Za-z]{3,})").unwrap();
        let mut extra: Vec<String> = user.captures_iter(text).map(|c| c[1].to_string()).filter(|n| n.starts_with(prefix) && n != prefix && !COMMANDS.iter().any(|(c, ..)| c == n)).collect();
        extra.sort();
        extra.dedup();
        for n in extra.into_iter().take(8) {
            let u = usage(text, &n);
            items.push((40 + u * 4, CompItem { label: format!("\\{n}"), detail: tr!("im Dokument" | "in document").into(), insert: n }));
        }
        if prefix == "beg" || prefix == "begi" || prefix == "begin" {
            items.insert(0, (200, CompItem { label: "\\begin{…}".into(), detail: tr!("Umgebung" | "Environment").into(), insert: "begin{$0".into() }));
        }
        if items.is_empty() {
            return None;
        }
        items.sort_by(|a, b| b.0.cmp(&a.0));
        return Some(Completion { kind: CompKind::Command, start, items: items.into_iter().take(12).map(|x| x.1).collect(), selected: 0 });
    }
    None
}

// ───────────────────────────── editor widget ─────────────────────────────

pub struct EditorStyle<'a> {
    pub pal: &'a Palette,
    pub syntax: &'a Syntax,
    pub font_size: f32,
    pub style_rev: u64,
    pub issues: &'a HashMap<usize, (Level, String)>,
    pub visual: bool,
    pub embedded: bool,
    pub git_marks: &'a [crate::git::LineMark],
    /// Active search query (find bar open) – matches get highlighted.
    pub search: Option<&'a str>,
    /// Visual mode: citation key → "Author Year".
    pub cite_labels: &'a HashMap<String, String>,
    /// Visual mode: line → heading number ("2.1") for this file.
    pub heading_numbers: &'a HashMap<usize, String>,
}

pub struct EditorOutput {
    pub changed: bool,
    pub ctrl_click_line: Option<usize>,
    pub vim: Option<crate::vim::VimOut>,
}

fn toggle_comment(buf: &mut Buffer) {
    let (a, b) = (buf.cursor.min(buf.sel_end), buf.cursor.max(buf.sel_end));
    let ba = char_to_byte(&buf.text, a);
    let bb = char_to_byte(&buf.text, b);
    let ls = buf.text[..ba].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let le = buf.text[bb..].find('\n').map(|p| bb + p).unwrap_or(buf.text.len());
    let block = &buf.text[ls..le];
    let all_commented = block.lines().filter(|l| !l.trim().is_empty()).all(|l| l.trim_start().starts_with('%'));
    let new: Vec<String> = block
        .split('\n')
        .map(|l| {
            if all_commented {
                if let Some(p) = l.find('%') {
                    let mut s = l.to_string();
                    let rm = if l[p + 1..].starts_with(' ') { 2 } else { 1 };
                    s.replace_range(p..p + rm, "");
                    s
                } else {
                    l.to_string()
                }
            } else if l.trim().is_empty() {
                l.to_string()
            } else {
                let ind = l.len() - l.trim_start().len();
                format!("{}% {}", &l[..ind], &l[ind..])
            }
        })
        .collect();
    let new = new.join("\n");
    let sc = byte_to_char(&buf.text, ls);
    let len = new.chars().count();
    buf.text.replace_range(ls..le, &new);
    buf.select(sc, sc + len);
}

pub fn editor_ui(ui: &mut egui::Ui, buf: &mut Buffer, st: &EditorStyle, src: &CompletionSources, mut vim: Option<&mut crate::vim::VimState>) -> EditorOutput {
    let ctx = ui.ctx().clone();
    let mut out = EditorOutput { changed: false, ctrl_click_line: None, vim: None };
    let has_focus = ctx.memory(|m| m.has_focus(buf.id));

    // keep cursor fresh from last frame state
    if let Some(state) = egui::TextEdit::load_state(&ctx, buf.id) {
        if let Some(r) = state.cursor.char_range() {
            buf.cursor = r.primary.index.0;
            buf.sel_end = r.secondary.index.0;
        }
    }

    // ── vim emulation (code mode) ──
    let mut vim_normal = false;
    if has_focus {
        if let Some(v) = vim.as_deref_mut() {
            let vo = v.handle(&ctx, buf, buf.completion.is_some());
            if vo.changed {
                out.changed = true;
                buf.completion = None;
            }
            // undo / redo through egui's TextEdit undoer
            ctx.input_mut(|i| {
                for _ in 0..vo.undo {
                    i.events.push(egui::Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
                }
                for _ in 0..vo.redo {
                    i.events.push(egui::Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND | Modifiers::SHIFT });
                }
            });
            vim_normal = v.mode != crate::vim::Mode::Insert;
            buf.vim_block = Some(v.block_pos());
            buf.vim_block_sel = v.block_ranges(&buf.text);
            out.vim = Some(vo);
        }
    }
    if vim.is_none() {
        buf.vim_block = None;
        buf.vim_block_sel.clear();
    } else if !has_focus {
        // keep showing the block where we left off
        buf.vim_block = buf.vim_block.map(|b| b.or(Some(buf.cursor)));
    }

    // ── smart typing: bracket pairs, automatic \end{…}, list continuation, indentation ──
    if has_focus && !vim_normal && buf.completion.is_none() {
        let evs = ctx.input_mut(|i| std::mem::take(&mut i.events));
        let mut keep = Vec::with_capacity(evs.len());
        let mut earlier_edit = false; // only transform when nothing else edits before us
        for e in evs {
            let sel = (buf.sel_end, buf.cursor);
            let collapsed = sel.0 == sel.1;
            let handled = if earlier_edit {
                None
            } else {
                match &e {
                    egui::Event::Text(t) if t.chars().count() == 1 => crate::smart::on_char(&buf.text, sel, t.chars().next().unwrap()),
                    egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if modifiers.is_none() && collapsed => {
                        Some(crate::smart::on_enter(&buf.text, buf.cursor))
                    }
                    egui::Event::Key { key: Key::Backspace, pressed: true, modifiers, .. } if modifiers.is_none() && collapsed => {
                        crate::smart::on_backspace(&buf.text, buf.cursor)
                    }
                    _ => None,
                }
            };
            match handled {
                Some(ed) => {
                    if ed.text != buf.text {
                        buf.text = ed.text;
                        out.changed = true;
                    }
                    buf.cursor = ed.cursor;
                    buf.sel_end = ed.cursor;
                    buf.place_cursor(ed.cursor, ed.cursor);
                }
                None => {
                    if matches!(e, egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Key { pressed: true, .. }) {
                        earlier_edit = true;
                    }
                    keep.push(e);
                }
            }
        }
        ctx.input_mut(|i| i.events = keep);
    }

    // ── key handling before the TextEdit sees the events ──
    let mut accept: Option<CompItem> = None;
    if has_focus && !vim_normal {
        if let Some(comp) = &mut buf.completion {
            let n = comp.items.len();
            ctx.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    comp.selected = (comp.selected + 1) % n;
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    comp.selected = (comp.selected + n - 1) % n;
                }
                if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab) {
                    accept = comp.items.get(comp.selected).cloned();
                }
            });
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                buf.completion = None;
            }
        } else {
            let (tab, bold, ital, comment, shift_tab) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Tab),
                    i.consume_key(Modifiers::COMMAND, Key::B),
                    i.consume_key(Modifiers::COMMAND, Key::I),
                    i.consume_key(Modifiers::COMMAND, Key::Slash) || i.consume_key(Modifiers::COMMAND, Key::Num7),
                    i.consume_key(Modifiers::SHIFT, Key::Tab),
                )
            });
            if tab {
                buf.insert_snippet("  $0");
                out.changed = true;
            }
            if shift_tab {
                // outdent current line
                let bl = char_to_byte(&buf.text, buf.cursor);
                let ls = buf.text[..bl].rfind('\n').map(|p| p + 1).unwrap_or(0);
                let rm = buf.text[ls..].chars().take(2).take_while(|c| *c == ' ').count();
                if rm > 0 {
                    buf.text.replace_range(ls..ls + rm, "");
                    let c = buf.cursor.saturating_sub(rm);
                    buf.select(c, c);
                    out.changed = true;
                }
            }
            if bold {
                buf.insert_snippet("\\textbf{$SEL$0}");
                out.changed = true;
            }
            if ital {
                buf.insert_snippet("\\textit{$SEL$0}");
                out.changed = true;
            }
            if comment {
                toggle_comment(buf);
                out.changed = true;
            }
        }
    }
    if let Some(item) = accept {
        let comp = buf.completion.take().unwrap();
        apply_completion(buf, &comp, &item);
        out.changed = true;
    }

    // place the cursor at the start of a freshly opened file (no scrolling, no focus)
    if buf.init_cursor {
        buf.init_cursor = false;
        if egui::TextEdit::load_state(&ctx, buf.id).is_none() {
            let mut state = egui::text_edit::TextEditState::default();
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            state.store(&ctx, buf.id);
            buf.cursor = 0;
            buf.sel_end = 0;
        }
    }
    // apply pending selection
    let mut scroll_to_cursor = false;
    if let Some((a, b)) = buf.pending_select.take() {
        let mut state = egui::TextEdit::load_state(&ctx, buf.id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
        state.store(&ctx, buf.id);
        buf.cursor = b;
        buf.sel_end = a;
        scroll_to_cursor = buf.pending_scroll;
    }
    // Focus is requested only once no mouse button is pressed: a click elsewhere (file tree,
    // quick open, tabs) would otherwise take the focus away again in the same frame.
    //
    // The request is repeated until the editor really has the focus: right after a modal
    // (quick open, dialogs) closes, egui still treats it as open for one frame and ignores
    // focus requests for widgets behind it.
    if buf.request_focus {
        if ctx.memory(|m| m.has_focus(buf.id)) {
            buf.request_focus = false;
            buf.focus_tries = 0;
        } else if ctx.input(|i| i.pointer.any_down() || i.pointer.any_pressed() || i.pointer.any_released()) {
            ctx.request_repaint();
        } else if buf.focus_tries > 10 {
            buf.request_focus = false;
            buf.focus_tries = 0;
        } else {
            ctx.memory_mut(|m| m.request_focus(buf.id));
            buf.focus_tries += 1;
            ctx.request_repaint();
        }
    }

    let pal = st.pal;
    let prev_cursor = buf.cursor;
    let (output, galley, gpos) = if st.embedded {
        editor_core(ui, buf, st, has_focus, scroll_to_cursor, &mut out, 0.0)
    } else {
        let avail_h = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt(("editor-scroll", buf.id, st.visual))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if st.visual {
                    let avail_w = ui.available_width();
                    // leave a margin on the left for heading numbers
                    let col = (avail_w - 140.0).clamp(240.0, 780.0);
                    let pad = ((avail_w - col) / 2.0).max(0.0);
                    let clip = ui.clip_rect();
                    // paper
                    let paper = egui::Rect::from_min_max(pos2(ui.min_rect().min.x + pad - 44.0, clip.min.y - 1.0), pos2(ui.min_rect().min.x + pad + col + 44.0, clip.max.y + 1.0));
                    ui.painter().rect_filled(paper, 0.0, pal.base);
                    ui.painter().line_segment([paper.left_top(), paper.left_bottom()], Stroke::new(1.0, with_alpha(pal.border, 90)));
                    ui.painter().line_segment([paper.right_top(), paper.right_bottom()], Stroke::new(1.0, with_alpha(pal.border, 90)));
                    ui.horizontal_top(|ui| {
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_width(col);
                            editor_core(ui, buf, st, has_focus, scroll_to_cursor, &mut out, avail_h)
                        })
                        .inner
                    })
                    .inner
                } else {
                    editor_core(ui, buf, st, has_focus, scroll_to_cursor, &mut out, avail_h)
                }
            })
            .inner
    };

    if output.response.changed() {
        out.changed = true;
    }
    if let Some(r) = output.cursor_range {
        buf.cursor = r.primary.index.0;
        buf.sel_end = r.secondary.index.0;
    }
    buf.cursor_moved = buf.cursor != prev_cursor;
    let before = &buf.text[..char_to_byte(&buf.text, buf.cursor)];
    buf.line = before.matches('\n').count() + 1;
    buf.col = before.rsplit('\n').next().map(|s| s.chars().count()).unwrap_or(0) + 1;

    // update completion
    if out.changed && ctx.memory(|m| m.has_focus(buf.id)) {
        buf.last_edit = ctx.input(|i| i.time);
        buf.completion = compute_completion(&buf.text, buf.cursor, src).map(|mut c| {
            if let Some(old) = &buf.completion {
                if old.kind == c.kind && old.start == c.start {
                    c.selected = old.selected.min(c.items.len().saturating_sub(1));
                }
            }
            c
        });
    } else if let Some(c) = &buf.completion {
        if buf.cursor < c.start || !ctx.memory(|m| m.has_focus(buf.id)) && !ctx.is_pointer_over_egui() {
            buf.completion = None;
        }
    }

    // completion popup
    if let Some(comp) = &mut buf.completion {
        let crect = galley.pos_from_cursor(CCursor::new(buf.cursor)).translate(gpos.to_vec2());
        let pos = pos2(crect.min.x - 8.0, crect.max.y + 4.0);
        let mut clicked: Option<usize> = None;
        egui::Area::new(buf.id.with("completion"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(pal.surface)
                    .stroke(Stroke::new(1.0, pal.border))
                    .inner_margin(egui::Margin::same(4))
                    .corner_radius(8)
                    .show(ui, |ui| {
                        ui.set_min_width(300.0);
                        ui.set_max_width(520.0);
                        let (icon, tint) = match comp.kind {
                            CompKind::Cite => (crate::icons::BOOK, pal.yellow),
                            CompKind::Ref => (crate::icons::LINK, pal.cyan),
                            CompKind::Env => (crate::icons::CUBE, pal.magenta),
                            CompKind::Command => (crate::icons::CODE, pal.blue),
                            CompKind::File => (crate::icons::FILE, pal.subtext),
                        };
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            for (i, it) in comp.items.iter().enumerate() {
                                let sel = i == comp.selected;
                                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), if it.detail.is_empty() { 26.0 } else { 40.0 }), egui::Sense::click());
                                if sel {
                                    ui.painter().rect_filled(rect, 6.0, with_alpha(pal.accent, 38));
                                    if comp_needs_scroll(ui, rect) {
                                        ui.scroll_to_rect(rect, None);
                                    }
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(rect, 6.0, pal.overlay);
                                }
                                let p = ui.painter();
                                p.text(pos2(rect.min.x + 14.0, rect.min.y + 13.0), egui::Align2::CENTER_CENTER, icon, FontId::proportional(12.0), tint);
                                p.text(pos2(rect.min.x + 28.0, rect.min.y + 13.0), egui::Align2::LEFT_CENTER, &it.label, FontId::new(13.0, FontFamily::Monospace), if sel { pal.bright } else { pal.text });
                                if !it.detail.is_empty() {
                                    let d: String = it.detail.chars().take(70).collect();
                                    p.text(pos2(rect.min.x + 28.0, rect.min.y + 29.0), egui::Align2::LEFT_CENTER, d, FontId::proportional(11.5), pal.dim);
                                }
                                if resp.clicked() {
                                    clicked = Some(i);
                                }
                            }
                        });
                    });
            });
        if let Some(i) = clicked {
            let comp = buf.completion.take().unwrap();
            let item = comp.items[i].clone();
            apply_completion(buf, &comp, &item);
            out.changed = true;
        }
    }
    if let Some(v) = vim.as_deref() {
        if has_focus || v.cmdline.is_some() {
            vim_cmdline(ui, v, pal);
        }
    }
    out
}


fn line_range_bytes(text: &str, ci: usize) -> (usize, usize) {
    let b = char_to_byte(text, ci);
    let ls = text[..b].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let le = text[b..].find('\n').map(|p| b + p).unwrap_or(text.len());
    (ls, le)
}

/// The TextEdit itself plus gutter / decorations. Returns output, galley and galley position.
fn editor_core(
    ui: &mut egui::Ui,
    buf: &mut Buffer,
    st: &EditorStyle,
    has_focus: bool,
    scroll_to_cursor: bool,
    out: &mut EditorOutput,
    min_h: f32,
) -> (egui::text_edit::TextEditOutput, std::sync::Arc<egui::Galley>, egui::Pos2) {
    let pal = st.pal;
    let visual = st.visual;
    let line_count = buf.text.matches('\n').count() + 1;
    let digits = line_count.to_string().len().max(3) as f32;
    let gutter = if visual { 0.0 } else { (digits * st.font_size * 0.6 + 30.0).min(120.0) };
    let row_h = st.font_size;
    let vsize = st.font_size + 3.0;
    let raw = if visual && (has_focus || buf.completion.is_some()) { Some(line_range_bytes(&buf.text, buf.cursor)) } else { None };

    let bg_idx = ui.painter().add(Shape::Noop);
    let search_idx = ui.painter().add(Shape::Noop);
    let vsel_idx = ui.painter().add(Shape::Noop);
    let chip_idx = ui.painter().add(Shape::Noop);
    let syn = st.syntax;
    let size = st.font_size;
    let rev = st.style_rev;
    let mut vtheme = crate::visual::VisualTheme::new(pal, vsize);
    if visual {
        vtheme.cites = st.cite_labels.clone();
    }
    let cites_key = st.cite_labels.len() as u64 ^ st.cite_labels.values().map(|v| v.len() as u64).sum::<u64>().rotate_left(11);
    let id = buf.id;
    let Buffer { text, hl_cache, vis_cache, .. } = buf;
    let mut layouter = |ui: &egui::Ui, tb: &dyn egui::TextBuffer, wrap: f32| {
        let s = tb.as_str();
        let th = hash_str(s);
        let job = if visual {
            if vis_cache.as_ref().is_none_or(|(h, _, _)| *h != th) {
                let (sp, de) = crate::visual::spans(s);
                *vis_cache = Some((th, sp, de));
            }
            let key = th ^ rev.rotate_left(7) ^ (vsize.to_bits() as u64).rotate_left(3) ^ raw.map_or(1, |(a, b)| (a as u64) << 20 ^ b as u64) ^ 0x5151 ^ cites_key.rotate_left(29);
            match hl_cache {
                Some((k, j)) if *k == key => j.clone(),
                _ => {
                    let j = crate::visual::layout_job(s, &vis_cache.as_ref().unwrap().1, &vtheme, raw);
                    *hl_cache = Some((key, j.clone()));
                    j
                }
            }
        } else {
            let key = th ^ rev.rotate_left(7) ^ (size.to_bits() as u64);
            match hl_cache {
                Some((k, j)) if *k == key => j.clone(),
                _ => {
                    let j = highlight(s, syn, size);
                    *hl_cache = Some((key, j.clone()));
                    j
                }
            }
        };
        let mut job = job;
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let margin = if visual {
        egui::Margin { left: 0, right: 0, top: if st.embedded { 6 } else { 36 }, bottom: if st.embedded { 6 } else { 80 } }
    } else {
        egui::Margin { left: gutter as i8, right: 16, top: 12, bottom: 40 }
    };
    let font = if visual { FontId::new(vsize, FontFamily::Name("serif".into())) } else { FontId::new(st.font_size, FontFamily::Monospace) };
    let vim_block = buf.vim_block;
    if vim_block.is_some() {
        // vim: we draw our own solid block cursor
        ui.visuals_mut().text_cursor.stroke = Stroke::NONE;
        ui.visuals_mut().text_cursor.blink = false;
    }
    let output = egui::TextEdit::multiline(text)
        .id(id)
        .font(font)
        .frame(egui::Frame::NONE.inner_margin(margin))
        .desired_width(f32::INFINITY)
        .min_size(vec2(0.0, (min_h - 4.0).max(0.0)))
        .lock_focus(true)
        .layouter(&mut layouter)
        .show(ui);
    let galley = output.galley.clone();
    let gpos = output.galley_pos;
    let clip = ui.clip_rect();
    let resp_rect = output.response.rect;
    let painter = ui.painter();

    if !visual {
        let gutter_rect = Rect::from_min_max(pos2(resp_rect.min.x, clip.min.y.max(resp_rect.min.y)), pos2(resp_rect.min.x + gutter - 12.0, clip.max.y.min(resp_rect.max.y)));
        painter.rect_filled(gutter_rect, 0.0, mix(pal.base, pal.mantle, 0.5));
    }

    // line bookkeeping: numbers, issue markers, cursor line
    let cur_line = buf.text[..char_to_byte(&buf.text, buf.cursor)].matches('\n').count() + 1;
    let mut line = 1usize;
    let mut new_par = true;
    let num_font = FontId::new(st.font_size * 0.86, FontFamily::Monospace);
    let mut cursor_rows: Option<(f32, f32)> = None;
    let left = if visual { resp_rect.min.x - 26.0 } else { resp_rect.min.x + gutter - 12.0 };
    // git change bars: line -> kind
    let mut gm: HashMap<usize, crate::git::MarkKind> = HashMap::new();
    for m in st.git_marks {
        if m.kind == crate::git::MarkKind::Deleted {
            gm.entry(m.start).or_insert(m.kind);
        } else {
            for l in m.start..m.start + m.count {
                gm.insert(l, m.kind);
            }
        }
    }
    let bar_x = if visual { resp_rect.min.x - 10.0 } else { resp_rect.min.x + gutter - 15.0 };
    for row in &galley.rows {
        let y0 = gpos.y + row.pos.y;
        let y1 = y0 + row.size.y.max(if visual { 1.0 } else { row_h });
        if line == cur_line {
            cursor_rows = Some(match cursor_rows {
                Some((a, _)) => (a, y1),
                None => (y0, y1),
            });
        }
        if y1 >= clip.min.y && y0 <= clip.max.y {
            if let Some(k) = gm.get(&line) {
                match k {
                    crate::git::MarkKind::Added => {
                        painter.rect_filled(Rect::from_min_max(pos2(bar_x - 1.5, y0), pos2(bar_x + 1.5, y1)), 1.0, with_alpha(pal.green, 200));
                    }
                    crate::git::MarkKind::Modified => {
                        painter.rect_filled(Rect::from_min_max(pos2(bar_x - 1.5, y0), pos2(bar_x + 1.5, y1)), 1.0, with_alpha(mix(pal.yellow, pal.orange, 0.35), 220));
                    }
                    crate::git::MarkKind::Deleted if new_par => {
                        let pts = vec![pos2(bar_x - 3.0, y0 - 4.0), pos2(bar_x + 3.0, y0), pos2(bar_x - 3.0, y0 + 4.0)];
                        painter.add(Shape::convex_polygon(pts, with_alpha(pal.red, 220), Stroke::NONE));
                    }
                    _ => {}
                }
            }
        }
        if new_par && y1 >= clip.min.y && y0 <= clip.max.y {
            if !visual {
                let color = if line == cur_line { pal.text } else { mix(pal.dim, pal.base, 0.2) };
                painter.text(pos2(resp_rect.min.x + gutter - 20.0, y0 + row.size.y * 0.5), egui::Align2::RIGHT_CENTER, line.to_string(), num_font.clone(), color);
            }
            if let Some((lvl, _msg)) = st.issues.get(&line) {
                let c = match lvl {
                    Level::Error => pal.red,
                    Level::Warning => pal.yellow,
                    Level::BadBox => pal.dim,
                };
                let dot_x = if visual { resp_rect.min.x - 18.0 } else { resp_rect.min.x + 8.0 };
                painter.circle_filled(pos2(dot_x, y0 + row.size.y * 0.5), 3.5, c);
                if row.size.y > 2.0 {
                    painter.rect_filled(Rect::from_min_max(pos2(left, y0), pos2(resp_rect.max.x, y0 + row.size.y)), 0.0, with_alpha(c, 18));
                }
            }
        }
        new_par = row.ends_with_newline;
        if row.ends_with_newline {
            line += 1;
        }
    }
    if let Some((a, b)) = cursor_rows {
        if has_focus || buf.completion.is_some() {
            let r = Rect::from_min_max(pos2(left, a), pos2(resp_rect.max.x + if visual { 26.0 } else { 0.0 }, b));
            if visual {
                painter.set(bg_idx, Shape::rect_filled(r.expand2(vec2(0.0, 2.0)), 6.0, with_alpha(pal.text, 7)));
            } else {
                painter.set(bg_idx, Shape::rect_filled(r, 0.0, with_alpha(pal.text, 9)));
            }
        }
    }
    if !visual {
        painter.line_segment(
            [pos2(resp_rect.min.x + gutter - 12.0, clip.min.y.max(resp_rect.min.y)), pos2(resp_rect.min.x + gutter - 12.0, clip.max.y.min(resp_rect.max.y))],
            Stroke::new(1.0, with_alpha(pal.border, 120)),
        );
    } else if let Some((_, spans, decor)) = &buf.vis_cache {
        // rounded chips behind \cite / \ref keys
        let mut chips = vec![];
        let (mut lb, mut lc) = (0usize, 0usize);
        for sp in spans.iter().filter(|s| matches!(s.st, crate::visual::Vs::Cite | crate::visual::Vs::Ref)) {
            if sp.e > buf.text.len() || raw.is_some_and(|(a, b)| sp.s < b && sp.e > a) {
                continue;
            }
            lc += buf.text[lb..sp.s].chars().count();
            lb = sp.s;
            let c0 = lc;
            let c1 = c0 + buf.text[sp.s..sp.e].chars().count();
            let r0 = galley.pos_from_cursor(CCursor::new(c0)).translate(gpos.to_vec2());
            let r1 = galley.pos_from_cursor(CCursor { index: c1.into(), prefer_next_row: false }).translate(gpos.to_vec2());
            if r0.max.y < clip.min.y || r0.min.y > clip.max.y || (r0.center().y - r1.center().y).abs() > 2.0 {
                continue;
            }
            let h = vsize * 1.05;
            let cy = r0.center().y - vsize * 0.02;
            let col = if sp.st == crate::visual::Vs::Cite { pal.accent } else { pal.cyan };
            let label = if sp.st == crate::visual::Vs::Cite { crate::visual::cite_label(&buf.text[sp.s..sp.e], &vtheme) } else { None };
            if let Some(label) = label {
                // keys are hidden; the reserved space in front of them holds "Author Year"
                let w = crate::visual::cite_width(&label, &vtheme);
                let rect = Rect::from_min_max(pos2(r0.min.x - w + 1.0, cy - h / 2.0), pos2(r0.min.x - 2.0, cy + h / 2.0));
                chips.push(Shape::rect_filled(rect, 5.0, with_alpha(col, 34)));
                let mut font = crate::visual::cite_font(&vtheme);
                let tc = mix(col, pal.bright, 0.35);
                let mut g = ui.painter().layout_no_wrap(label.clone(), font.clone(), tc);
                if g.size().x > rect.width() - 8.0 {
                    // estimated width was too small: shrink the text to fit the chip
                    font.size *= ((rect.width() - 8.0) / g.size().x).max(0.6);
                    g = ui.painter().layout_no_wrap(label, font, tc);
                }
                let gx = rect.min.x + (rect.width() - g.size().x) / 2.0;
                chips.push(Shape::galley(pos2(gx, rect.center().y - g.size().y / 2.0), g, pal.text));
            } else {
                let rect = Rect::from_min_max(pos2(r0.min.x - 4.0, cy - h / 2.0), pos2(r1.min.x + 4.0, cy + h / 2.0));
                chips.push(Shape::rect_filled(rect, 5.0, with_alpha(col, 30)));
            }
        }
        // typographic glyphs (°, …, –, “ ”) painted into the space reserved before hidden markup
        let (mut lb, mut lc) = (0usize, 0usize);
        for sp in spans.iter() {
            let crate::visual::Vs::Glyph(gl, ctxs) = sp.st else { continue };
            if sp.e > buf.text.len() || raw.is_some_and(|(a, b)| sp.s < b.max(a + 1) && sp.e > a) {
                continue;
            }
            lc += buf.text[lb..sp.s].chars().count();
            lb = sp.s;
            let r0 = galley.pos_from_cursor(CCursor::new(lc)).translate(gpos.to_vec2());
            if r0.max.y < clip.min.y || r0.min.y > clip.max.y {
                continue;
            }
            let base = crate::visual::glyph_format(*ctxs, &vtheme);
            let w = crate::visual::glyph_em(gl) * base.font_id.size;
            let g = ui.painter().layout_no_wrap(gl.to_string(), base.font_id.clone(), base.color);
            // align the glyph's baseline with the baseline of the normal text in that row
            let row = galley.rows.iter().find(|row| {
                let y = gpos.y + row.pos.y;
                y <= r0.center().y && r0.center().y <= y + row.size.y.max(1.0)
            });
            let baseline = row.and_then(|row| {
                row.glyphs.iter().max_by(|a, b| a.font_height.total_cmp(&b.font_height)).map(|gl| gpos.y + row.pos.y + gl.pos.y)
            });
            let own = g.rows.first().and_then(|r| r.glyphs.first()).map(|gl| gl.pos.y).unwrap_or(g.size().y * 0.8);
            let top = baseline.map(|b| b - own).unwrap_or(r0.center().y - g.size().y / 2.0);
            chips.push(Shape::galley(pos2(r0.min.x - w + (w - g.size().x) / 2.0, top), g, base.color));
        }
        painter.set(chip_idx, Shape::Vec(chips));
        let _ = &spans;
        // heading numbers in the left margin
        if !st.heading_numbers.is_empty() {
            let mut line = 1usize;
            let mut new_par = true;
            for row in &galley.rows {
                let y0 = gpos.y + row.pos.y;
                if new_par && row.size.y > 4.0 && y0 <= clip.max.y && y0 + row.size.y >= clip.min.y {
                    if let Some(num) = st.heading_numbers.get(&line) {
                        if raw.is_none_or(|(a, _)| buf.text[..a.min(buf.text.len())].matches('\n').count() + 1 != line) {
                            let f = FontId::new((row.size.y * 0.42).clamp(11.0, 22.0), FontFamily::Name("serif".into()));
                            painter.text(pos2(resp_rect.min.x - 16.0, y0 + row.size.y * 0.5), egui::Align2::RIGHT_CENTER, num, f, with_alpha(pal.accent, 150));
                        }
                    }
                }
                new_par = row.ends_with_newline;
                if row.ends_with_newline {
                    line += 1;
                }
            }
        }
        // bullets / numbers for hidden \item
        let (mut lb, mut lc) = (0usize, 0usize);
        for d in decor {
            if raw.is_some_and(|(a, _)| a == d.line_start) || d.byte > buf.text.len() {
                continue;
            }
            lc += buf.text[lb..d.byte].chars().count();
            lb = d.byte;
            let r = galley.pos_from_cursor(CCursor::new(lc)).translate(gpos.to_vec2());
            if r.max.y < clip.min.y || r.min.y > clip.max.y {
                continue;
            }
            let x = r.min.x + 6.0 + 22.0 * d.depth.saturating_sub(1) as f32;
            let y = r.center().y + 1.0;
            match d.kind {
                crate::visual::DecorKind::Bullet => {
                    if d.depth % 2 == 1 {
                        painter.circle_filled(pos2(x, y), 2.8, mix(pal.accent, pal.text, 0.3));
                    } else {
                        painter.circle_stroke(pos2(x, y), 2.6, Stroke::new(1.2, mix(pal.accent, pal.text, 0.3)));
                    }
                }
                crate::visual::DecorKind::Number(n) => {
                    painter.text(pos2(x + 8.0, y), egui::Align2::RIGHT_CENTER, format!("{n}."), FontId::new(vsize * 0.95, FontFamily::Name("serif".into())), mix(pal.accent, pal.text, 0.3));
                }
            }
        }
    }

    // ── search highlights ──
    if let Some(q) = st.search.filter(|q| !q.is_empty()) {
        let th = hash_str(&buf.text);
        if buf.search_cache.as_ref().is_none_or(|(h, cq, _)| *h != th || cq != q) {
            buf.search_cache = Some((th, q.to_string(), find_matches(&buf.text, q)));
        }
        let matches = &buf.search_cache.as_ref().unwrap().2;
        let (sa, sb) = (buf.cursor.min(buf.sel_end), buf.cursor.max(buf.sel_end));
        let current = matches.iter().position(|m| m.0 == sa && m.1 == sb);
        let now = ui.input(|i| i.time);
        if let Some(c) = current {
            if buf.search_pulse.0 != matches[c].0 {
                buf.search_pulse = (matches[c].0, now);
            }
        }
        let hit = mix(pal.yellow, pal.orange, 0.3);
        let mut shapes = vec![];
        let mut ticks = vec![];
        for (k, &(a, b)) in matches.iter().enumerate() {
            let r0 = galley.pos_from_cursor(CCursor::new(a)).translate(gpos.to_vec2());
            let r1 = galley.pos_from_cursor(CCursor { index: b.into(), prefer_next_row: false }).translate(gpos.to_vec2());
            let is_cur = current == Some(k);
            ticks.push((r0.center().y, is_cur));
            if r1.max.y < clip.min.y || r0.min.y > clip.max.y {
                continue;
            }
            // one rect per visual row the match spans
            let mut rects = vec![];
            if (r0.min.y - r1.min.y).abs() < 1.0 {
                rects.push(Rect::from_min_max(pos2(r0.min.x, r0.min.y), pos2(r1.min.x.max(r0.min.x + 3.0), r0.max.y)));
            } else {
                for row in &galley.rows {
                    let ry0 = gpos.y + row.pos.y;
                    let ry1 = ry0 + row.size.y;
                    if ry1 <= r0.min.y + 0.5 || ry0 >= r1.max.y - 0.5 {
                        continue;
                    }
                    let x0 = if (ry0 - r0.min.y).abs() < 1.0 { r0.min.x } else { gpos.x + row.pos.x };
                    let x1 = if (ry0 - r1.min.y).abs() < 1.0 { r1.min.x } else { gpos.x + row.pos.x + row.size.x };
                    rects.push(Rect::from_min_max(pos2(x0, ry0), pos2(x1.max(x0 + 3.0), ry1)));
                }
            }
            for r in rects {
                let r = r.expand2(vec2(1.5, 0.0));
                if is_cur {
                    let age = (now - buf.search_pulse.1) as f32;
                    let pulse = (1.0 - age / 0.6).clamp(0.0, 1.0);
                    if pulse > 0.0 {
                        shapes.push(Shape::rect_filled(r.expand(2.0 + 6.0 * pulse), 6.0, with_alpha(pal.accent, (60.0 * pulse) as u8)));
                        ui.ctx().request_repaint();
                    }
                    shapes.push(Shape::rect_filled(r.expand(2.5), 5.0, with_alpha(pal.accent, 28)));
                    shapes.push(Shape::rect_filled(r, 4.0, with_alpha(pal.accent, 95)));
                    shapes.push(Shape::rect_stroke(r, 4.0, Stroke::new(1.5, pal.accent), egui::StrokeKind::Outside));
                } else {
                    shapes.push(Shape::rect_filled(r, 4.0, with_alpha(hit, 70)));
                    shapes.push(Shape::line_segment([pos2(r.min.x + 2.0, r.max.y - 0.5), pos2(r.max.x - 2.0, r.max.y - 0.5)], Stroke::new(1.5, with_alpha(hit, 200))));
                }
            }
        }
        painter.set(search_idx, Shape::Vec(shapes));
        // overview ticks on the right edge of the visible area
        if !st.embedded && resp_rect.height() > 1.0 {
            let total = resp_rect.height();
            for (y, is_cur) in ticks {
                let f = ((y - resp_rect.min.y) / total).clamp(0.0, 1.0);
                let ty = clip.min.y + 4.0 + f * (clip.height() - 8.0);
                let tr = Rect::from_center_size(pos2(clip.max.x - 5.0, ty), vec2(if is_cur { 8.0 } else { 6.0 }, if is_cur { 4.0 } else { 3.0 }));
                painter.rect_filled(tr, 1.5, if is_cur { pal.accent } else { with_alpha(hit, 210) });
            }
        }
    }

    // ── vim visual-block selection ──
    if !buf.vim_block_sel.is_empty() {
        let mut shapes = vec![];
        for &(a, b) in &buf.vim_block_sel {
            let r0 = galley.pos_from_cursor(CCursor::new(a)).translate(gpos.to_vec2());
            let r1 = galley.pos_from_cursor(CCursor { index: b.into(), prefer_next_row: false }).translate(gpos.to_vec2());
            if r0.max.y < clip.min.y || r0.min.y > clip.max.y {
                continue;
            }
            let x1 = if (r1.min.y - r0.min.y).abs() < 1.0 { r1.min.x } else { r0.min.x + st.font_size * 0.6 };
            shapes.push(Shape::rect_filled(Rect::from_min_max(r0.min, pos2(x1.max(r0.min.x + 2.0), r0.max.y)), 0.0, pal.selection));
        }
        painter.set(vsel_idx, Shape::Vec(shapes));
    }

    // ── vim block cursor ──
    if let Some(block) = vim_block {
        let p = block.unwrap_or_else(|| output.cursor_range.map(|r| r.primary.index.0).unwrap_or(buf.cursor));
        let chars: Vec<char> = buf.text.chars().skip(p).take(1).collect();
        let ch = chars.first().copied().filter(|c| *c != '\n');
        let r0 = galley.pos_from_cursor(CCursor::new(p)).translate(gpos.to_vec2());
        let mut w = st.font_size * 0.6;
        if ch.is_some() {
            let r1 = galley.pos_from_cursor(CCursor { index: (p + 1).into(), prefer_next_row: false }).translate(gpos.to_vec2());
            if (r1.min.y - r0.min.y).abs() < 1.0 && r1.min.x > r0.min.x {
                w = r1.min.x - r0.min.x;
            }
        }
        let rect = Rect::from_min_size(r0.min, vec2(w.max(2.0), r0.height()));
        if rect.intersects(clip) {
            let focused = ui.ctx().memory(|m| m.has_focus(buf.id));
            if focused {
                painter.rect_filled(rect, 2.0, pal.accent);
                if let Some(c) = ch {
                    let f = if visual { FontId::new(vsize, FontFamily::Name("serif".into())) } else { FontId::new(st.font_size, FontFamily::Monospace) };
                    painter.text(pos2(rect.min.x, rect.center().y), egui::Align2::LEFT_CENTER, c.to_string(), f, pal.on_accent);
                }
            } else {
                painter.rect_stroke(rect, 2.0, Stroke::new(1.5, with_alpha(pal.accent, 200)), egui::StrokeKind::Inside);
            }
        }
    }

    if scroll_to_cursor {
        let r = galley.pos_from_cursor(CCursor::new(buf.cursor)).translate(gpos.to_vec2());
        ui.scroll_to_rect(r.expand2(vec2(0.0, 60.0)), Some(egui::Align::Center));
    }

    if output.response.clicked() && ui.input(|i| i.modifiers.command) {
        if let Some(r) = output.cursor_range {
            let ci = r.primary.index.0;
            out.ctrl_click_line = Some(buf.text[..char_to_byte(&buf.text, ci)].matches('\n').count() + 1);
        }
    }
    (output, galley, gpos)
}

/// Vim command line (":" / "/") drawn at the bottom of the editor.
fn vim_cmdline(ui: &egui::Ui, v: &crate::vim::VimState, pal: &Palette) {
    let Some(cl) = &v.cmdline else { return };
    let clip = ui.clip_rect();
    let r = Rect::from_min_max(pos2(clip.min.x + 12.0, clip.max.y - 40.0), pos2(clip.max.x - 12.0, clip.max.y - 10.0));
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("vim-cmdline")));
    p.rect_filled(r.translate(vec2(0.0, 3.0)), 8.0, with_alpha(Color32::BLACK, 60));
    p.rect_filled(r, 8.0, pal.surface);
    p.rect_stroke(r, 8.0, Stroke::new(1.0, pal.accent), egui::StrokeKind::Inside);
    let g = p.layout_no_wrap(cl.clone(), FontId::new(14.0, FontFamily::Monospace), pal.bright);
    let w = g.size().x;
    p.galley(pos2(r.min.x + 12.0, r.center().y - g.size().y / 2.0), g, pal.bright);
    let blink = (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0;
    if blink {
        p.rect_filled(Rect::from_min_size(pos2(r.min.x + 13.0 + w, r.center().y - 8.0), vec2(8.0, 16.0)), 1.0, with_alpha(pal.accent, 200));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

fn apply_completion(buf: &mut Buffer, comp: &Completion, item: &CompItem) {
    let (clean, mut caret) = match item.insert.find("$0") {
        Some(p) => (item.insert.replacen("$0", "", 1), byte_to_char(&item.insert, p)),
        None => (item.insert.clone(), item.insert.chars().count()),
    };
    // replace the typed prefix and the rest of the word under the cursor
    let chars: Vec<char> = buf.text.chars().collect();
    let mut end = buf.cursor.min(chars.len());
    while end < chars.len() && (chars[end].is_alphanumeric() || "_-:.".contains(chars[end])) {
        end += 1;
    }
    let mut ins = clean;
    if matches!(comp.kind, CompKind::Cite | CompKind::Ref | CompKind::File) && chars.get(end) != Some(&'}') && !ins.contains('}') {
        ins.push('}');
        caret = ins.chars().count();
    }
    // an auto-paired `}` already follows: don't insert a second one
    if matches!(comp.kind, CompKind::Env) && ins.contains('}') && chars.get(end) == Some(&'}') {
        end += 1;
    }
    buf.replace_chars(comp.start, end, &ins);
    buf.select(comp.start + caret, comp.start + caret);
}

fn comp_needs_scroll(ui: &egui::Ui, rect: Rect) -> bool {
    !ui.clip_rect().contains_rect(rect)
}

/// Case-insensitive matches of `q` in `text` as char ranges.
pub fn find_matches(text: &str, q: &str) -> Vec<(usize, usize)> {
    if q.is_empty() {
        return vec![];
    }
    let hay: Vec<char> = text.chars().flat_map(|c| c.to_lowercase().next()).collect();
    let needle: Vec<char> = q.chars().flat_map(|c| c.to_lowercase().next()).collect();
    let mut out = vec![];
    if needle.len() > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            out.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    fn comp(text: &str) -> Option<Completion> {
        let cites = vec![CompItem { label: "knuth1984".into(), detail: "Knuth 1984".into(), insert: "knuth1984".into() }];
        let labels = vec!["fig:esters".to_string(), "tab:recipe".to_string()];
        let files = vec![];
        let src = CompletionSources { cites: &cites, labels: &labels, files: &files };
        compute_completion(text, text.chars().count(), &src)
    }

    #[test]
    fn completes() {
        let c = comp("\\sec").unwrap();
        assert_eq!(c.items[0].label, "\\section");
        assert!(comp("\\s").unwrap().items.iter().all(|i| i.label.starts_with("\\s")));
        let c = comp("\\begin{item").unwrap();
        assert_eq!(c.items[0].label, "itemize");
        assert!(c.items[0].insert.contains("\\item $0") && c.items[0].insert.ends_with("\\end{itemize}"));
        let c = comp("\\usepackage{book").unwrap();
        assert_eq!(c.items[0].label, "booktabs");
        let c = comp("see \\cref{fig").unwrap();
        assert_eq!(c.items[0].label, "fig:esters");
        assert!(!c.items[0].detail.is_empty());
        // commands used in the document are offered too
        let c = comp("\\mymacro{x} and \\mym").unwrap();
        assert!(c.items.iter().any(|i| i.label == "\\mymacro"));
    }
}
