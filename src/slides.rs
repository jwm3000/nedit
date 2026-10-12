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
    /// `\pause` before this item (it appears one step later)
    pub pause: bool,
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
    Table {
        /// preceded by \centering
        center: bool,
        /// column spec as written, e.g. `lrr`
        spec: String,
        rows: Vec<Vec<String>>,
        /// first row is a header (rule after it)
        header: bool,
        rules: Rules,
    },
    Pause,
    TitlePage,
    /// `\tableofcontents`, optionally in `multicols` (cols = 2 …)
    Toc { cols: u8 },
    Raw(String),
}

/// Horizontal lines of a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rules {
    /// \toprule / \midrule / \bottomrule (booktabs)
    Booktabs,
    /// \hline
    Hline,
    None,
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
    /// A `\section{…}` directly before this frame (no other frame in between):
    /// (start of the command, end of its line, title start, title end) as byte offsets.
    pub section_here: Option<(usize, usize, usize, usize)>,
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
    /// booktabs is loaded (new tables use \toprule …)
    pub booktabs: bool,
    /// Table of contents: (level 1 = section / 2 = subsection, title)
    pub toc: Vec<(u8, String)>,
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
    // a \pause seen since the last item: the next item appears one step later
    let mut pending_pause = false;
    let flush = |cur: &mut Option<Item>, text: &mut String, out: &mut Vec<Item>| {
        if let Some(mut it) = cur.take() {
            it.text = unwrap_par(text);
            out.push(it);
        }
        text.clear();
    };
    while i < body.len() {
        let r = &body[i..];
        if r.starts_with("\\pause") && !r[6..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            flush(&mut cur, &mut text, out);
            pending_pause = true;
            i += 6;
            continue;
        }
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
            cur = Some(Item { level, overlay, text: String::new(), pause: pending_pause });
            pending_pause = false;
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
    // a \pause after the last item has no item to attach to
    !pending_pause
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
                    simple(Elem::Toc { cols: 1 }, &mut par, &mut out);
                    i = line_end;
                    continue;
                }
                "\\centering" if par.trim().is_empty() && s[skip_ws(s, line_end)..].starts_with("\\begin{tabular}") => {
                    // \centering + tabular → centered table
                    let next = skip_ws(s, line_end);
                    let head = next + "\\begin{tabular}".len();
                    if let Some((e0, e1)) = env_end(s, head, "tabular") {
                        if let Some(Elem::Table { spec, rows, header, rules, .. }) = parse_env("tabular", s, head, e0) {
                            simple(Elem::Table { center: true, spec, rows, header, rules }, &mut par, &mut out);
                            i = e1;
                            continue;
                        }
                    }
                }
                "\\centering" if par.trim().is_empty() && {
                    let next = skip_ws(s, line_end);
                    let nl = s[next..].find('\n').map_or(s.len(), |p| next + p);
                    let l = s[next..nl].trim();
                    !l.is_empty() && !l.starts_with("\\begin") && !l.starts_with("\\includegraphics") && !l.starts_with('%') && !l.starts_with("\\[")
                } => {
                    // \centering before a paragraph: part of its style
                    par.push_str("\\centering ");
                    i = skip_ws(s, line_end);
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
            if let Some(rest) = body.strip_prefix("\\begin{tabular}") {
                let off = s[head_end..].find("\\begin{tabular}")? + head_end + "\\begin{tabular}".len();
                let (e0, e1) = env_end(s, off, "tabular")?;
                if !s[e1..body_end].trim().is_empty() {
                    return None;
                }
                let _ = rest;
                return match parse_env("tabular", s, off, e0)? {
                    Elem::Table { spec, rows, header, rules, .. } => Some(Elem::Table { center: true, spec, rows, header, rules }),
                    _ => None,
                };
            }
            image_from(body)
        }
        "tabular" => {
            let j = skip_sp(s, head_end);
            if !s[j..].starts_with('{') {
                return None;
            }
            let e = group_end(s, j)?;
            let spec = s[j + 1..e - 1].to_string();
            let (rows, header, rules) = parse_table(&s[e..body_end])?;
            Some(Elem::Table { center: false, spec, rows, header, rules })
        }
        "equation*" | "displaymath" => Some(Elem::Math(dedent(&s[head_end..body_end]).trim().to_string())),
        "multicols" => {
            // \begin{multicols}{2}\tableofcontents\end{multicols} (TU Graz outline slide)
            let inner = s[head_end..body_end].trim();
            let (cols, rest) = match inner.strip_prefix('{').and_then(|r| r.split_once('}')) {
                Some((n, rest)) => (n.trim().parse::<u8>().ok()?, rest.trim()),
                None => return None,
            };
            (rest == "\\tableofcontents").then_some(Elem::Toc { cols })
        }
        _ => None,
    }
}

