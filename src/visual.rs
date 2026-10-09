//! "Visual" rendering of LaTeX source: markup is laid out with (near) zero size so
//! the text reads like a typeset document, while the buffer stays plain LaTeX.
//! The line holding the cursor is revealed so it can be edited as source.

use crate::theme::{mix, with_alpha, Palette};
use egui::text::{ByteIndex, LayoutJob, LayoutSection, TextFormat};
use egui::{Color32, FontFamily, FontId, Stroke};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Vs {
    Body,
    Bold,
    Italic,
    BoldItalic,
    Mono,
    Head(u8),
    Math,
    Cite,
    Ref,
    Foot,
    Caption,
    Comment,
    Code,
    Env,
    Tilde,
    /// Gap after a hidden `\item` (room for the painted bullet), with list depth.
    ItemGap(u8),
    /// Markup that is hidden unless on the cursor line; carries the surrounding style.
    Markup(&'static Vs),
}

#[derive(Clone, Copy, Debug)]
pub enum DecorKind {
    Bullet,
    Number(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct Decor {
    pub byte: usize,
    pub depth: u8,
    pub kind: DecorKind,
    pub line_start: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub s: usize,
    pub e: usize,
    pub st: Vs,
}

// `Markup` needs a 'static reference to its context style.
const BODY: Vs = Vs::Body;
const BOLD: Vs = Vs::Bold;
const ITAL: Vs = Vs::Italic;
const BI: Vs = Vs::BoldItalic;
const MONO: Vs = Vs::Mono;
const H0: Vs = Vs::Head(0);
const H1: Vs = Vs::Head(1);
const H2: Vs = Vs::Head(2);
const H3: Vs = Vs::Head(3);
const H4: Vs = Vs::Head(4);
const H5: Vs = Vs::Head(5);
const FOOT: Vs = Vs::Foot;
const CAPT: Vs = Vs::Caption;
const CODE: Vs = Vs::Code;
const MATH: Vs = Vs::Math;

fn ctx_ref(v: Vs) -> &'static Vs {
    match v {
        Vs::Bold => &BOLD,
        Vs::Italic => &ITAL,
        Vs::BoldItalic => &BI,
        Vs::Mono => &MONO,
        Vs::Head(0) => &H0,
        Vs::Head(1) => &H1,
        Vs::Head(2) => &H2,
        Vs::Head(3) => &H3,
        Vs::Head(4) => &H4,
        Vs::Head(_) => &H5,
        Vs::Foot => &FOOT,
        Vs::Caption => &CAPT,
        Vs::Code | Vs::Env | Vs::Comment => &CODE,
        Vs::Math => &MATH,
        _ => &BODY,
    }
}

fn markup(v: Vs) -> Vs {
    Vs::Markup(ctx_ref(v))
}

const HIDE_ENVS: &[&str] = &["itemize", "enumerate", "description", "document", "center", "flushleft", "flushright", "raggedright", "abstract", "quote", "quotation", "minipage", "small", "footnotesize"];
const MATH_ENVS: &[&str] = &["equation", "equation*", "align", "align*", "gather", "gather*", "multline", "multline*", "flalign", "flalign*", "displaymath", "eqnarray", "eqnarray*"];

struct Out {
    spans: Vec<Span>,
    decor: Vec<Decor>,
}

impl Out {
    fn push(&mut self, s: usize, e: usize, st: Vs) {
        if e <= s {
            return;
        }
        if let Some(l) = self.spans.last_mut() {
            if l.st == st && l.e == s {
                l.e = e;
                return;
            }
        }
        self.spans.push(Span { s, e, st });
    }
}

fn matching_brace(b: &[u8], open: usize, limit: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < limit {
        match b[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn combine(base: Vs, add: Vs) -> Vs {
    match (base, add) {
        (Vs::Head(n), _) => Vs::Head(n),
        (Vs::Italic, Vs::Bold) | (Vs::Bold, Vs::Italic) => Vs::BoldItalic,
        (Vs::BoldItalic, _) => Vs::BoldItalic,
        (_, a) => a,
    }
}

fn skip_opt(b: &[u8], mut j: usize, e: usize) -> usize {
    while j < e && b[j] == b'[' {
        let mut k = j;
        while k < e && b[k] != b']' {
            k += 1;
        }
        j = (k + 1).min(e);
    }
    j
}

/// Inline LaTeX inside [s, e) with `base` style.
fn inline(text: &str, s: usize, e: usize, base: Vs, o: &mut Out) {
    let b = text.as_bytes();
    let mut i = s;
    let mut run = s;
    macro_rules! flush {
        () => {
            if i > run {
                o.push(run, i, base);
            }
        };
    }
    while i < e {
        let c = b[i];
        match c {
            b'%' => {
                // trailing comments disappear in the visual view
                flush!();
                o.push(i, e, markup(base));
                i = e;
                run = e;
            }
            b'\\' => {
                flush!();
                let mut j = i + 1;
                if j < e && b[j].is_ascii_alphabetic() {
                    while j < e && (b[j].is_ascii_alphabetic() || b[j] == b'@') {
                        j += 1;
                    }
                    let name = &text[i + 1..j];
                    let mut after = j;
                    if after < e && b[after] == b'*' {
                        after += 1;
                    }
                    let style_cmd = match name {
                        "textbf" | "textsuperscript" | "strong" => Some(Vs::Bold),
                        "textit" | "emph" | "textsl" | "game" => Some(Vs::Italic),
                        "texttt" | "url" | "path" | "verb" => Some(Vs::Mono),
                        "footnote" => Some(Vs::Foot),
                        "enquote" | "mbox" | "textrm" | "textsf" | "textsc" | "hl" | "underline" | "ul" | "textnormal" | "uline" => Some(base),
                        "caption" => Some(Vs::Caption),
                        _ => None,
                    };
                    let is_cite = name.contains("cite");
                    let is_ref = matches!(name, "ref" | "cref" | "Cref" | "eqref" | "autoref" | "pageref" | "vref" | "nameref" | "Autoref");
                    let open = skip_opt(b, after, e);
                    if (style_cmd.is_some() || is_cite || is_ref || name == "label") && open < e && b[open] == b'{' {
                        let close = matching_brace(b, open, e).unwrap_or(e);
                        o.push(i, open + 1, markup(base));
                        if name == "label" {
                            o.push(open + 1, close, markup(base));
                        } else if is_cite {
                            o.push(open + 1, close, Vs::Cite);
                        } else if is_ref {
                            o.push(open + 1, close, Vs::Ref);
                        } else {
                            let st = combine(base, style_cmd.unwrap());
                            inline(text, open + 1, close, st, o);
                        }
                        if close < e {
                            o.push(close, close + 1, markup(base));
                            i = close + 1;
                        } else {
                            i = e;
                        }
                    } else if matches!(name, "LaTeX" | "TeX" | "LaTeXe") {
                        o.push(i, i + 1, markup(base));
                        o.push(i + 1, after, base);
                        i = after;
                        if i < e && b[i] == b'{' && i + 1 < e && b[i + 1] == b'}' {
                            o.push(i, i + 2, markup(base));
                            i += 2;
                        }
                    } else if matches!(name, "noindent" | "centering" | "par" | "newline" | "linebreak" | "xspace" | "medskip" | "bigskip" | "smallskip" | "hfill" | "vfill" | "protect") {
                        o.push(i, after, markup(base));
                        i = after;
                    } else {
                        // unknown command: show it (with its arguments) subtly as code
                        let mut j2 = after;
                        loop {
                            let k = skip_opt(b, j2, e);
                            if k < e && b[k] == b'{' {
                                match matching_brace(b, k, e) {
                                    Some(c) => j2 = c + 1,
                                    None => break,
                                }
                            } else {
                                j2 = k.max(j2);
                                break;
                            }
                        }
                        o.push(i, j2.max(after), Vs::Code);
                        i = j2.max(after);
                    }
                } else if j < e {
                    let ch = b[j];
                    let clen = text[j..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                    match ch {
                        b'(' | b'[' => {
                            let close_pat = if ch == b'(' { "\\)" } else { "\\]" };
                            let end = text[j..e].find(close_pat).map(|p| j + p).unwrap_or(e);
                            o.push(i, j + 1, markup(base));
                            o.push(j + 1, end, Vs::Math);
                            if end < e {
                                o.push(end, end + 2, markup(base));
                                i = end + 2;
                            } else {
                                i = e;
                            }
                        }
                        b'%' | b'&' | b'$' | b'#' | b'_' | b'{' | b'}' => {
                            o.push(i, j, markup(base));
                            o.push(j, j + 1, base);
                            i = j + 1;
                        }
                        _ => {
                            // \\, \, \; \- \  etc.
                            o.push(i, j + clen, markup(base));
                            i = j + clen;
                        }
                    }
                } else {
                    o.push(i, e, markup(base));
                    i = e;
                }
                run = i;
            }
            b'$' => {
                flush!();
                let dd = i + 1 < e && b[i + 1] == b'$';
                let w = if dd { 2 } else { 1 };
                let pat = if dd { "$$" } else { "$" };
                let end = text[i + w..e].find(pat).map(|p| i + w + p).unwrap_or(e);
                o.push(i, i + w, markup(base));
                o.push(i + w, end, Vs::Math);
                if end < e {
                    o.push(end, end + w, markup(base));
                    i = end + w;
                } else {
                    i = e;
                }
                run = i;
            }
            b'{' | b'}' => {
                flush!();
                o.push(i, i + 1, markup(base));
                i += 1;
                run = i;
            }
            b'~' => {
                flush!();
                o.push(i, i + 1, Vs::Tilde);
                i += 1;
                run = i;
            }
            _ => i += 1,
        }
    }
    flush!();
}

pub fn spans(text: &str) -> (Vec<Span>, Vec<Decor>) {
    let mut o = Out { spans: Vec::with_capacity(text.len() / 12), decor: vec![] };
    let b = text.as_bytes();
    let doc_begin = text.find("\\begin{document}");
    let head_re = regex::Regex::new(r"^\\(part|chapter|section|subsection|subsubsection|paragraph|addchap|addsec|minisec)\*?(\[[^\]]*\])?\{").unwrap();
    let env_re = regex::Regex::new(r"^\\(begin|end)\{([^}]+)\}").unwrap();
    let cmd_line_re = regex::Regex::new(r"^\\[a-zA-Z@]+\*?(\[[^\]]*\])*(\{[^{}]*\})*\s*(%.*)?$").unwrap();
    let mut envs: Vec<String> = vec![];
    let mut lists: Vec<(bool, usize)> = vec![]; // (enumerate?, counter)
    let mut pos = 0usize;
    for line in text.split_inclusive('\n') {
        let ls = pos;
        let le = pos + line.len();
        pos = le;
        let ce = if line.ends_with('\n') { le - 1 } else { le };
        let l = &text[ls..ce];
        let t = l.trim_start();
        let ts = ls + (l.len() - t.len());
        let t_trim = t.trim_end();

        // preamble
        if let Some(db) = doc_begin {
            if ls < db {
                o.push(ls, le, Vs::Code);
                continue;
            }
        }
        // leading indentation is hidden
        o.push(ls, ts, markup(Vs::Body));

        if t.is_empty() {
            o.push(ce, le, Vs::Body);
            continue;
        }
        if t.starts_with('%') {
            // comment lines are hidden in the visual view (revealed on the cursor line)
            o.push(ts, le, markup(Vs::Comment));
            continue;
        }
        if let Some(m) = head_re.captures(t) {
            let lvl: u8 = match &m[1] {
                "part" => 0,
                "chapter" | "addchap" => 1,
                "section" | "addsec" => 2,
                "subsection" => 3,
                "subsubsection" | "minisec" => 4,
                _ => 5,
            };
            let st = Vs::Head(lvl);
            let open = ts + m.get(0).unwrap().end() - 1;
            let close = matching_brace(b, open, ce).unwrap_or(ce);
            o.push(ts, open + 1, markup(st));
            inline(text, open + 1, close, st, &mut o);
            if close < ce {
                o.push(close, close + 1, markup(st));
                inline(text, close + 1, ce, st, &mut o);
            }
            o.push(ce, le, st);
            continue;
        }
        if let Some(m) = env_re.captures(t) {
            let begin = &m[1] == "begin";
            let env = m[2].to_string();
            let hide = HIDE_ENVS.contains(&env.as_str());
            if begin {
                envs.push(env.clone());
                if env == "itemize" || env == "description" {
                    lists.push((false, 0));
                } else if env == "enumerate" {
                    lists.push((true, 0));
                }
            } else {
                if let Some(p) = envs.iter().rposition(|x| *x == env) {
                    envs.truncate(p);
                }
                if env == "itemize" || env == "enumerate" || env == "description" {
                    lists.pop();
                }
            }
            let mend = ts + m.get(0).unwrap().end();
            let rest = text[mend..ce].trim();
            if hide && (rest.is_empty() || rest.starts_with('%') || rest.starts_with('[')) {
                o.push(ts, le, markup(Vs::Body));
            } else if hide {
                o.push(ts, mend, markup(Vs::Body));
                inline(text, mend, ce, Vs::Body, &mut o);
                o.push(ce, le, Vs::Body);
            } else {
                o.push(ts, le, Vs::Env);
            }
            continue;
        }
        let cur_env = envs.last().map(String::as_str).unwrap_or("");
        if MATH_ENVS.contains(&cur_env) {
            o.push(ts, le, Vs::Math);
            continue;
        }
        if matches!(cur_env, "tikzpicture" | "axis" | "tabular" | "tabularx" | "lstlisting" | "verbatim" | "minted" | "longtable") {
            o.push(ts, le, Vs::Code);
            continue;
        }
        if t.starts_with("\\item") && !t[5..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            let depth = lists.len().max(1) as u8;
            let mut j = ts + 5;
            // optional [label]
            j = skip_opt(b, j, ce);
            let gap_end = if j < ce && b[j] == b' ' { j + 1 } else { j };
            let kind = match lists.last_mut() {
                Some((true, n)) => {
                    *n += 1;
                    DecorKind::Number(*n)
                }
                _ => DecorKind::Bullet,
            };
            o.decor.push(Decor { byte: ts, depth, kind, line_start: ls });
            if gap_end > j {
                o.push(ts, j, markup(Vs::Body));
                o.push(j, gap_end, Vs::ItemGap(depth));
            } else {
                o.push(ts, j, Vs::ItemGap(depth));
            }
            inline(text, gap_end, ce, Vs::Body, &mut o);
            o.push(ce, le, Vs::Body);
            continue;
        }
        if t_trim.starts_with("\\label{") && t_trim.ends_with('}') && !t_trim[1..].contains('\\') {
            o.push(ts, le, markup(Vs::Body));
            continue;
        }
        if matches!(cur_env, "figure" | "figure*" | "table" | "table*") && !t.starts_with("\\caption") {
            o.push(ts, le, Vs::Code);
            continue;
        }
        if t.starts_with('\\') && cmd_line_re.is_match(t) && !t.starts_with("\\caption") && !t.starts_with("\\textbf") && !t.starts_with("\\emph") && !t.starts_with("\\textit") {
            o.push(ts, le, Vs::Code);
            continue;
        }
        inline(text, ts, ce, Vs::Body, &mut o);
        o.push(ce, le, Vs::Body);
    }
    (o.spans, o.decor)
}

pub struct VisualTheme {
    pub size: f32,
    pub text: Color32,
    pub heading: Color32,
    pub chapter: Color32,
    pub dim: Color32,
    pub math: Color32,
    pub cite: Color32,
    pub refc: Color32,
    pub env: Color32,
    pub chip_bg: Color32,
}

impl VisualTheme {
    pub fn new(p: &Palette, size: f32) -> Self {
        VisualTheme {
            size,
            text: p.text,
            heading: p.bright,
            chapter: mix(p.accent, p.bright, 0.25),
            dim: mix(p.dim, p.base, 0.05),
            math: p.green,
            cite: mix(p.accent, p.bright, 0.25),
            refc: p.cyan,
            env: mix(p.magenta, p.dim, 0.4),
            chip_bg: with_alpha(p.text, 16),
        }
    }
}

fn fam(n: &str) -> FontFamily {
    FontFamily::Name(n.into())
}

fn format_for(st: Vs, th: &VisualTheme, revealed: bool) -> TextFormat {
    let sz = th.size;
    let lh = |s: f32, f: f32| Some(s * f);
    let mut f = TextFormat { font_id: FontId::new(sz, fam("serif")), color: th.text, line_height: lh(sz, 1.62), ..Default::default() };
    match st {
        Vs::Body | Vs::Tilde => {
            if st == Vs::Tilde {
                f.color = Color32::TRANSPARENT;
            }
        }
        Vs::Bold => f.font_id.family = fam("serif-bold"),
        Vs::Italic => f.font_id.family = fam("serif-italic"),
        Vs::BoldItalic => f.font_id.family = fam("serif-bolditalic"),
        Vs::Mono => {
            f.font_id = FontId::new(sz * 0.86, FontFamily::Monospace);
        }
        Vs::Head(n) => {
            let (s, family) = match n {
                0 => (sz * 2.0, "serif-bold"),
                1 => (sz * 1.75, "serif-bold"),
                2 => (sz * 1.38, "serif-bold"),
                3 => (sz * 1.18, "serif-bold"),
                4 => (sz * 1.05, "serif-bold"),
                _ => (sz, "serif-bolditalic"),
            };
            f.font_id = FontId::new(s, fam(family));
            f.color = if n <= 1 { th.chapter } else { th.heading };
            f.line_height = lh(s, if n <= 2 { 2.1 } else { 1.8 });
        }
        Vs::Math => {
            f.font_id = FontId::new(sz * 0.98, fam("serif-italic"));
            f.color = th.math;
        }
        Vs::Cite | Vs::Ref => {
            f.font_id = FontId::new(sz * 0.74, FontFamily::Monospace);
            f.color = if st == Vs::Cite { th.cite } else { th.refc };
            // chip background is painted by the editor (rounded, compact)
        }
        Vs::Foot => {
            f.font_id = FontId::new(sz * 0.8, fam("serif"));
            f.color = th.dim;
            f.background = th.chip_bg;
        }
        Vs::Caption => {
            f.font_id = FontId::new(sz * 0.9, fam("serif-italic"));
            f.color = mix(th.text, th.dim, 0.3);
        }
        Vs::Comment => {
            f.font_id = FontId::new(sz * 0.72, FontFamily::Monospace);
            f.color = with_alpha(th.dim, 170);
            f.italics = true;
            f.line_height = lh(sz * 0.72, 1.6);
        }
        Vs::Code => {
            f.font_id = FontId::new(sz * 0.74, FontFamily::Monospace);
            f.color = th.dim;
            f.line_height = lh(sz * 0.74, 1.55);
        }
        Vs::Env => {
            f.font_id = FontId::new(sz * 0.7, FontFamily::Monospace);
            f.color = th.env;
            f.line_height = lh(sz * 0.7, 1.9);
        }
        Vs::ItemGap(d) => {
            f.extra_letter_spacing = 18.0 + 22.0 * (d.saturating_sub(1)) as f32;
            if revealed {
                f.font_id = FontId::new(sz * 0.8, FontFamily::Monospace);
                f.color = th.dim;
                f.extra_letter_spacing = 0.0;
            }
        }
        Vs::Markup(ctx) => {
            if revealed {
                let base = format_for(*ctx, th, false);
                f.font_id = FontId::new((base.font_id.size * 0.62).max(th.size * 0.72), FontFamily::Monospace);
                f.color = th.dim;
                f.line_height = base.line_height;
            } else {
                f.font_id = FontId::new(0.6, FontFamily::Monospace);
                f.color = Color32::TRANSPARENT;
                f.line_height = Some(0.5);
            }
        }
    }
    f.underline = Stroke::NONE;
    f
}

/// Build the layout job. `raw` = byte range shown as source (cursor line).
pub fn layout_job(text: &str, spans: &[Span], th: &VisualTheme, raw: Option<(usize, usize)>) -> LayoutJob {
    let mut job = LayoutJob { text: text.to_string(), ..Default::default() };
    for sp in spans {
        let in_raw = raw.is_some_and(|(a, b)| sp.s < b.max(a + 1) && sp.e > a);
        let revealed = in_raw && matches!(sp.st, Vs::Markup(_) | Vs::ItemGap(_));
        let mut fmt = format_for(sp.st, th, revealed);
        if in_raw && sp.st == Vs::Tilde {
            fmt.color = th.dim;
        }
        job.sections.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(sp.s)..ByteIndex(sp.e), format: fmt });
    }
    if job.sections.is_empty() {
        job.sections.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(0), format: format_for(Vs::Body, th, false) });
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible(text: &str) -> String {
        let (sp, _) = spans(text);
        let mut out = String::new();
        for s in sp {
            if !matches!(s.st, Vs::Markup(_)) {
                out.push_str(&text[s.s..s.e]);
            }
        }
        out
    }

    #[test]
    fn hides_markup() {
        assert_eq!(visible("Ein \\textbf{fettes} Wort.\n"), "Ein fettes Wort.\n");
        assert_eq!(visible("\\section{Motivation}\n"), "Motivation\n");
        assert_eq!(visible("Siehe \\citep{knuth} und \\cref{fig:a}.\n"), "Siehe knuth und fig:a.\n");
        assert_eq!(visible("\\begin{itemize}\n  \\item Eins\n\\end{itemize}\n"), " Eins\n");
        assert_eq!(visible("100\\,\\% sicher\n"), "100% sicher\n");
    }

    #[test]
    fn spans_cover_text() {
        let t = "\\documentclass{x}\n\\begin{document}\n\\chapter{A}\nText $x^2$ \\emph{b}\n% c\n\\end{document}\n";
        let (sp, _) = spans(t);
        let mut pos = 0;
        for s in &sp {
            assert_eq!(s.s, pos, "gap/overlap at {pos}: {sp:?}");
            pos = s.e;
        }
        assert_eq!(pos, t.len());
    }
}
