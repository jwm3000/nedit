//! Slide model for the visual presentation editor.
//!
//! A Beamer source is split into frames; each frame body is parsed into simple elements
//! (text, lists, blocks, images, columns, formulas, pauses). Anything that isn't understood
//! is kept verbatim as `Raw`, so editing a slide never loses LaTeX. When a slide is edited,
//! only its own `\begin{frame} … \end{frame}` range is rewritten with clean, indented LaTeX.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub level: u8,
    /// Overlay spec such as `<2->` (kept as written).
    pub overlay: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Block,
    Alert,
    Example,
}

impl BlockKind {
    pub fn env(self) -> &'static str {
        match self {
            BlockKind::Block => "block",
            BlockKind::Alert => "alertblock",
            BlockKind::Example => "exampleblock",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Elem {
    Text(String),
    List {
        /// environment name as written (itemize, enumerate, tugitemize …)
        env: String,
        numbered: bool,
        /// reveal items one after another (`[<+->]`)
        step: bool,
        items: Vec<Item>,
    },
    Block {
        kind: BlockKind,
        title: String,
        body: Vec<Elem>,
    },
    Image {
        path: String,
        /// fraction of \textheight (0.1 … 1.0)
        height: f32,
    },
    Columns {
        env: String,
        cols: Vec<Vec<Elem>>,
    },
    Math(String),
    Pause,
    TitlePage,
    Toc,
    Raw(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Slide {
    /// Frame options without brackets, e.g. `c` or `plain`.
    pub opts: String,
    pub title: String,
    pub body: Vec<Elem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrameRange {
    /// Byte range of `\begin{frame} … \end{frame}`.
    pub start: usize,
    pub end: usize,
    /// Section the frame belongs to (last `\section` before it).
    pub section: String,
    pub slide: Slide,
}

/// Title-page metadata in the preamble (`\title`, `\author`, …) with the byte range of the
/// mandatory argument, so it can be edited in place.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub cmd: &'static str,
    pub range: (usize, usize),
    pub value: String,
}

#[derive(Clone, Debug, Default)]
pub struct Deck {
    pub frames: Vec<FrameRange>,
    pub meta: Vec<Meta>,
    /// 16:9 (`aspectratio=169`) or 4:3
    pub wide: bool,
    /// The deck uses the TU Graz theme's list environments.
    pub tug: bool,
    /// Where `\end{document}` starts (new slides go before it).
    pub doc_end: usize,
}

// ───────────────────────────── scanning helpers ─────────────────────────────

/// Index after the group starting at `open` (`{` or `[`), honouring nesting and escapes.
fn group_end(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    let (o, c) = match b.get(open)? {
        b'{' => (b'{', b'}'),
        b'[' => (b'[', b']'),
        _ => return None,
    };
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'%' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            x if x == o => depth += 1,
            x if x == c => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Byte index just after the `\end{env}` matching a `\begin{env}` that ends at `from`.
fn env_end(s: &str, from: usize, env: &str) -> Option<(usize, usize)> {
    let begin = format!("\\begin{{{env}}}");
    let end = format!("\\end{{{env}}}");
    let mut depth = 1;
    let mut i = from;
    while i < s.len() {
        let r = &s[i..];
        if r.starts_with('%') && (i == 0 || s.as_bytes()[i - 1] != b'\\') {
            i += r.find('\n').unwrap_or(r.len());
            continue;
        }
        if r.starts_with(&begin) {
            depth += 1;
            i += begin.len();
        } else if r.starts_with(&end) {
            depth -= 1;
            if depth == 0 {
                return Some((i, i + end.len()));
            }
            i += end.len();
        } else {
            i += r.chars().next().map_or(1, |c| c.len_utf8());
        }
    }
    None
}

fn skip_ws(s: &str, mut i: usize) -> usize {
    let b = s.as_bytes();
    while i < b.len() && (b[i] as char).is_whitespace() {
        i += 1;
    }
    i
}

/// Skip spaces/tabs only (stay on the line).
fn skip_sp(s: &str, mut i: usize) -> usize {
    let b = s.as_bytes();
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    i
}

fn dedent(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let ind = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    lines.iter().map(|l| if l.len() >= ind { &l[ind..] } else { l.trim_start() }).collect::<Vec<_>>().join("\n").trim_matches('\n').to_string()
}

/// Join wrapped source lines of a paragraph into one line (`%` comments stay raw elsewhere).
fn unwrap_par(s: &str) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}

static RE_BEGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\\begin\{([A-Za-z*]+)\}").unwrap());
static RE_GFX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\\includegraphics(?:\[([^\]]*)\])?\{([^{}]*)\}\s*$").unwrap());
/// Lines that only carry layout – kept verbatim as raw LaTeX.
static RE_LAYOUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\\(vspace\*?\{[^{}]*\}|vfill|medskip|bigskip|smallskip|hfill|newpage|framebreak|centering|raggedright|raggedleft)\s*$").unwrap());

fn is_list_env(e: &str) -> Option<bool> {
    match e {
        "itemize" | "tugitemize" | "compactitem" => Some(false),
        "enumerate" | "tugenumerate" | "compactenum" => Some(true),
        _ => None,
    }
}

// ───────────────────────────── parser ─────────────────────────────

/// Parse a list body into items (nested lists become deeper levels).
fn parse_items(body: &str, level: u8, out: &mut Vec<Item>) -> bool {
    let mut i = 0;
    let b = body.as_bytes();
    let mut cur: Option<Item> = None;
    let mut text = String::new();
    let flush = |cur: &mut Option<Item>, text: &mut String, out: &mut Vec<Item>| {
        if let Some(mut it) = cur.take() {
            it.text = unwrap_par(text);
            out.push(it);
        }
        text.clear();
    };
    while i < body.len() {
        let r = &body[i..];
        if r.starts_with("\\item") && !r[5..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            flush(&mut cur, &mut text, out);
            let mut j = i + 5;
            let mut overlay = String::new();
            if body[j..].starts_with('<') {
                let e = body[j..].find('>').map(|p| j + p + 1).unwrap_or(j);
                overlay = body[j..e].to_string();
                j = e;
            }
            if body[j..].starts_with('[') {
                return false; // custom labels: not supported visually
            }
            cur = Some(Item { level, overlay, text: String::new() });
            i = skip_sp(body, j);
            continue;
        }
        if let Some(m) = RE_BEGIN.captures(r) {
            let env = m[1].to_string();
            if is_list_env(&env).is_some() && cur.is_some() {
                let start = i + m[0].len();
                let Some((e0, e1)) = env_end(body, start, &env) else { return false };
                flush(&mut cur, &mut text, out);
                if !parse_items(&body[start..e0], level + 1, out) {
                    return false;
                }
                i = e1;
                continue;
            }
            return false;
        }
        if r.starts_with('%') && (i == 0 || b[i - 1] != b'\\') {
            return false;
        }
        let ch = r.chars().next().unwrap();
        if cur.is_none() {
            if !ch.is_whitespace() {
                return false; // text before the first \item
            }
        } else {
            text.push(ch);
        }
        i += ch.len_utf8();
    }
    flush(&mut cur, &mut text, out);
    true
}

/// Parse a frame (or column/block) body into elements.
pub fn parse_body(s: &str) -> Vec<Elem> {
    let mut out = vec![];
    let mut i = 0;
    let mut par = String::new();
    let push_par = |par: &mut String, out: &mut Vec<Elem>| {
        let t = unwrap_par(par);
        if !t.is_empty() {
            out.push(Elem::Text(t));
        }
        par.clear();
    };
    while i < s.len() {
        let line_start = s[..i].rfind('\n').map_or(0, |p| p + 1);
        let at_line_start = s[line_start..i].trim().is_empty();
        let r = &s[i..];
        if at_line_start {
            let t = r.trim_start_matches([' ', '\t']);
            let ti = i + (r.len() - t.len());
            // blank line ends a paragraph
            if t.starts_with('\n') {
                push_par(&mut par, &mut out);
                i = ti + 1;
                continue;
            }
            let line_end = s[ti..].find('\n').map_or(s.len(), |p| ti + p);
            let line = s[ti..line_end].trim_end();
            let simple = |e: Elem, par: &mut String, out: &mut Vec<Elem>| {
                push_par(par, out);
                out.push(e);
            };
            match line {
                "\\pause" => {
                    simple(Elem::Pause, &mut par, &mut out);
                    i = line_end;
                    continue;
                }
                "\\maketitle" | "\\titlepage" => {
                    simple(Elem::TitlePage, &mut par, &mut out);
                    i = line_end;
                    continue;
                }
                "\\tableofcontents" => {
                    simple(Elem::Toc, &mut par, &mut out);
                    i = line_end;
                    continue;
                }
                "\\centering" if par.trim().is_empty() => {
                    // a lone \centering before an image belongs to the image
                    let next = skip_ws(s, line_end);
                    let nl = s[next..].find('\n').map_or(s.len(), |p| next + p);
                    if let Some(img) = image_from(&s[next..nl]) {
                        simple(img, &mut par, &mut out);
                        i = nl;
                        continue;
                    }
                }
                _ => {}
            }
            if let Some(img) = image_from(line) {
                simple(img, &mut par, &mut out);
                i = line_end;
                continue;
            }
            if t.starts_with('%') || RE_LAYOUT.is_match(line) {
                push_par(&mut par, &mut out);
                out.push(Elem::Raw(line.to_string()));
                i = line_end;
                continue;
            }
            if t.starts_with("\\[") {
                if let Some(p) = t.find("\\]") {
                    push_par(&mut par, &mut out);
                    out.push(Elem::Math(dedent(&t[2..p]).trim().to_string()));
                    i = ti + p + 2;
                    continue;
                }
            }
            if let Some(m) = RE_BEGIN.captures(t) {
                push_par(&mut par, &mut out);
                let env = m[1].to_string();
                let head_end = ti + m[0].len();
                let Some((e0, e1)) = env_end(s, head_end, &env) else {
                    out.push(Elem::Raw(dedent(&s[ti..])));
                    return out;
                };
                let whole = &s[ti..e1];
                out.push(parse_env(&env, s, head_end, e0).unwrap_or_else(|| Elem::Raw(dedent(whole))));
                i = e1;
                continue;
            }
        }
        let ch = r.chars().next().unwrap();
        par.push(ch);
        i += ch.len_utf8();
    }
    push_par(&mut par, &mut out);
    // raw lines that only carry layout (\vspace …) stay raw – merge neighbouring raw lines
    let mut merged: Vec<Elem> = vec![];
    for e in out {
        if let (Some(Elem::Raw(prev)), Elem::Raw(cur)) = (merged.last_mut(), &e) {
            prev.push('\n');
            prev.push_str(cur);
            continue;
        }
        merged.push(e);
    }
    merged
}

fn image_from(line: &str) -> Option<Elem> {
    let m = RE_GFX.captures(line.trim())?;
    let opts = m.get(1).map_or("", |x| x.as_str());
    let height = Regex::new(r"height=([0-9.]+)\\textheight").unwrap().captures(opts).and_then(|h| h[1].parse().ok());
    let height = match (height, opts.is_empty()) {
        (Some(h), _) => h,
        (None, true) => 0.6,
        (None, false) => return None, // other options (width=…, angle …): keep raw
    };
    Some(Elem::Image { path: m[2].to_string(), height })
}

fn parse_env(env: &str, s: &str, head_end: usize, body_end: usize) -> Option<Elem> {
    if let Some(numbered) = is_list_env(env) {
        let mut j = head_end;
        let mut step = false;
        if s[j..].starts_with('[') {
            let e = group_end(s, j)?;
            if s[j..e].replace(' ', "") != "[<+->]" {
                return None;
            }
            step = true;
            j = e;
        }
        let mut items = vec![];
        if !parse_items(&s[j..body_end], 0, &mut items) {
            return None;
        }
        return Some(Elem::List { env: env.to_string(), numbered, step, items });
    }
    match env {
        "block" | "alertblock" | "exampleblock" => {
            let j = skip_sp(s, head_end);
            if !s[j..].starts_with('{') {
                return None;
            }
            let e = group_end(s, j)?;
            let title = s[j + 1..e - 1].to_string();
            let kind = match env {
                "alertblock" => BlockKind::Alert,
                "exampleblock" => BlockKind::Example,
                _ => BlockKind::Block,
            };
            let body = parse_body(&dedent(&s[e..body_end]));
            Some(Elem::Block { kind, title, body })
        }
        "columns" | "tugcolumns" => {
            if s[head_end..].starts_with('[') {
                return None;
            }
            let inner = &s[head_end..body_end];
            let mut cols = vec![];
            let mut k = 0;
            loop {
                let rest = &inner[k..];
                let Some(p) = rest.find("\\begin{column}") else {
                    if !rest.trim().is_empty() {
                        return None;
                    }
                    break;
                };
                if !rest[..p].trim().is_empty() {
                    return None;
                }
                let a = k + p + "\\begin{column}".len();
                let a2 = skip_sp(inner, a);
                let we = group_end(inner, a2)?; // width argument
                let (b0, b1) = env_end(inner, we, "column")?;
                cols.push(parse_body(&dedent(&inner[we..b0])));
                k = b1;
            }
            if cols.is_empty() {
                return None;
            }
            Some(Elem::Columns { env: env.to_string(), cols })
        }
        "center" => {
            let body = s[head_end..body_end].trim();
            image_from(body)
        }
        "equation*" | "displaymath" => Some(Elem::Math(dedent(&s[head_end..body_end]).trim().to_string())),
        "multicols" => {
            // \begin{multicols}{2}\tableofcontents\end{multicols} (TU Graz outline slide)
            let inner = s[head_end..body_end].trim();
            let inner = inner.strip_prefix("{2}").unwrap_or(inner).trim();
            (inner == "\\tableofcontents").then_some(Elem::Toc)
        }
        _ => None,
    }
}

static RE_FRAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\begin\{frame\}").unwrap());
static RE_SECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\section\*?(?:\[[^\]]*\])?\{([^{}]*)\}").unwrap());

/// Parse the whole presentation source.
pub fn parse_deck(src: &str) -> Deck {
    let mut deck = Deck { doc_end: src.rfind("\\end{document}").unwrap_or(src.len()), ..Default::default() };
    deck.wide = Regex::new(r"(?m)^[^%\n]*\\documentclass\[[^\]]*aspectratio=169").unwrap().is_match(src);
    deck.tug = src.contains("tugraz2018") || src.contains("tugitemize");
    // title metadata in the preamble
    let pre_end = src.find("\\begin{document}").unwrap_or(0);
    for cmd in ["title", "subtitle", "author", "institute", "date"] {
        let re = Regex::new(&format!(r"(?m)^[ \t]*\\{cmd}(?:\[[^\]]*\])?\{{")).unwrap();
        if let Some(m) = re.find(&src[..pre_end]) {
            let open = m.end() - 1;
            if let Some(e) = group_end(src, open) {
                deck.meta.push(Meta { cmd, range: (open + 1, e - 1), value: src[open + 1..e - 1].to_string() });
            }
        }
    }
    let mut section = String::new();
    let mut last = pre_end;
    for m in RE_FRAME.find_iter(src) {
        let start = m.start();
        if start < pre_end || start < last {
            continue;
        }
        // skip commented-out frames
        let ls = src[..start].rfind('\n').map_or(0, |p| p + 1);
        if src[ls..start].contains('%') {
            continue;
        }
        for sm in RE_SECTION.captures_iter(&src[last..start]) {
            section = sm[1].to_string();
        }
        let Some((e0, e1)) = env_end(src, m.end(), "frame") else { break };
        let mut j = m.end();
        let mut opts = String::new();
        if src[j..].starts_with('[') {
            if let Some(e) = group_end(src, j) {
                opts = src[j + 1..e - 1].to_string();
                j = e;
            }
        }
        let mut title = String::new();
        if src[j..].starts_with('{') {
            if let Some(e) = group_end(src, j) {
                title = src[j + 1..e - 1].to_string();
                j = e;
            }
        }
        let mut body = &src[j..e0];
        // \frametitle{…} as first line
        let bt = body.trim_start();
        if title.is_empty() && bt.starts_with("\\frametitle{") {
            let off = body.len() - bt.len() + "\\frametitle".len();
            if let Some(e) = group_end(body, off) {
                title = body[off + 1..e - 1].to_string();
                body = &body[e..];
            }
        }
        deck.frames.push(FrameRange { start, end: e1, section: section.clone(), slide: Slide { opts, title, body: parse_body(&dedent(body)) } });
        last = e1;
    }
    deck
}

// ───────────────────────────── serializer ─────────────────────────────

fn write_elems(out: &mut String, elems: &[Elem], ind: &str) {
    let mut first = true;
    for e in elems {
        // paragraphs and big elements are separated by a blank line, pauses hug their neighbours
        if !first && !matches!(e, Elem::Pause) {
            out.push('\n');
        }
        first = matches!(e, Elem::Pause);
        match e {
            Elem::Text(t) => {
                for l in t.trim().lines() {
                    out.push_str(&format!("{ind}{}\n", l.trim()));
                }
            }
            Elem::Pause => out.push_str(&format!("{ind}\\pause\n")),
            Elem::TitlePage => out.push_str(&format!("{ind}\\maketitle\n")),
            Elem::Toc => out.push_str(&format!("{ind}\\tableofcontents\n")),
            Elem::Math(m) => {
                out.push_str(&format!("{ind}\\[\n"));
                for l in m.trim().lines() {
                    out.push_str(&format!("{ind}  {}\n", l.trim()));
                }
                out.push_str(&format!("{ind}\\]\n"));
            }
            Elem::Image { path, height } => {
                out.push_str(&format!("{ind}\\centering\n{ind}\\includegraphics[height={}\\textheight]{{{path}}}\n", fmt_frac(*height)));
            }
            Elem::List { env, step, items, .. } => {
                let opt = if *step { "[<+->]" } else { "" };
                out.push_str(&format!("{ind}\\begin{{{env}}}{opt}\n"));
                let mut level = 0u8;
                let mut stack = vec![];
                for it in items {
                    while it.level > level {
                        let pad = format!("{ind}{}", "  ".repeat(level as usize * 2 + 2));
                        out.push_str(&format!("{pad}\\begin{{{env}}}\n"));
                        stack.push(pad);
                        level += 1;
                    }
                    while it.level < level {
                        let pad = stack.pop().unwrap();
                        out.push_str(&format!("{pad}\\end{{{env}}}\n"));
                        level -= 1;
                    }
                    let pad = format!("{ind}{}", "  ".repeat(level as usize * 2 + 1));
                    let text = it.text.trim();
                    out.push_str(&format!("{pad}\\item{}{}{}\n", it.overlay, if text.is_empty() { "" } else { " " }, text));
                }
                while let Some(pad) = stack.pop() {
                    out.push_str(&format!("{pad}\\end{{{env}}}\n"));
                }
                out.push_str(&format!("{ind}\\end{{{env}}}\n"));
            }
            Elem::Block { kind, title, body } => {
                out.push_str(&format!("{ind}\\begin{{{}}}{{{title}}}\n", kind.env()));
                write_elems(out, body, &format!("{ind}  "));
                out.push_str(&format!("{ind}\\end{{{}}}\n", kind.env()));
            }
            Elem::Columns { env, cols } => {
                let w = match cols.len() {
                    0 | 1 => "\\linewidth".to_string(),
                    n => format!("{}\\linewidth", fmt_frac(0.96 / n as f32 - 0.0)),
                };
                out.push_str(&format!("{ind}\\begin{{{env}}}\n"));
                for c in cols {
                    out.push_str(&format!("{ind}  \\begin{{column}}{{{w}}}\n"));
                    write_elems(out, c, &format!("{ind}    "));
                    out.push_str(&format!("{ind}  \\end{{column}}\n"));
                }
                out.push_str(&format!("{ind}\\end{{{env}}}\n"));
            }
            Elem::Raw(r) => {
                for l in r.lines() {
                    if l.trim().is_empty() {
                        out.push('\n');
                    } else {
                        out.push_str(&format!("{ind}{l}\n"));
                    }
                }
            }
        }
    }
}

fn fmt_frac(f: f32) -> String {
    let s = format!("{:.2}", f.clamp(0.05, 1.0));
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "1" { String::new() } else { s.to_string() }
}

/// Clean LaTeX for one slide.
pub fn write_slide(s: &Slide) -> String {
    let mut out = String::from("\\begin{frame}");
    if !s.opts.trim().is_empty() {
        out.push_str(&format!("[{}]", s.opts.trim()));
    }
    if !s.title.trim().is_empty() {
        out.push_str(&format!("{{{}}}", s.title.trim()));
    }
    out.push('\n');
    write_elems(&mut out, &s.body, "  ");
    out.push_str("\\end{frame}");
    out
}

/// Escape characters that are special in LaTeX when typed into a visual text field.
pub fn escape_typed(ch: char, before: Option<char>) -> Option<&'static str> {
    if before == Some('\\') {
        return None;
    }
    Some(match ch {
        '%' => "\\%",
        '&' => "\\&",
        '#' => "\\#",
        '_' => "\\_",
        _ => return None,
    })
}

// ───────────────────────────── templates ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    Bullets,
    TwoColumns,
    Image,
    Block,
    Section,
    Blank,
}