/// Rows of a simple tabular body; None for anything fancy (multicolumn, nested envs …).
fn parse_table(body: &str) -> Option<(Vec<Vec<String>>, bool, Rules)> {
    if ["\\multicolumn", "\\multirow", "\\cline", "\\begin{", "\\cmidrule"].iter().any(|x| body.contains(x)) {
        return None;
    }
    if body.lines().any(|l| l.trim_start().starts_with('%')) {
        return None;
    }
    let rules = if body.contains("\\toprule") || body.contains("\\midrule") {
        Rules::Booktabs
    } else if body.contains("\\hline") {
        Rules::Hline
    } else {
        Rules::None
    };
    let mut rows: Vec<Vec<String>> = vec![];
    let mut header = false;
    for seg in body.split("\\\\") {
        let has_mid = seg.contains("\\midrule") || (rules == Rules::Hline && rows.len() == 1 && seg.contains("\\hline"));
        if has_mid && rows.len() == 1 {
            header = true;
        }
        let clean = seg.replace("\\toprule", "").replace("\\midrule", "").replace("\\bottomrule", "").replace("\\hline", "");
        let clean = clean.trim();
        if clean.is_empty() {
            continue;
        }
        // split on unescaped &
        let mut cells = vec![];
        let mut cur = String::new();
        let mut prev = ' ';
        for ch in clean.chars() {
            if ch == '&' && prev != '\\' {
                cells.push(cur.trim().to_string());
                cur.clear();
            } else {
                cur.push(ch);
            }
            prev = ch;
        }
        cells.push(cur.trim().to_string());
        rows.push(cells.into_iter().map(|c| c.split_whitespace().collect::<Vec<_>>().join(" ")).collect());
    }
    if rows.is_empty() {
        return None;
    }
    let n = rows.iter().map(Vec::len).max().unwrap_or(1);
    for r in &mut rows {
        r.resize(n, String::new());
    }
    Some((rows, header, rules))
}

static RE_FRAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\begin\{frame\}").unwrap());
static RE_SECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\section\*?(?:\[[^\]]*\])?\{([^{}]*)\}").unwrap());

/// Parse the whole presentation source.
pub fn parse_deck(src: &str) -> Deck {
    let mut deck = Deck { doc_end: src.rfind("\\end{document}").unwrap_or(src.len()), ..Default::default() };
    deck.wide = Regex::new(r"(?m)^[^%\n]*\\documentclass\[[^\]]*aspectratio=169").unwrap().is_match(src);
    deck.tug = src.contains("tugraz2018") || src.contains("tugitemize");
    deck.booktabs = src.contains("{booktabs}");
    let body_start = src.find("\\begin{document}").unwrap_or(0);
    let re_toc = Regex::new(r"(?m)^[ \t]*\\(section|subsection)(\*?)(?:\[[^\]]*\])?\{([^{}]*)\}").unwrap();
    for c in re_toc.captures_iter(&src[body_start..]) {
        if &c[2] == "*" || c[3].trim().is_empty() {
            continue;
        }
        deck.toc.push((if &c[1] == "section" { 1 } else { 2 }, c[3].trim().to_string()));
    }
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
        let mut section_here = None;
        for sm in RE_SECTION.captures_iter(&src[last..start]) {
            section = sm[1].to_string();
            let whole = sm.get(0).unwrap();
            let t = sm.get(1).unwrap();
            let (a, b) = (last + whole.start(), last + whole.end());
            let line_end = src[b..].find('\n').map_or(src.len(), |p| b + p + 1);
            section_here = Some((a, line_end, last + t.start(), last + t.end()));
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
        deck.frames.push(FrameRange { start, end: e1, section: section.clone(), section_here, slide: Slide { opts, title, body: parse_body(&dedent(body)) } });
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
            Elem::Toc { cols } if *cols > 1 => {
                out.push_str(&format!("{ind}\\begin{{multicols}}{{{cols}}}\n{ind}  \\tableofcontents\n{ind}\\end{{multicols}}\n"));
            }
            Elem::Toc { .. } => out.push_str(&format!("{ind}\\tableofcontents\n")),
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
                    if it.pause {
                        out.push_str(&format!("{pad}\\pause\n"));
                    }
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
            Elem::Table { center, spec, rows, header, rules } => {
                if *center {
                    out.push_str(&format!("{ind}\\centering\n"));
                }
                out.push_str(&format!("{ind}\\begin{{tabular}}{{{spec}}}\n"));
                let (top, mid, bot) = match rules {
                    Rules::Booktabs => (Some("\\toprule"), Some("\\midrule"), Some("\\bottomrule")),
                    Rules::Hline => (Some("\\hline"), Some("\\hline"), Some("\\hline")),
                    Rules::None => (None, None, None),
                };
                if let Some(t) = top {
                    out.push_str(&format!("{ind}  {t}\n"));
                }
                for (i, r) in rows.iter().enumerate() {
                    out.push_str(&format!("{ind}  {} \\\\\n", r.iter().map(|c| c.trim()).collect::<Vec<_>>().join(" & ")));
                    if i == 0 && *header && rows.len() > 1 {
                        if let Some(m) = mid {
                            out.push_str(&format!("{ind}  {m}\n"));
                        }
                    }
                }
                if let Some(b) = bot {
                    out.push_str(&format!("{ind}  {b}\n"));
                }
                out.push_str(&format!("{ind}\\end{{tabular}}\n"));
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


// ───────────────────────────── editing operations ─────────────────────────────
//
// Element paths: `[i]` = top-level element i; `[i, j]` = element j in the body of block i;
// `[i, c, j]` = element j in column c of columns element i.

/// The element at `path`.
pub fn elem_mut<'a>(body: &'a mut Vec<Elem>, path: &[usize]) -> Option<&'a mut Elem> {
    let (list, i) = parent_mut(body, path)?;
    list.get_mut(i)
}

pub fn elem_ref<'a>(body: &'a [Elem], path: &[usize]) -> Option<&'a Elem> {
    match path {
        [] => None,
        [i] => body.get(*i),
        [i, rest @ ..] => match body.get(*i)? {
            Elem::Block { body, .. } => elem_ref(body, rest),
            Elem::Columns { cols, .. } => {
                let (c, rest2) = rest.split_first()?;
                elem_ref(cols.get(*c)?, rest2)
            }
            _ => None,
        },
    }
}

/// The list containing `path` and the index in it.
pub fn parent_mut<'a>(body: &'a mut Vec<Elem>, path: &[usize]) -> Option<(&'a mut Vec<Elem>, usize)> {
    match path {
        [] => None,
        [i] => Some((body, *i)),
        [i, rest @ ..] => match body.get_mut(*i)? {
            Elem::Block { body, .. } => parent_mut(body, rest),
            Elem::Columns { cols, .. } => {
                let (c, rest2) = rest.split_first()?;
                parent_mut(cols.get_mut(*c)?, rest2)
            }
            _ => None,
        },
    }
}

fn with_last(path: &[usize], i: usize) -> Vec<usize> {
    let mut p = path.to_vec();
    if let Some(l) = p.last_mut() {
        *l = i;
    }
    p
}