pub fn new_slide(t: Template, tug: bool) -> Slide {
    let (ul, cols) = if tug { ("tugitemize", "tugcolumns") } else { ("itemize", "columns") };
    let item = |s: &str| Item { level: 0, overlay: String::new(), text: s.into() };
    let list = |items: Vec<Item>| Elem::List { env: ul.into(), numbered: false, step: false, items };
    let tr = |de: &'static str, en: &'static str| if crate::i18n::en() { en.to_string() } else { de.to_string() };
    match t {
        Template::Bullets => Slide { opts: String::new(), title: tr("Neue Folie", "New slide"), body: vec![list(vec![item(&tr("Erster Punkt", "First point")), item(&tr("Zweiter Punkt", "Second point"))])] },
        Template::TwoColumns => Slide {
            opts: String::new(),
            title: tr("Vergleich", "Comparison"),
            body: vec![Elem::Columns { env: cols.into(), cols: vec![vec![list(vec![item(&tr("Links", "Left"))])], vec![list(vec![item(&tr("Rechts", "Right"))])]] }],
        },
        Template::Image => Slide { opts: String::new(), title: tr("Abbildung", "Figure"), body: vec![Elem::Image { path: String::new(), height: 0.6 }] },
        Template::Block => Slide {
            opts: String::new(),
            title: tr("Kernaussage", "Key message"),
            body: vec![Elem::Block { kind: BlockKind::Block, title: tr("Merke", "Note"), body: vec![Elem::Text(tr("Text", "Text"))] }],
        },
        Template::Section => Slide { opts: "c".into(), title: String::new(), body: vec![Elem::Text(format!("\\centering\\Large {}", tr("Fragen?", "Questions?")))] },
        Template::Blank => Slide { opts: String::new(), title: tr("Neue Folie", "New slide"), body: vec![] },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r"\documentclass[aspectratio=169]{beamer}
\usetheme{tugraz2018}
\title[Short]{Hops, Yeast\\Pure Delight}
\author{Norbert Winter}
\begin{document}

\begin{frame}
  \maketitle
\end{frame}

\section{Motivation}

\begin{frame}{Why Beer?}
  \begin{tugitemize}
    \item Only four ingredients
    \item Much of the aroma is created in the \alert{fermenter}
      \begin{tugitemize}
        \item nested point
      \end{tugitemize}
    \item<2-> Systematic data are missing
  \end{tugitemize}
  \vspace{4mm}
  \begin{block}{Research Question}
    How strongly does temperature
    shape the aroma?
  \end{block}
\end{frame}

\begin{frame}{Approach}
  \begin{tugcolumns}
    \begin{column}{.48\linewidth}
      \begin{tugenumerate}[<+->]
        \item 12 brews
        \item Tasting
      \end{tugenumerate}
    \end{column}
    \begin{column}{.48\linewidth}
      \[ k(T) = A e^{-E_a/RT} \]
    \end{column}
  \end{tugcolumns}
\end{frame}

\begin{frame}{An Image}
  \centering
  \includegraphics[height=0.6\textheight]{figures/photo}
  \pause
  Some text after a pause.
\end{frame}

%\begin{frame}{commented}
%\end{frame}

\begin{frame}[c]
  \begin{tikzpicture}
    \draw (0,0) -- (1,1);
  \end{tikzpicture}
\end{frame}

\end{document}
";

    #[test]
    fn parses_deck() {
        let d = parse_deck(SRC);
        assert!(d.wide && d.tug);
        assert_eq!(d.frames.len(), 5);
        assert_eq!(d.meta.iter().find(|m| m.cmd == "title").unwrap().value, "Hops, Yeast\\\\Pure Delight");
        assert_eq!(d.frames[0].slide.body, vec![Elem::TitlePage]);
        let f1 = &d.frames[1];
        assert_eq!(f1.section, "Motivation");
        assert_eq!(f1.slide.title, "Why Beer?");
        let Elem::List { items, step, .. } = &f1.slide.body[0] else { panic!("{:?}", f1.slide.body) };
        assert!(!step);
        assert_eq!(items.len(), 4);
        assert_eq!(items[2], Item { level: 1, overlay: String::new(), text: "nested point".into() });
        assert_eq!(items[3].overlay, "<2->");
        assert_eq!(f1.slide.body[1], Elem::Raw("\\vspace{4mm}".into()));
        assert_eq!(f1.slide.body[2], Elem::Block { kind: BlockKind::Block, title: "Research Question".into(), body: vec![Elem::Text("How strongly does temperature shape the aroma?".into())] });
        let Elem::Columns { cols, .. } = &d.frames[2].slide.body[0] else { panic!() };
        assert_eq!(cols.len(), 2);
        assert!(matches!(&cols[0][0], Elem::List { numbered: true, step: true, .. }));
        assert_eq!(cols[1][0], Elem::Math("k(T) = A e^{-E_a/RT}".into()));
        assert_eq!(d.frames[3].slide.body, vec![Elem::Image { path: "figures/photo".into(), height: 0.6 }, Elem::Pause, Elem::Text("Some text after a pause.".into())]);
        assert!(matches!(&d.frames[4].slide.body[0], Elem::Raw(r) if r.contains("tikzpicture")));
        assert_eq!(d.frames[4].slide.opts, "c");
    }

    #[test]
    fn round_trip_is_stable() {
        let d = parse_deck(SRC);
        for f in &d.frames {
            let w = write_slide(&f.slide);
            let again = parse_deck(&format!("\\begin{{document}}\n{w}\n\\end{{document}}"));
            assert_eq!(again.frames[0].slide, f.slide, "\n{w}");
            // writing twice gives identical text
            assert_eq!(write_slide(&again.frames[0].slide), w);
        }
        let w = write_slide(&d.frames[1].slide);
        assert!(w.contains("  \\begin{tugitemize}\n    \\item Only four ingredients\n"), "{w}");
        assert!(w.contains("      \\begin{tugitemize}\n        \\item nested point\n      \\end{tugitemize}\n"), "{w}");
        assert!(w.contains("\\item<2-> Systematic"), "{w}");
    }

    #[test]
    fn templates_and_escape() {
        for t in [Template::Bullets, Template::TwoColumns, Template::Image, Template::Block, Template::Section, Template::Blank] {
            let s = new_slide(t, true);
            let w = write_slide(&s);
            let d = parse_deck(&format!("\\begin{{document}}\n{w}\n\\end{{document}}"));
            assert_eq!(d.frames[0].slide, s, "{w}");
        }
        assert_eq!(escape_typed('%', Some('a')), Some("\\%"));
        assert_eq!(escape_typed('%', Some('\\')), None);
        assert_eq!(escape_typed('a', None), None);
    }

    #[test]
    fn real_decks() {
        for src in [include_str!("../templates/masterarbeit/praesentation/folien.tex"), include_str!("../examples/bier-masterarbeit/praesentation/folien.tex")] {
            let d = parse_deck(src);
            assert!(d.frames.len() >= 6);
            for f in &d.frames {
                let w = write_slide(&f.slide);
                let again = parse_deck(&format!("\\begin{{document}}\n{w}\n\\end{{document}}"));
                assert_eq!(again.frames[0].slide, f.slide, "\n{w}");
                eprintln!("{:<14} {:?}", f.slide.title, f.slide.body.iter().map(|e| match e {
                    Elem::Text(_) => "Text", Elem::List { .. } => "List", Elem::Block { .. } => "Block", Elem::Image { .. } => "Image",
                    Elem::Columns { .. } => "Columns", Elem::Math(_) => "Math", Elem::Pause => "Pause", Elem::TitlePage => "Title",
                    Elem::Toc => "Toc", Elem::Raw(_) => "RAW" }).collect::<Vec<_>>());
            }
        }
    }
}