/// Insert after the element at `after` (same container), or at the end of the slide.
pub fn insert_after(body: &mut Vec<Elem>, after: Option<&[usize]>, e: Elem) -> Vec<usize> {
    if let Some(p) = after {
        if let Some((list, i)) = parent_mut(body, p) {
            let at = (i + 1).min(list.len());
            list.insert(at, e);
            return with_last(p, at);
        }
    }
    body.push(e);
    vec![body.len() - 1]
}

/// Insert before the element at `path` (same container).
pub fn insert_before(body: &mut Vec<Elem>, path: &[usize], e: Elem) -> Vec<usize> {
    if let Some((list, i)) = parent_mut(body, path) {
        let at = i.min(list.len());
        list.insert(at, e);
        return with_last(path, at);
    }
    body.push(e);
    vec![body.len() - 1]
}

/// Remove an element; returns the path of a neighbour to select (if any).
pub fn delete(body: &mut Vec<Elem>, path: &[usize]) -> Option<Vec<usize>> {
    let (list, i) = parent_mut(body, path)?;
    if i >= list.len() {
        return None;
    }
    list.remove(i);
    if list.is_empty() {
        // keep blocks/columns editable: an empty container gets an empty paragraph
        if path.len() > 1 {
            list.push(Elem::Text(String::new()));
            return Some(with_last(path, 0));
        }
        return None;
    }
    // the element that moved into the gap, or the new last one
    Some(with_last(path, i.min(list.len() - 1)))
}

pub fn duplicate(body: &mut Vec<Elem>, path: &[usize]) -> Option<Vec<usize>> {
    let e = elem_ref(body, path)?.clone();
    Some(insert_after(body, Some(path), e))
}

/// Move up (-1) or down (+1) within the container; returns the new path.
pub fn move_by(body: &mut Vec<Elem>, path: &[usize], delta: i32) -> Option<Vec<usize>> {
    let (list, i) = parent_mut(body, path)?;
    let j = i as i32 + delta;
    if j < 0 || j as usize >= list.len() {
        return None;
    }
    list.swap(i, j as usize);
    Some(with_last(path, j as usize))
}

fn split_chars(s: &str, at: usize) -> (String, String) {
    let c: Vec<char> = s.chars().collect();
    let at = at.min(c.len());
    (c[..at].iter().collect(), c[at..].iter().collect())
}

/// Enter in a paragraph: split it at `at`; returns the path of the new paragraph.
pub fn split_text(body: &mut Vec<Elem>, path: &[usize], at: usize) -> Option<Vec<usize>> {
    let Some(Elem::Text(t)) = elem_mut(body, path) else { return None };
    let (head, tail) = split_chars(t, at);
    // keep \centering / size of the paragraph for the new one
    let (center, size, _) = text_style(&head);
    *t = head.trim_end().to_string();
    let tail = set_text_style(tail.trim_start(), center, size);
    Some(insert_after(body, Some(path), Elem::Text(tail)))
}

/// Backspace at the start of a paragraph: merge into the previous paragraph.
/// Returns the previous path and the caret position there.
pub fn merge_text_prev(body: &mut Vec<Elem>, path: &[usize]) -> Option<(Vec<usize>, usize)> {
    let i = *path.last()?;
    if i == 0 {
        return None;
    }
    let prev = with_last(path, i - 1);
    let Some(Elem::Text(cur)) = elem_ref(body, path).cloned() else { return None };
    let Some(Elem::Text(p)) = elem_mut(body, &prev) else { return None };
    let caret = p.chars().count();
    let (_, _, rest) = text_style(&cur);
    let rest = rest.to_string();
    if !p.is_empty() && !rest.is_empty() && !p.ends_with(' ') {
        p.push(' ');
    }
    let caret = if p.chars().count() > caret { caret + 1 } else { caret };
    p.push_str(&rest);
    let (list, idx) = parent_mut(body, path)?;
    list.remove(idx);
    Some((prev, caret))
}

/// What happens after Enter/Backspace in a list item.
#[derive(Debug, PartialEq)]
pub enum ListResult {
    /// focus item `idx` at char `caret`
    Item(usize, usize),
    /// the list ended: a paragraph was added after it (path of the paragraph)
    Exit(Vec<usize>),
    /// the list was removed completely; select this path (if any)
    Removed(Option<Vec<usize>>),
}

/// Enter in item `idx` at `at`: empty item → outdent / leave the list; otherwise split.
pub fn list_enter(body: &mut Vec<Elem>, path: &[usize], idx: usize, at: usize) -> Option<ListResult> {
    let Some(Elem::List { items, .. }) = elem_mut(body, path) else { return None };
    let it = items.get(idx)?.clone();
    if it.text.trim().is_empty() {
        if it.level > 0 {
            items[idx].level -= 1;
            return Some(ListResult::Item(idx, 0));
        }
        items.remove(idx);
        let tail: Vec<Item> = items.drain(idx..).collect();
        let empty = items.is_empty();
        // leave the list: new paragraph after it (items after the cursor form a new list)
        let list_clone = elem_ref(body, path).cloned();
        let par_path = insert_after(body, Some(path), Elem::Text(String::new()));
        if !tail.is_empty() {
            if let Some(Elem::List { env, numbered, step, .. }) = list_clone {
                insert_after(body, Some(&par_path), Elem::List { env, numbered, step, items: tail });
            }
        }
        if empty {
            delete(body, path);
            return Some(ListResult::Exit(path.to_vec()));
        }
        return Some(ListResult::Exit(par_path));
    }
    let (head, tail) = split_chars(&it.text, at);
    items[idx].text = head.trim_end().to_string();
    items.insert(idx + 1, Item { level: it.level, overlay: String::new(), text: tail.trim_start().to_string(), pause: false });
    Some(ListResult::Item(idx + 1, 0))
}

/// Backspace at the start of item `idx`: outdent, merge with the previous item, or remove.
pub fn list_backspace(body: &mut Vec<Elem>, path: &[usize], idx: usize) -> Option<ListResult> {
    let Some(Elem::List { items, .. }) = elem_mut(body, path) else { return None };
    if items.get(idx)?.level > 0 {
        items[idx].level -= 1;
        return Some(ListResult::Item(idx, 0));
    }
    if idx > 0 {
        let cur = items.remove(idx);
        let prev = &mut items[idx - 1];
        let caret = prev.text.chars().count();
        if !prev.text.is_empty() && !cur.text.is_empty() {
            prev.text.push(' ');
        }
        let caret = if prev.text.chars().count() > caret { caret + 1 } else { caret };
        prev.text.push_str(&cur.text);
        return Some(ListResult::Item(idx - 1, caret));
    }
    if items.len() == 1 && items[0].text.is_empty() {
        let sel = delete(body, path);
        return Some(ListResult::Removed(sel));
    }
    None
}

/// Indent (+1) / outdent (-1) an item; a nested item can be at most one level deeper
/// than the one before it.
pub fn list_indent(body: &mut Vec<Elem>, path: &[usize], idx: usize, delta: i32) -> bool {
    let Some(Elem::List { items, .. }) = elem_mut(body, path) else { return false };
    let max = if idx == 0 { 0 } else { items[idx - 1].level as i32 + 1 };
    let l = (items[idx].level as i32 + delta).clamp(0, max.min(3));
    let changed = l != items[idx].level as i32;
    items[idx].level = l as u8;
    changed
}

const SIZES: [&str; 7] = ["\\footnotesize", "\\small", "", "\\large", "\\Large", "\\LARGE", "\\huge"];

/// Paragraph style from leading commands: (centered, size step -2…4, rest of the text).
pub fn text_style(t: &str) -> (bool, i32, &str) {
    let mut rest = t.trim_start();
    let mut center = false;
    let mut size = 0;
    loop {
        if let Some(r) = rest.strip_prefix("\\centering") {
            center = true;
            rest = r.trim_start();
            continue;
        }
        let mut hit = false;
        for (i, cmd) in SIZES.iter().enumerate() {
            if cmd.is_empty() {
                continue;
            }
            if let Some(r) = rest.strip_prefix(cmd) {
                if !r.starts_with(|c: char| c.is_ascii_alphabetic()) {
                    size = i as i32 - 2;
                    rest = r.trim_start();
                    hit = true;
                    break;
                }
            }
        }
        if !hit {
            break;
        }
    }
    (center, size, rest)
}

pub fn set_text_style(t: &str, center: bool, size: i32) -> String {
    let (_, _, rest) = text_style(t);
    let mut out = String::new();
    if center {
        out.push_str("\\centering ");
    }
    let cmd = SIZES[(size.clamp(-2, 4) + 2) as usize];
    if !cmd.is_empty() {
        out.push_str(cmd);
        out.push(' ');
    }
    out.push_str(rest);
    out
}

/// A new table (3×3 with header).
pub fn new_table(booktabs: bool) -> Elem {
    let h = |s: &str| s.to_string();
    let (a, b, c) = if crate::i18n::en() { ("Column 1", "Column 2", "Column 3") } else { ("Spalte 1", "Spalte 2", "Spalte 3") };
    Elem::Table {
        center: true,
        spec: "lll".into(),
        rows: vec![vec![h(a), h(b), h(c)], vec![String::new(), String::new(), String::new()], vec![String::new(), String::new(), String::new()]],
        header: true,
        rules: if booktabs { Rules::Booktabs } else { Rules::Hline },
    }
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
    let item = |s: &str| Item { level: 0, overlay: String::new(), text: s.into(), pause: false };
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
        assert_eq!(items[2], Item { level: 1, overlay: String::new(), text: "nested point".into(), pause: false });
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
                    Elem::Toc { .. } => "Toc", Elem::Table { .. } => "Table", Elem::Raw(_) => "RAW" }).collect::<Vec<_>>());
            }
        }
    }

    fn body(src: &str) -> Vec<Elem> {
        parse_body(src)
    }

    #[test]
    fn table_round_trip() {
        let src = "\\begin{document}\n\\begin{frame}{T}\n  \\centering\n  \\begin{tabular}{lrr}\n    \\toprule\n    A & B & C \\\\\n    \\midrule\n    1 & 2 & 3 \\\\\n    4 & 5 \\& x & 6 \\\\\n    \\bottomrule\n  \\end{tabular}\n\\end{frame}\n\\end{document}\n";
        let d = parse_deck(src);
        let Elem::Table { center, spec, rows, header, rules } = &d.frames[0].slide.body[0] else { panic!("{:?}", d.frames[0].slide.body) };
        assert!(*center && *header && *rules == Rules::Booktabs);
        assert_eq!(spec, "lrr");
        assert_eq!(rows[2], vec!["4", "5 \\& x", "6"]);
        let w = write_slide(&d.frames[0].slide);
        let again = parse_deck(&format!("\\begin{{document}}\n{w}\n\\end{{document}}"));
        assert_eq!(again.frames[0].slide, d.frames[0].slide, "{w}");
        // hline tables keep their style
        let b = body("\\begin{tabular}{|l|l|}\n\\hline\na & b \\\\\n\\hline\nc & d \\\\\n\\hline\n\\end{tabular}");
        assert!(matches!(&b[0], Elem::Table { rules: Rules::Hline, header: true, center: false, .. }), "{b:?}");
        // fancy tables stay raw
        let b = body("\\begin{tabular}{ll}\n\\multicolumn{2}{c}{x} \\\\\n\\end{tabular}");
        assert!(matches!(&b[0], Elem::Raw(_)));
        let t = new_table(true);
        let s = Slide { opts: String::new(), title: "T".into(), body: vec![t] };
        let w = write_slide(&s);
        assert_eq!(parse_deck(&format!("\\begin{{document}}\n{w}\n\\end{{document}}")).frames[0].slide, s);
    }

    #[test]
    fn sections_on_frames() {
        let src = "\\begin{document}\n\\begin{frame}\\maketitle\\end{frame}\n\n\\section{Intro}\n\n\\begin{frame}{A}\nx\n\\end{frame}\n\\begin{frame}{B}\ny\n\\end{frame}\n\\end{document}";
        let d = parse_deck(src);
        assert!(d.frames[0].section_here.is_none());
        let (a, e, t0, t1) = d.frames[1].section_here.unwrap();
        assert_eq!(&src[t0..t1], "Intro");
        assert_eq!(&src[a..e], "\\section{Intro}\n");
        assert_eq!(d.frames[2].section, "Intro");
        assert!(d.frames[2].section_here.is_none());
    }

    #[test]
    fn paragraph_ops() {
        let mut b = vec![Elem::Text("Hallo Welt".into())];
        let p = split_text(&mut b, &[0], 5).unwrap();
        assert_eq!(p, vec![1]);
        assert_eq!(b, vec![Elem::Text("Hallo".into()), Elem::Text("Welt".into())]);
        let (prev, caret) = merge_text_prev(&mut b, &[1]).unwrap();
        assert_eq!((prev, caret), (vec![0], 6));
        assert_eq!(b, vec![Elem::Text("Hallo Welt".into())]);
        // style survives a split
        let mut b = vec![Elem::Text("\\centering \\Large Fragen? Danke".into())];
        split_text(&mut b, &[0], "\\centering \\Large Fragen?".len()).unwrap();
        assert_eq!(b[1], Elem::Text("\\centering \\Large Danke".into()));
        assert_eq!(text_style("\\centering\\Large Hi"), (true, 2, "Hi"));
        assert_eq!(set_text_style("\\Large Hi", false, 0), "Hi");
        // move, duplicate, delete in a column
        let mut b = vec![Elem::Columns { env: "columns".into(), cols: vec![vec![Elem::Text("a".into()), Elem::Text("b".into())], vec![]] }];
        assert_eq!(move_by(&mut b, &[0, 0, 0], 1), Some(vec![0, 0, 1]));
        assert_eq!(duplicate(&mut b, &[0, 0, 1]), Some(vec![0, 0, 2]));
        assert_eq!(delete(&mut b, &[0, 0, 0]), Some(vec![0, 0, 0]));
        let Elem::Columns { cols, .. } = &b[0] else { panic!() };
        assert_eq!(cols[0], vec![Elem::Text("a".into()), Elem::Text("a".into())]);
        // inserting into an empty slide
        let mut e: Vec<Elem> = vec![];
        assert_eq!(insert_after(&mut e, None, Elem::Pause), vec![0]);
        assert_eq!(insert_after(&mut e, Some(&[7]), Elem::Toc { cols: 1 }), vec![1]);
    }

    #[test]
    fn list_ops() {
        let it = |l: u8, t: &str| Item { level: l, overlay: String::new(), text: t.into(), pause: false };
        let list = |items: Vec<Item>| Elem::List { env: "itemize".into(), numbered: false, step: false, items };
        // split
        let mut b = vec![list(vec![it(0, "eins zwei")])];
        assert_eq!(list_enter(&mut b, &[0], 0, 4), Some(ListResult::Item(1, 0)));
        assert_eq!(b[0], list(vec![it(0, "eins"), it(0, "zwei")]));
        // empty nested item → outdent, empty top item → leave the list
        let mut b = vec![list(vec![it(0, "a"), it(1, "")])];
        assert_eq!(list_enter(&mut b, &[0], 1, 0), Some(ListResult::Item(1, 0)));
        assert_eq!(list_enter(&mut b, &[0], 1, 0), Some(ListResult::Exit(vec![1])));
        assert_eq!(b, vec![list(vec![it(0, "a")]), Elem::Text(String::new())]);
        // leaving in the middle splits the list
        let mut b = vec![list(vec![it(0, "a"), it(0, ""), it(0, "c")])];
        assert_eq!(list_enter(&mut b, &[0], 1, 0), Some(ListResult::Exit(vec![1])));
        assert_eq!(b, vec![list(vec![it(0, "a")]), Elem::Text(String::new()), list(vec![it(0, "c")])]);
        // backspace: outdent, then merge
        let mut b = vec![list(vec![it(0, "a"), it(1, "b")])];
        assert_eq!(list_backspace(&mut b, &[0], 1), Some(ListResult::Item(1, 0)));
        assert_eq!(list_backspace(&mut b, &[0], 1), Some(ListResult::Item(0, 2)));
        assert_eq!(b[0], list(vec![it(0, "a b")]));
        // indent limits
        let mut b = vec![list(vec![it(0, "a"), it(0, "b")])];
        assert!(!list_indent(&mut b, &[0], 0, 1));
        assert!(list_indent(&mut b, &[0], 1, 1));
        assert!(!list_indent(&mut b, &[0], 1, 1));
    }
}
#[cfg(test)]
mod pause_toc_tests {
    use super::*;

    fn one(body: &str) -> Slide {
        let src = format!("\\begin{{document}}\n\\begin{{frame}}{{T}}\n{body}\n\\end{{frame}}\n\\end{{document}}\n");
        parse_deck(&src).frames[0].slide.clone()
    }

    fn round_trip(s: &Slide) {
        let w = write_slide(s);
        let again = parse_deck(&format!("\\begin{{document}}\n{w}\n\\end{{document}}")).frames[0].slide.clone();
        assert_eq!(&again, s, "\n{w}");
    }

    #[test]
    fn nested_list_with_pauses() {
        let body = "  \\begin{tugenumerate}\n    \\item First item\n      \\begin{tugenumerate}\n        \\item First subitem\n          \\begin{tugenumerate}\n            \\item First \n            \\item Second\n          \\end{tugenumerate}\n      \\end{tugenumerate}\n      \\pause\n    \\item Second item\n      \\pause\n    \\item Third item\n  \\end{tugenumerate}";
        let s = one(body);
        let Elem::List { items, numbered, .. } = &s.body[0] else { panic!("{:?}", s.body) };
        assert!(*numbered);
        let v: Vec<(u8, bool, &str)> = items.iter().map(|i| (i.level, i.pause, i.text.as_str())).collect();
        assert_eq!(v, vec![(0, false, "First item"), (1, false, "First subitem"), (2, false, "First"), (2, false, "Second"), (0, true, "Second item"), (0, true, "Third item")]);
        round_trip(&s);
        let w = write_slide(&s);
        assert!(w.contains("    \\pause\n    \\item Second item"), "{w}");
        // a trailing \pause in a list cannot be represented → raw
        assert!(matches!(one("\\begin{itemize}\n\\item a\n\\pause\n\\end{itemize}").body[0], Elem::Raw(_)));
    }

    #[test]
    fn toc_and_centering() {
        let s = one("  \\begin{multicols}{2}\n    \\tableofcontents\n  \\end{multicols}");
        assert_eq!(s.body, vec![Elem::Toc { cols: 2 }]);
        round_trip(&s);
        let s = one("  \\tableofcontents\n\n  es ist perfekt");
        assert_eq!(s.body[0], Elem::Toc { cols: 1 });
        let s = one("  \\centering\n  \\Large Questions?");
        assert_eq!(s.body, vec![Elem::Text("\\centering \\Large Questions?".into())]);
        round_trip(&s);
        let d = parse_deck("\\begin{document}\n\\section{A}\n\\subsection{A1}\n\\section*{}\n\\section{B}\n\\begin{frame}x\\end{frame}\\end{document}");
        assert_eq!(d.toc, vec![(1, "A".into()), (2, "A1".into()), (1, "B".into())]);
    }
}

#[cfg(test)]
mod user_deck_probe {
    #[test]
    #[ignore]
    fn probe() {
        let p = std::env::var("PROBE").unwrap();
        let src = std::fs::read_to_string(p).unwrap();
        let d = super::parse_deck(&src);
        for f in &d.frames {
            eprintln!("{:?} section={:?} {:?}", f.slide.title, f.section, f.slide.body.iter().map(|e| format!("{e:?}").chars().take(60).collect::<String>()).collect::<Vec<_>>());
        }
    }
}
