//! Vim emulation for the editor: modes, motions, operators, text objects, registers,
//! `.` repeat and an ex command line. The core (`process`) works on a plain `String`
//! and is unit-tested; `handle` adapts it to egui input events.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
    VisualBlock,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::VisualBlock => "V-BLOCK",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum VKey {
    Ch(char),
    Esc,
    Enter,
    Back,
    Del,
    Tab,
    Ctrl(char),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

#[derive(Default, Debug, Clone)]
pub struct VimOut {
    pub changed: bool,
    pub save: bool,
    pub close: bool,
    pub search: Option<String>,
    pub noh: bool,
    pub undo: usize,
    pub redo: usize,
    pub yanked: Option<String>,
    pub scroll_center: bool,
}

#[derive(Default)]
pub struct VimState {
    pub mode: Mode,
    pub pos: usize,
    anchor: usize,
    want_col: Option<usize>,
    pending: Vec<VKey>,
    register: String,
    reg_line: bool,
    pub cmdline: Option<String>,
    pub message: String,
    last_find: Option<(char, char)>,
    last_search: String,
    last_set: Option<(usize, usize)>,
    buf_id: Option<egui::Id>,
    last_change: Vec<VKey>,
    insert_log: Vec<VKey>,
    recording: bool,
    replaying: bool,
    reg_block: bool,
    block_eol: bool,
    block_ins: Option<BlockIns>,
}

/// Pending visual-block insert (`I`, `A`, `c`): replicate the typed text on Esc.
#[derive(Clone, Debug)]
struct BlockIns {
    lines: Vec<usize>,
    col: usize,
    pad: bool,
    eol: bool,
}

enum Step {
    Wait,
    Done,
    Bad,
}

/// Result of a motion: target position, linewise?, inclusive?
#[derive(Clone, Copy)]
struct Mv {
    to: usize,
    line: bool,
    incl: bool,
}

enum MRes {
    Wait,
    Bad,
    To(Mv),
    Range(usize, usize, bool), // text object: [a, b), linewise
}

// ───────────────────────────── text helpers ─────────────────────────────

fn ls(c: &[char], p: usize) -> usize {
    let mut i = p.min(c.len());
    while i > 0 && c[i - 1] != '\n' {
        i -= 1;
    }
    i
}
fn le(c: &[char], p: usize) -> usize {
    let mut i = p.min(c.len());
    while i < c.len() && c[i] != '\n' {
        i += 1;
    }
    i
}
fn first_nb(c: &[char], p: usize) -> usize {
    let mut i = ls(c, p);
    let e = le(c, p);
    while i < e && (c[i] == ' ' || c[i] == '\t') {
        i += 1;
    }
    i
}
fn line_of(c: &[char], p: usize) -> usize {
    c[..p.min(c.len())].iter().filter(|&&x| x == '\n').count()
}
fn line_start_n(c: &[char], n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut k = 0;
    for (i, &ch) in c.iter().enumerate() {
        if ch == '\n' {
            k += 1;
            if k == n {
                return i + 1;
            }
        }
    }
    ls(c, c.len())
}
fn line_count(c: &[char]) -> usize {
    c.iter().filter(|&&x| x == '\n').count() + 1
}
#[derive(PartialEq, Clone, Copy)]
enum Cls {
    Space,
    Word,
    Punct,
}
fn cls(ch: char, big: bool) -> Cls {
    if ch.is_whitespace() {
        Cls::Space
    } else if big || ch.is_alphanumeric() || ch == '_' {
        Cls::Word
    } else {
        Cls::Punct
    }
}

fn word_fwd(c: &[char], mut p: usize, big: bool) -> usize {
    let n = c.len();
    if p >= n {
        return n;
    }
    let k = cls(c[p], big);
    if k != Cls::Space {
        while p < n && cls(c[p], big) == k {
            p += 1;
        }
    }
    while p < n && c[p].is_whitespace() {
        if c[p] == '\n' && p + 1 < n && c[p + 1] == '\n' {
            return p + 1; // empty line is a word
        }
        p += 1;
    }
    p
}
fn word_end(c: &[char], mut p: usize, big: bool) -> usize {
    let n = c.len();
    if p + 1 >= n {
        return n.saturating_sub(1);
    }
    p += 1;
    while p < n && c[p].is_whitespace() {
        p += 1;
    }
    if p >= n {
        return n - 1;
    }
    let k = cls(c[p], big);
    while p + 1 < n && cls(c[p + 1], big) == k {
        p += 1;
    }
    p
}
fn word_back(c: &[char], mut p: usize, big: bool) -> usize {
    if p == 0 {
        return 0;
    }
    p -= 1;
    while p > 0 && c[p].is_whitespace() {
        p -= 1;
    }
    let k = cls(c[p], big);
    while p > 0 && cls(c[p - 1], big) == k {
        p -= 1;
    }
    p
}

fn match_pair(c: &[char], p: usize) -> Option<usize> {
    let e = le(c, p);
    let mut i = p;
    while i < e && !"(){}[]".contains(c[i]) {
        i += 1;
    }
    if i >= e {
        return None;
    }
    let (open, close, fwd) = match c[i] {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        '[' => ('[', ']', true),
        _ => ('[', ']', false),
    };
    let mut depth = 0i32;
    if fwd {
        for j in i..c.len() {
            if c[j] == open {
                depth += 1;
            } else if c[j] == close {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
        }
    } else {
        let mut j = i as isize;
        while j >= 0 {
            let ch = c[j as usize];
            if ch == close {
                depth += 1;
            } else if ch == open {
                depth -= 1;
                if depth == 0 {
                    return Some(j as usize);
                }
            }
            j -= 1;
        }
    }
    None
}

/// Enclosing bracket pair around p: (open_idx, close_idx).
fn enclosing(c: &[char], p: usize, open: char, close: char) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let mut a = None;
    let mut j = p.min(c.len().saturating_sub(1)) as isize;
    if p < c.len() && c[p] == open {
        a = Some(p);
    } else {
        while j >= 0 {
            let ch = c[j as usize];
            if ch == close && j as usize != p {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    a = Some(j as usize);
                    break;
                }
                depth -= 1;
            }
            j -= 1;
        }
    }
    let a = a?;
    let mut depth = 0i32;
    for k in a..c.len() {
        if c[k] == open {
            depth += 1;
        } else if c[k] == close {
            depth -= 1;
            if depth == 0 {
                return Some((a, k));
            }
        }
    }
    None
}

fn quote_pair(c: &[char], p: usize, q: char) -> Option<(usize, usize)> {
    let s = ls(c, p);
    let e = le(c, p);
    let idx: Vec<usize> = (s..e).filter(|&i| c[i] == q && (i == 0 || c[i - 1] != '\\')).collect();
    for w in idx.chunks(2) {
        if w.len() == 2 && w[0] <= p && p <= w[1] {
            return Some((w[0], w[1]));
        }
    }
    // cursor before the first pair on the line
    idx.iter().position(|&i| i > p).and_then(|k| if k + 1 < idx.len() { Some((idx[k], idx[k + 1])) } else { None })
}

fn key_char(k: &VKey) -> Option<char> {
    match k {
        VKey::Ch(c) => Some(*c),
        _ => None,
    }
}

// ───────────────────────────── core ─────────────────────────────

impl VimState {
    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn set_text(text: &mut String, c: &[char]) {
        *text = c.iter().collect();
    }

    /// Clamp the cursor for Normal mode (never on the newline of a non-empty line).
    fn clamp(&mut self, c: &[char]) {
        self.pos = self.pos.min(c.len());
        if self.mode != Mode::Insert {
            let e = le(c, self.pos);
            let s = ls(c, self.pos);
            if self.pos >= e && e > s {
                self.pos = e - 1;
            }
        }
    }

    fn yank(&mut self, s: String, line: bool, out: &mut VimOut) {
        out.yanked = Some(s.clone());
        self.register = s;
        self.reg_line = line;
        self.reg_block = false;
    }

    /// Lines and columns spanned by the visual block: (first_line, last_line, first_col, last_col).
    fn block_bounds(&self, c: &[char]) -> (usize, usize, usize, usize) {
        let (la, lb) = (line_of(c, self.anchor), line_of(c, self.pos));
        let (ca, cb) = (self.anchor - ls(c, self.anchor), self.pos - ls(c, self.pos));
        (la.min(lb), la.max(lb), ca.min(cb), ca.max(cb))
    }

    /// Char ranges of the visual block, one per line (for drawing and operating).
    pub fn block_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        if self.mode != Mode::VisualBlock {
            return vec![];
        }
        let c: Vec<char> = text.chars().collect();
        self.block_ranges_c(&c)
    }

    fn block_ranges_c(&self, c: &[char]) -> Vec<(usize, usize)> {
        let (l0, l1, c0, c1) = self.block_bounds(c);
        (l0..=l1)
            .filter_map(|ln| {
                let s = line_start_n(c, ln);
                let e = le(c, s);
                let a = s + c0;
                if a > e || (a == e && !self.block_eol) {
                    return None;
                }
                let b = if self.block_eol { e } else { (s + c1 + 1).min(e) };
                Some((a, b))
            })
            .collect()
    }

    fn block_cmd(&mut self, text: &mut String, k: &VKey, rest: &[VKey], n: usize, out: &mut VimOut) -> Step {
        let mut c = Self::chars(text);
        let (l0, l1, c0, c1) = self.block_bounds(&c);
        let ranges = self.block_ranges_c(&c);
        let top_left = line_start_n(&c, l0) + c0.min(le(&c, line_start_n(&c, l0)) - line_start_n(&c, l0));
        match k {
            VKey::Ch('o') | VKey::Ch('O') => {
                std::mem::swap(&mut self.anchor, &mut self.pos);
                Step::Done
            }
            VKey::Ch('$') | VKey::End => {
                self.block_eol = true;
                let e = le(&c, self.pos);
                self.pos = e.saturating_sub(1).max(ls(&c, self.pos));
                Step::Done
            }
            VKey::Ch('d') | VKey::Ch('x') | VKey::Del | VKey::Ch('y') => {
                let parts: Vec<String> = (l0..=l1)
                    .map(|ln| {
                        ranges.iter().find(|r| line_of(&c, r.0) == ln).map(|&(a, b)| c[a..b].iter().collect()).unwrap_or_default()
                    })
                    .collect();
                self.yank(parts.join("\n"), false, out);
                self.reg_block = true;
                if *k != VKey::Ch('y') {
                    for &(a, b) in ranges.iter().rev() {
                        c.drain(a..b);
                    }
                    Self::set_text(text, &c);
                    out.changed = true;
                }
                self.pos = top_left;
                self.mode = Mode::Normal;
                Step::Done
            }
            VKey::Ch('c') | VKey::Ch('s') | VKey::Ch('I') | VKey::Ch('A') => {
                let change = matches!(k, VKey::Ch('c') | VKey::Ch('s'));
                if change {
                    for &(a, b) in ranges.iter().rev() {
                        c.drain(a..b);
                    }
                    Self::set_text(text, &c);
                    out.changed = true;
                }
                let append = *k == VKey::Ch('A');
                let col = if append { c1 + 1 } else { c0 };
                let eol = append && self.block_eol;
                let s0 = line_start_n(&c, l0);
                let e0 = le(&c, s0);
                self.pos = if eol { e0 } else { (s0 + col).min(e0) };
                if append && !eol && s0 + col > e0 {
                    for _ in e0..s0 + col {
                        c.insert(e0, ' ');
                    }
                    Self::set_text(text, &c);
                    self.pos = s0 + col;
                }
                self.mode = Mode::Normal;
                self.enter_insert();
                self.block_ins = Some(BlockIns { lines: (l0 + 1..=l1).collect(), col, pad: append, eol });
                Step::Done
            }
            VKey::Ch('r') => {
                let Some(next) = rest.first() else { return Step::Wait };
                let Some(ch) = key_char(next) else { return Step::Bad };
                for &(a, b) in &ranges {
                    for x in a..b {
                        c[x] = ch;
                    }
                }
                Self::set_text(text, &c);
                out.changed = true;
                self.pos = top_left;
                self.mode = Mode::Normal;
                Step::Done
            }
            VKey::Ch('~') | VKey::Ch('u') | VKey::Ch('U') => {
                for &(a, b) in &ranges {
                    for x in a..b {
                        let ch = c[x];
                        c[x] = match k {
                            VKey::Ch('u') => ch.to_lowercase().next().unwrap_or(ch),
                            VKey::Ch('U') => ch.to_uppercase().next().unwrap_or(ch),
                            _ if ch.is_uppercase() => ch.to_lowercase().next().unwrap_or(ch),
                            _ => ch.to_uppercase().next().unwrap_or(ch),
                        };
                    }
                }
                Self::set_text(text, &c);
                out.changed = true;
                self.pos = top_left;
                self.mode = Mode::Normal;
                Step::Done
            }
            VKey::Ch('>') | VKey::Ch('<') => {
                let a = line_start_n(&c, l0);
                let b = le(&c, line_start_n(&c, l1));
                self.mode = Mode::Normal;
                let op = if *k == VKey::Ch('>') { '>' } else { '<' };
                self.operate_range(text, op, a, b, true, out)
            }
            _ => {
                let mut seq = vec![k.clone()];
                seq.extend_from_slice(rest);
                match self.motion(&c, &seq, n, None) {
                    MRes::Wait => Step::Wait,
                    MRes::To(mv) => {
                        if !matches!(k, VKey::Ch('j') | VKey::Ch('k') | VKey::Up | VKey::Down) {
                            self.block_eol = false;
                        }
                        self.pos = mv.to;
                        Step::Done
                    }
                    _ => Step::Bad,
                }
            }
        }
    }

    fn enter_insert(&mut self) {
        self.mode = Mode::Insert;
        if !self.replaying {
            self.recording = true;
            self.insert_log.clear();
        }
    }

    /// Process key tokens on `text`. `self.pos` is the cursor (char index).
    pub fn process(&mut self, text: &mut String, toks: &[VKey]) -> VimOut {
        let mut out = VimOut::default();
        for t in toks {
            self.feed(text, t.clone(), &mut out);
        }
        let c = Self::chars(text);
        self.clamp(&c);
        out
    }

    fn feed(&mut self, text: &mut String, t: VKey, out: &mut VimOut) {
        // ex command line
        if let Some(cl) = &mut self.cmdline {
            match t {
                VKey::Esc => self.cmdline = None,
                VKey::Back => {
                    cl.pop();
                    if cl.is_empty() {
                        self.cmdline = None;
                    }
                }
                VKey::Enter => {
                    let cmd = self.cmdline.take().unwrap_or_default();
                    self.ex(text, &cmd, out);
                }
                VKey::Ch(ch) => cl.push(ch),
                VKey::Tab => cl.push(' '),
                _ => {}
            }
            return;
        }
        if self.mode == Mode::Insert {
            if self.recording {
                self.insert_log.push(t.clone());
            }
            let mut c = Self::chars(text);
            match t {
                VKey::Esc => {
                    self.mode = Mode::Normal;
                    self.recording = false;
                    if let Some(bi) = self.block_ins.take() {
                        let typed = typed_text(&self.insert_log);
                        if !typed.is_empty() && !typed.contains('\n') {
                            let ins: Vec<char> = typed.chars().collect();
                            for &ln in bi.lines.iter().rev() {
                                if ln >= line_count(&c) {
                                    continue;
                                }
                                let s = line_start_n(&c, ln);
                                let e = le(&c, s);
                                let at = if bi.eol { e } else { s + bi.col };
                                if at > e {
                                    if !bi.pad {
                                        continue;
                                    }
                                    for _ in e..at {
                                        c.insert(e, ' ');
                                    }
                                }
                                for (k, ch) in ins.iter().enumerate() {
                                    c.insert(at + k, *ch);
                                }
                            }
                            Self::set_text(text, &c);
                            out.changed = true;
                        }
                    }
                    if self.pos > ls(&c, self.pos) {
                        self.pos -= 1;
                    }
                }
                VKey::Ch(ch) => {
                    c.insert(self.pos.min(c.len()), ch);
                    self.pos += 1;
                    out.changed = true;
                }
                VKey::Tab => {
                    for _ in 0..2 {
                        c.insert(self.pos.min(c.len()), ' ');
                        self.pos += 1;
                    }
                    out.changed = true;
                }
                VKey::Enter => {
                    let ind: String = c[ls(&c, self.pos)..first_nb(&c, self.pos)].iter().collect();
                    c.insert(self.pos.min(c.len()), '\n');
                    self.pos += 1;
                    for ch in ind.chars() {
                        c.insert(self.pos, ch);
                        self.pos += 1;
                    }
                    out.changed = true;
                }
                VKey::Back if self.pos > 0 => {
                    self.pos -= 1;
                    c.remove(self.pos);
                    out.changed = true;
                }
                VKey::Del if self.pos < c.len() => {
                    c.remove(self.pos);
                    out.changed = true;
                }
                VKey::Left => self.pos = self.pos.saturating_sub(1).max(ls(&c, self.pos)),
                VKey::Right => self.pos = (self.pos + 1).min(le(&c, self.pos)),
                _ => {}
            }
            if out.changed {
                Self::set_text(text, &c);
            }
            return;
        }
        self.pending.push(t);
        let before = self.pending.clone();
        let changed_before = out.changed;
        match self.try_exec(text, out) {
            Step::Wait => {}
            Step::Done => {
                if (out.changed && !changed_before || self.mode == Mode::Insert) && !self.replaying && is_change(&before) {
                    self.last_change = before;
                }
                self.pending.clear();
            }
            Step::Bad => self.pending.clear(),
        }
    }

    fn try_exec(&mut self, text: &mut String, out: &mut VimOut) -> Step {
        let toks = self.pending.clone();
        let mut i = 0;
        let mut count: Option<usize> = None;
        while i < toks.len() {
            match &toks[i] {
                VKey::Ch(d @ '1'..='9') => count = Some(count.unwrap_or(0) * 10 + d.to_digit(10).unwrap() as usize),
                VKey::Ch('0') if count.is_some() => count = Some(count.unwrap() * 10),
                _ => break,
            }
            i += 1;
        }
        if i >= toks.len() {
            return Step::Wait;
        }
        let n = count.unwrap_or(1);
        let c = Self::chars(text);
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine | Mode::VisualBlock);
        let k = toks[i].clone();
        if k == VKey::Ctrl('v') || k == VKey::Ctrl('q') {
            self.mode = if self.mode == Mode::VisualBlock { Mode::Normal } else { Mode::VisualBlock };
            self.anchor = self.pos;
            self.block_eol = false;
            return Step::Done;
        }
        if self.mode == Mode::VisualBlock {
            return self.block_cmd(text, &k, &toks[i + 1..], n, out);
        }
        let rest = &toks[i + 1..];

        // keys that need one more character
        if let VKey::Ch('r') = k {
            let Some(next) = rest.first() else { return Step::Wait };
            let Some(ch) = key_char(next).or(if *next == VKey::Enter { Some('\n') } else { None }) else { return Step::Bad };
            let mut c = c;
            if visual {
                let (a, b) = self.vrange(&c);
                for x in a..b {
                    if c[x] != '\n' {
                        c[x] = ch;
                    }
                }
                self.pos = a;
                self.mode = Mode::Normal;
            } else {
                let e = le(&c, self.pos);
                if self.pos + n > e {
                    return Step::Bad;
                }
                for x in self.pos..self.pos + n {
                    c[x] = ch;
                }
                self.pos += n - 1;
            }
            Self::set_text(text, &c);
            out.changed = true;
            return Step::Done;
        }
        if let VKey::Ch('z') = k {
            if rest.is_empty() {
                return Step::Wait;
            }
            out.scroll_center = true;
            return Step::Done;
        }

        match k {
            VKey::Esc | VKey::Ctrl('c') | VKey::Ctrl('[') => {
                self.mode = Mode::Normal;
                return Step::Done;
            }
            VKey::Ch(':') => {
                self.cmdline = Some(if visual { ":'<,'>".into() } else { ":".into() });
                self.mode = Mode::Normal;
                return Step::Done;
            }
            VKey::Ch('/') | VKey::Ch('?') => {
                self.cmdline = Some("/".into());
                return Step::Done;
            }
            VKey::Ch('u') if !visual => {
                out.undo += n;
                return Step::Done;
            }
            VKey::Ctrl('r') => {
                out.redo += n;
                return Step::Done;
            }
            VKey::Ch('v') => {
                self.mode = if self.mode == Mode::Visual { Mode::Normal } else { Mode::Visual };
                self.anchor = self.pos;
                return Step::Done;
            }
            VKey::Ch('V') => {
                self.mode = if self.mode == Mode::VisualLine { Mode::Normal } else { Mode::VisualLine };
                self.anchor = self.pos;
                return Step::Done;
            }
            VKey::Ch('.') if !visual => {
                let last = self.last_change.clone();
                let log = self.insert_log.clone();
                if last.is_empty() {
                    return Step::Done;
                }
                self.replaying = true;
                self.pending.clear();
                for _ in 0..n {
                    for t in &last {
                        self.feed(text, t.clone(), out);
                    }
                    if self.mode == Mode::Insert {
                        for t in &log {
                            self.feed(text, t.clone(), out);
                        }
                        if self.mode == Mode::Insert {
                            self.feed(text, VKey::Esc, out);
                        }
                    }
                }
                self.replaying = false;
                return Step::Done;
            }
            VKey::Ch('n') | VKey::Ch('N') if !visual || true => {
                if self.last_search.is_empty() {
                    return Step::Done;
                }
                let m = crate::editor::find_matches(text, &self.last_search);
                if m.is_empty() {
                    self.message = trf!("Nicht gefunden: {}" | "Not found: {}", self.last_search);
                    return Step::Done;
                }
                let fwd = k == VKey::Ch('n');
                for _ in 0..n {
                    self.pos = if fwd {
                        m.iter().find(|x| x.0 > self.pos).or(m.first()).unwrap().0
                    } else {
                        m.iter().rev().find(|x| x.0 < self.pos).or(m.last()).unwrap().0
                    };
                }
                out.search = Some(self.last_search.clone());
                return Step::Done;
            }
            VKey::Ctrl('d') | VKey::Ctrl('u') => {
                let lines = 15 * n;
                let cur = line_of(&c, self.pos);
                let target = if k == VKey::Ctrl('d') { (cur + lines).min(line_count(&c) - 1) } else { cur.saturating_sub(lines) };
                self.pos = first_nb(&c, line_start_n(&c, target));
                out.scroll_center = true;
                return Step::Done;
            }
            _ => {}
        }

        if visual {
            return self.visual_cmd(text, &k, rest, n, out);
        }

        // simple normal-mode commands
        let mut c = c;
        let simple = match k {
            VKey::Ch('i') => {
                self.enter_insert();
                true
            }
            VKey::Ch('a') => {
                if self.pos < le(&c, self.pos) {
                    self.pos += 1;
                }
                self.enter_insert();
                true
            }
            VKey::Ch('I') => {
                self.pos = first_nb(&c, self.pos);
                self.enter_insert();
                true
            }
            VKey::Ch('A') => {
                self.pos = le(&c, self.pos);
                self.enter_insert();
                true
            }
            VKey::Ch('o') | VKey::Ch('O') => {
                let ind: String = c[ls(&c, self.pos)..first_nb(&c, self.pos)].iter().collect();
                let at = if k == VKey::Ch('o') { le(&c, self.pos) } else { ls(&c, self.pos) };
                let ins: Vec<char> = if k == VKey::Ch('o') { format!("\n{ind}").chars().collect() } else { format!("{ind}\n").chars().collect() };
                for (j, ch) in ins.iter().enumerate() {
                    c.insert(at + j, *ch);
                }
                self.pos = if k == VKey::Ch('o') { at + ins.len() } else { at + ind.chars().count() };
                Self::set_text(text, &c);
                out.changed = true;
                self.enter_insert();
                true
            }
            VKey::Ch('x') | VKey::Del => {
                let e = le(&c, self.pos);
                let b = (self.pos + n).min(e);
                if b > self.pos {
                    let s: String = c[self.pos..b].iter().collect();
                    self.yank(s, false, out);
                    c.drain(self.pos..b);
                    Self::set_text(text, &c);
                    out.changed = true;
                }
                true
            }
            VKey::Ch('X') => {
                let s = ls(&c, self.pos);
                let a = self.pos.saturating_sub(n).max(s);
                if a < self.pos {
                    let y: String = c[a..self.pos].iter().collect();
                    self.yank(y, false, out);
                    c.drain(a..self.pos);
                    self.pos = a;
                    Self::set_text(text, &c);
                    out.changed = true;
                }
                true
            }
            VKey::Ch('p') | VKey::Ch('P') if self.reg_block => {
                let col = self.pos - ls(&c, self.pos) + if k == VKey::Ch('p') && self.pos < le(&c, self.pos) { 1 } else { 0 };
                let start_line = line_of(&c, self.pos);
                let parts: Vec<String> = self.register.split('\n').map(String::from).collect();
                for (i, part) in parts.iter().enumerate() {
                    let ln = start_line + i;
                    while ln >= line_count(&c) {
                        c.push('\n');
                    }
                    let s = line_start_n(&c, ln);
                    let e = le(&c, s);
                    if s + col > e {
                        for _ in e..s + col {
                            c.insert(e, ' ');
                        }
                    }
                    for (k2, ch) in part.chars().enumerate() {
                        c.insert(s + col + k2, ch);
                    }
                }
                Self::set_text(text, &c);
                self.pos = ls(&c, self.pos) + col;
                out.changed = true;
                true
            }
            VKey::Ch('p') | VKey::Ch('P') => {
                let reg: Vec<char> = self.register.repeat(n).chars().collect();
                if reg.is_empty() {
                    return Step::Done;
                }
                if self.reg_line {
                    let mut ins = reg.clone();
                    if ins.last() != Some(&'\n') {
                        ins.push('\n');
                    }
                    let at = if k == VKey::Ch('p') {
                        let e = le(&c, self.pos);
                        if e >= c.len() {
                            c.push('\n');
                            ins.pop();
                            c.len()
                        } else {
                            e + 1
                        }
                    } else {
                        ls(&c, self.pos)
                    };
                    for (j, ch) in ins.iter().enumerate() {
                        c.insert(at + j, *ch);
                    }
                    self.pos = at;
                    Self::set_text(text, &c);
                    self.pos = first_nb(&Self::chars(text), at);
                } else {
                    let at = if k == VKey::Ch('p') && self.pos < le(&c, self.pos) { self.pos + 1 } else { self.pos };
                    for (j, ch) in reg.iter().enumerate() {
                        c.insert(at + j, *ch);
                    }
                    self.pos = at + reg.len() - 1;
                    Self::set_text(text, &c);
                }
                out.changed = true;
                true
            }
            VKey::Ch('J') => {
                for _ in 0..n.max(1) {
                    let e = le(&c, self.pos);
                    if e >= c.len() {
                        break;
                    }
                    let mut j = e + 1;
                    while j < c.len() && (c[j] == ' ' || c[j] == '\t') {
                        j += 1;
                    }
                    let join_space = j < c.len() && c[j] != '\n' && c[j] != ')' && e > ls(&c, e);
                    c.drain(e..j);
                    if join_space {
                        c.insert(e, ' ');
                    }
                    self.pos = e;
                }
                Self::set_text(text, &c);
                out.changed = true;
                true
            }
            VKey::Ch('~') => {
                let e = le(&c, self.pos);
                for _ in 0..n {
                    if self.pos >= e {
                        break;
                    }
                    let ch = c[self.pos];
                    c[self.pos] = if ch.is_uppercase() { ch.to_lowercase().next().unwrap_or(ch) } else { ch.to_uppercase().next().unwrap_or(ch) };
                    self.pos += 1;
                }
                Self::set_text(text, &c);
                out.changed = true;
                true
            }
            VKey::Ch('D') => return self.operate(text, 'd', Mv { to: le(&c, self.pos), line: false, incl: false }, out),
            VKey::Ch('C') => return self.operate(text, 'c', Mv { to: le(&c, self.pos), line: false, incl: false }, out),
            VKey::Ch('s') => return self.operate(text, 'c', Mv { to: (self.pos + n).min(le(&c, self.pos)), line: false, incl: false }, out),
            VKey::Ch('S') => {
                let to = line_start_n(&c, (line_of(&c, self.pos) + n - 1).min(line_count(&c) - 1));
                return self.operate(text, 'c', Mv { to, line: true, incl: true }, out);
            }
            VKey::Ch('Y') => {
                let to = line_start_n(&c, (line_of(&c, self.pos) + n - 1).min(line_count(&c) - 1));
                return self.operate(text, 'y', Mv { to, line: true, incl: true }, out);
            }
            _ => false,
        };
        if simple {
            return Step::Done;
        }

        // operators
        if let VKey::Ch(op @ ('d' | 'c' | 'y' | '>' | '<')) = k {
            let mut j = 0;
            let mut c2: Option<usize> = None;
            while j < rest.len() {
                match &rest[j] {
                    VKey::Ch(d @ '1'..='9') => c2 = Some(c2.unwrap_or(0) * 10 + d.to_digit(10).unwrap() as usize),
                    VKey::Ch('0') if c2.is_some() => c2 = Some(c2.unwrap() * 10),
                    _ => break,
                }
                j += 1;
            }
            if j >= rest.len() {
                return Step::Wait;
            }
            let total = n * c2.unwrap_or(1);
            if rest[j] == VKey::Ch(op) {
                // dd, cc, yy, >>, <<
                let to = line_start_n(&c, (line_of(&c, self.pos) + total - 1).min(line_count(&c) - 1));
                return self.operate(text, op, Mv { to, line: true, incl: true }, out);
            }
            return match self.motion(&c, &rest[j..], total, Some(op)) {
                MRes::Wait => Step::Wait,
                MRes::Bad => Step::Bad,
                MRes::To(mut mv) => {
                    // cw behaves like ce
                    if op == 'c' && matches!(rest[j], VKey::Ch('w') | VKey::Ch('W')) && self.pos < c.len() && !c[self.pos].is_whitespace() {
                        let big = rest[j] == VKey::Ch('W');
                        let mut e = self.pos;
                        for _ in 0..total {
                            e = word_end(&c, if e == self.pos { e.saturating_sub(0) } else { e }, big);
                        }
                        if self.pos < c.len() && cls(c[self.pos], big) != Cls::Space && (self.pos + 1 >= c.len() || cls(c[self.pos + 1], big) != cls(c[self.pos], big)) && total == 1 {
                            e = self.pos;
                        }
                        mv = Mv { to: e, line: false, incl: true };
                    }
                    // dw never crosses the end of the line
                    if op != 'c' && matches!(rest[j], VKey::Ch('w') | VKey::Ch('W')) && !mv.line {
                        let e = le(&c, self.pos);
                        if mv.to > e && e > self.pos {
                            mv.to = e;
                        }
                    }
                    self.operate(text, op, mv, out)
                }
                MRes::Range(a, b, line) => self.operate_range(text, op, a, b, line, out),
            };
        }

        // plain motion
        match self.motion(&c, &toks[i..], n, None) {
            MRes::Wait => Step::Wait,
            MRes::Bad => Step::Bad,
            MRes::To(mv) => {
                let vertical = matches!(toks[i], VKey::Ch('j') | VKey::Ch('k') | VKey::Down | VKey::Up);
                if !vertical {
                    self.want_col = None;
                }
                self.pos = mv.to;
                Step::Done
            }
            MRes::Range(..) => Step::Bad,
        }
    }

    fn vrange(&self, c: &[char]) -> (usize, usize) {
        let (lo, hi) = (self.anchor.min(self.pos), self.anchor.max(self.pos));
        if self.mode == Mode::VisualLine {
            let e = le(c, hi);
            (ls(c, lo), if e < c.len() { e + 1 } else { e })
        } else {
            (lo, (hi + 1).min(c.len()))
        }
    }

    fn visual_cmd(&mut self, text: &mut String, k: &VKey, rest: &[VKey], n: usize, out: &mut VimOut) -> Step {
        let c = Self::chars(text);
        let line = self.mode == Mode::VisualLine;
        let (a, b) = self.vrange(&c);
        let op = match k {
            VKey::Ch('d') | VKey::Ch('x') | VKey::Del => Some('d'),
            VKey::Ch('y') => Some('y'),
            VKey::Ch('c') | VKey::Ch('s') => Some('c'),
            VKey::Ch('>') => Some('>'),
            VKey::Ch('<') => Some('<'),
            VKey::Ch('~') | VKey::Ch('u') | VKey::Ch('U') => {
                let mut c = c;
                for x in a..b {
                    let ch = c[x];
                    c[x] = match k {
                        VKey::Ch('u') => ch.to_lowercase().next().unwrap_or(ch),
                        VKey::Ch('U') => ch.to_uppercase().next().unwrap_or(ch),
                        _ if ch.is_uppercase() => ch.to_lowercase().next().unwrap_or(ch),
                        _ => ch.to_uppercase().next().unwrap_or(ch),
                    };
                }
                Self::set_text(text, &c);
                self.pos = a;
                self.mode = Mode::Normal;
                out.changed = true;
                return Step::Done;
            }
            VKey::Ch('o') => {
                std::mem::swap(&mut self.anchor, &mut self.pos);
                return Step::Done;
            }
            VKey::Ch('p') | VKey::Ch('P') => {
                let reg: Vec<char> = self.register.chars().collect();
                let mut c = c;
                let old: String = c[a..b].iter().collect();
                c.splice(a..b, reg.iter().cloned());
                Self::set_text(text, &c);
                self.yank(old, line, out);
                self.pos = a;
                self.mode = Mode::Normal;
                out.changed = true;
                return Step::Done;
            }
            VKey::Ch('J') => {
                self.mode = Mode::Normal;
                self.pos = a;
                let lines = line_of(&c, b.saturating_sub(1)) - line_of(&c, a);
                self.pending.clear();
                for _ in 0..lines.max(1) {
                    self.feed(text, VKey::Ch('J'), out);
                }
                return Step::Done;
            }
            _ => None,
        };
        if let Some(op) = op {
            let r = self.operate_range(text, op, a, b, line, out);
            if self.mode != Mode::Insert {
                self.mode = Mode::Normal;
            }
            return r;
        }
        // text objects extend the selection
        if matches!(k, VKey::Ch('i') | VKey::Ch('a')) {
            let mut seq = vec![k.clone()];
            seq.extend_from_slice(rest);
            return match self.motion(&c, &seq, n, Some('v')) {
                MRes::Wait => Step::Wait,
                MRes::Range(x, y, _) => {
                    self.anchor = x;
                    self.pos = y.saturating_sub(1).max(x);
                    Step::Done
                }
                _ => Step::Bad,
            };
        }
        let mut seq = vec![k.clone()];
        seq.extend_from_slice(rest);
        match self.motion(&c, &seq, n, None) {
            MRes::Wait => Step::Wait,
            MRes::To(mv) => {
                self.pos = mv.to;
                Step::Done
            }
            _ => Step::Bad,
        }
    }

    fn motion(&mut self, c: &[char], toks: &[VKey], n: usize, op: Option<char>) -> MRes {
        let Some(k) = toks.first() else { return MRes::Wait };
        let p = self.pos;
        let len = c.len();
        let excl = |to| MRes::To(Mv { to, line: false, incl: false });
        let incl = |to| MRes::To(Mv { to, line: false, incl: true });
        match k {
            VKey::Ch('h') | VKey::Left | VKey::Back => excl(p.saturating_sub(n).max(ls(c, p))),
            VKey::Ch('l') | VKey::Right | VKey::Ch(' ') => {
                let e = le(c, p);
                let lim = if op.is_some() { e } else { e.saturating_sub(1).max(ls(c, p)) };
                excl((p + n).min(lim))
            }
            VKey::Ch('j') | VKey::Down | VKey::Ch('k') | VKey::Up | VKey::Enter | VKey::Ch('+') | VKey::Ch('-') => {
                let down = matches!(k, VKey::Ch('j') | VKey::Down | VKey::Enter | VKey::Ch('+'));
                let cur = line_of(c, p);
                let target = if down { (cur + n).min(line_count(c) - 1) } else { cur.saturating_sub(n) };
                if target == cur && op.is_none() {
                    return MRes::To(Mv { to: p, line: true, incl: true });
                }
                let col = self.want_col.unwrap_or(p - ls(c, p));
                if op.is_none() {
                    self.want_col = Some(col);
                }
                let s = line_start_n(c, target);
                let to = if matches!(k, VKey::Enter | VKey::Ch('+') | VKey::Ch('-')) { first_nb(c, s) } else { (s + col).min(le(c, s).saturating_sub(if le(c, s) > s { 1 } else { 0 })).max(s) };
                MRes::To(Mv { to, line: true, incl: true })
            }
            VKey::Ch(w @ ('w' | 'W')) => {
                let mut q = p;
                for _ in 0..n {
                    q = word_fwd(c, q, *w == 'W');
                }
                excl(q)
            }
            VKey::Ch(b @ ('b' | 'B')) => {
                let mut q = p;
                for _ in 0..n {
                    q = word_back(c, q, *b == 'B');
                }
                excl(q)
            }
            VKey::Ch(e @ ('e' | 'E')) => {
                let mut q = p;
                for _ in 0..n {
                    q = word_end(c, q, *e == 'E');
                }
                incl(q)
            }
            VKey::Ch('0') | VKey::Home => excl(ls(c, p)),
            VKey::Ch('^') | VKey::Ch('_') => excl(first_nb(c, p)),
            VKey::Ch('$') | VKey::End => {
                let line = (line_of(c, p) + n - 1).min(line_count(c) - 1);
                let e = le(c, line_start_n(c, line));
                if op.is_some() {
                    excl(e)
                } else {
                    MRes::To(Mv { to: e.saturating_sub(1).max(ls(c, e)), line: false, incl: true })
                }
            }
            VKey::Ch('G') => {
                let line = if toks_count_explicit(n) { n.saturating_sub(1) } else { line_count(c) - 1 };
                let s = line_start_n(c, line.min(line_count(c) - 1));
                MRes::To(Mv { to: first_nb(c, s), line: true, incl: true })
            }
            VKey::Ch('g') => match toks.get(1) {
                None => MRes::Wait,
                Some(VKey::Ch('g')) => {
                    let line = if n > 1 { n - 1 } else { 0 };
                    let s = line_start_n(c, line.min(line_count(c) - 1));
                    MRes::To(Mv { to: first_nb(c, s), line: true, incl: true })
                }
                Some(VKey::Ch('e')) => {
                    let mut q = p;
                    for _ in 0..n {
                        q = q.saturating_sub(1);
                        while q > 0 && c[q].is_whitespace() {
                            q -= 1;
                        }
                        let k0 = cls(c[q], false);
                        while q > 0 && cls(c[q - 1], false) == k0 {
                            q -= 1;
                        }
                        q = q.saturating_sub(1);
                    }
                    incl(q)
                }
                _ => MRes::Bad,
            },
            VKey::Ch(f @ ('f' | 'F' | 't' | 'T')) => {
                let Some(t2) = toks.get(1) else { return MRes::Wait };
                let Some(ch) = key_char(t2) else { return MRes::Bad };
                self.last_find = Some((*f, ch));
                self.find_char(c, *f, ch, n, op.is_some())
            }
            VKey::Ch(';') | VKey::Ch(',') => {
                let Some((f, ch)) = self.last_find else { return MRes::Bad };
                let f = if *k == VKey::Ch(',') {
                    match f {
                        'f' => 'F',
                        'F' => 'f',
                        't' => 'T',
                        _ => 't',
                    }
                } else {
                    f
                };
                self.find_char(c, f, ch, n, op.is_some())
            }
            VKey::Ch('%') => match match_pair(c, p) {
                Some(q) => incl(q),
                None => MRes::Bad,
            },
            VKey::Ch('}') | VKey::Ch('{') => {
                let fwd = *k == VKey::Ch('}');
                let mut q = p;
                for _ in 0..n {
                    if fwd {
                        q = le(c, q);
                        while q < len && c.get(q + 1) == Some(&'\n') {
                            q += 1;
                        }
                        while q < len {
                            let s = q + 1;
                            if s >= len {
                                q = len;
                                break;
                            }
                            if c[s] == '\n' {
                                q = s;
                                break;
                            }
                            q = le(c, s);
                        }
                    } else {
                        q = ls(c, q);
                        while q > 0 && q >= 2 && c[q - 2] == '\n' {
                            q -= 1;
                        }
                        loop {
                            if q == 0 {
                                break;
                            }
                            let s = ls(c, q - 1);
                            if s == q - 1 {
                                q = s;
                                break;
                            }
                            q = s;
                        }
                    }
                }
                excl(q.min(len))
            }
            VKey::Ch(io @ ('i' | 'a')) if op.is_some() => {
                let Some(t2) = toks.get(1) else { return MRes::Wait };
                let inner = *io == 'i';
                match t2 {
                    VKey::Ch('w') | VKey::Ch('W') => {
                        if p >= len {
                            return MRes::Bad;
                        }
                        let big = *t2 == VKey::Ch('W');
                        let k0 = cls(c[p], big);
                        let mut a = p;
                        while a > 0 && cls(c[a - 1], big) == k0 && c[a - 1] != '\n' {
                            a -= 1;
                        }
                        let mut b = p;
                        while b < len && cls(c[b], big) == k0 && c[b] != '\n' {
                            b += 1;
                        }
                        if !inner {
                            while b < len && (c[b] == ' ' || c[b] == '\t') {
                                b += 1;
                            }
                        }
                        MRes::Range(a, b, false)
                    }
                    VKey::Ch(o) => {
                        let pair = match o {
                            '(' | ')' | 'b' => Some(('(', ')')),
                            '{' | '}' | 'B' => Some(('{', '}')),
                            '[' | ']' => Some(('[', ']')),
                            '<' | '>' => Some(('<', '>')),
                            _ => None,
                        };
                        if let Some((op_, cl)) = pair {
                            match enclosing(c, p, op_, cl) {
                                Some((a, b)) => {
                                    if inner {
                                        MRes::Range(a + 1, b, false)
                                    } else {
                                        MRes::Range(a, b + 1, false)
                                    }
                                }
                                None => MRes::Bad,
                            }
                        } else if matches!(o, '"' | '\'' | '`' | '$') {
                            match quote_pair(c, p, *o) {
                                Some((a, b)) => {
                                    if inner {
                                        MRes::Range(a + 1, b, false)
                                    } else {
                                        MRes::Range(a, b + 1, false)
                                    }
                                }
                                None => MRes::Bad,
                            }
                        } else if *o == 'p' {
                            // paragraph
                            let mut a = ls(c, p);
                            while a > 0 && !(a >= 2 && c[a - 2] == '\n' && c[a - 1] == '\n') {
                                a = ls(c, a - 1);
                            }
                            let mut b = le(c, p);
                            while b < len && !(b + 1 < len && c[b + 1] == '\n') {
                                b = le(c, b + 1);
                            }
                            let b = (b + 1).min(len);
                            MRes::Range(a, if inner { b } else { (b + 1).min(len) }, true)
                        } else {
                            MRes::Bad
                        }
                    }
                    _ => MRes::Bad,
                }
            }
            _ => MRes::Bad,
        }
    }

    fn find_char(&self, c: &[char], f: char, ch: char, n: usize, for_op: bool) -> MRes {
        let p = self.pos;
        let s = ls(c, p);
        let e = le(c, p);
        let mut q = p;
        for k in 0..n {
            match f {
                'f' | 't' => {
                    let start = if f == 't' && k == 0 { q + 2 } else { q + 1 };
                    match (start.min(e)..e).find(|&i| c[i] == ch) {
                        Some(i) => q = if f == 't' { i - 1 } else { i },
                        None => return MRes::Bad,
                    }
                }
                _ => {
                    let end = if f == 'T' && k == 0 { q.saturating_sub(1) } else { q };
                    match (s..end).rev().find(|&i| c[i] == ch) {
                        Some(i) => q = if f == 'T' { i + 1 } else { i },
                        None => return MRes::Bad,
                    }
                }
            }
        }
        let _ = for_op;
        if matches!(f, 'f' | 't') {
            MRes::To(Mv { to: q, line: false, incl: true })
        } else {
            MRes::To(Mv { to: q, line: false, incl: false })
        }
    }

    fn operate(&mut self, text: &mut String, op: char, mv: Mv, out: &mut VimOut) -> Step {
        let c = Self::chars(text);
        let (mut a, mut b) = if mv.to >= self.pos { (self.pos, mv.to) } else { (mv.to, self.pos) };
        if mv.incl && !mv.line {
            b = (b + 1).min(c.len());
        }
        if mv.line {
            a = ls(&c, a);
            let e = le(&c, b);
            b = if e < c.len() { e + 1 } else { e };
        }
        let _ = &mut a;
        self.operate_range(text, op, a, b, mv.line, out)
    }

    fn operate_range(&mut self, text: &mut String, op: char, a: usize, b: usize, line: bool, out: &mut VimOut) -> Step {
        let mut c = Self::chars(text);
        let (a, b) = (a.min(c.len()), b.min(c.len()));
        match op {
            'y' | 'v' => {
                let s: String = c[a..b].iter().collect();
                self.yank(s, line, out);
                self.pos = a;
            }
            'd' => {
                let mut a = a;
                // deleting the last line(s) – including an empty last line after a
                // trailing newline – removes the newline before them, like Vim
                if line && b == c.len() && a > 0 && (b == a || c[b - 1] != '\n') {
                    a -= 1;
                }
                let s: String = c[a..b].iter().collect();
                let s = if line && !s.ends_with('\n') { format!("{}\n", s.trim_start_matches('\n')) } else { s };
                self.yank(s, line, out);
                c.drain(a..b);
                Self::set_text(text, &c);
                self.pos = if line { first_nb(&c, a.min(c.len())) } else { a };
                out.changed = true;
            }
            'c' => {
                let b = if line && b > a && c.get(b - 1) == Some(&'\n') { b - 1 } else { b };
                let (a, keep_indent) = if line { (first_nb(&c, a), true) } else { (a, false) };
                let _ = keep_indent;
                let s: String = c[a..b].iter().collect();
                self.yank(s, line, out);
                c.drain(a..b);
                Self::set_text(text, &c);
                self.pos = a;
                out.changed = true;
                self.enter_insert();
            }
            '>' | '<' => {
                let first = line_of(&c, a);
                let last = line_of(&c, b.saturating_sub(1).max(a));
                for ln in (first..=last).rev() {
                    let s = line_start_n(&c, ln);
                    if op == '>' {
                        if le(&c, s) > s {
                            c.insert(s, ' ');
                            c.insert(s, ' ');
                        }
                    } else {
                        let mut k = 0;
                        while k < 2 && s + k < c.len() && c[s + k] == ' ' {
                            k += 1;
                        }
                        c.drain(s..s + k);
                    }
                }
                Self::set_text(text, &c);
                self.pos = first_nb(&c, line_start_n(&c, first));
                out.changed = true;
            }
            _ => return Step::Bad,
        }
        Step::Done
    }

    fn ex(&mut self, text: &mut String, cmd: &str, out: &mut VimOut) {
        if let Some(pat) = cmd.strip_prefix('/') {
            if !pat.is_empty() {
                self.last_search = pat.to_string();
            }
            let m = crate::editor::find_matches(text, &self.last_search);
            match m.iter().find(|x| x.0 > self.pos).or(m.first()) {
                Some(x) => {
                    self.pos = x.0;
                    out.search = Some(self.last_search.clone());
                }
                None => self.message = format!("Nicht gefunden: {}", self.last_search),
            }
            return;
        }
        let c = cmd.trim_start_matches(':').trim();
        let c = c.strip_prefix("'<,'>").unwrap_or(c);
        match c {
            "w" | "write" => out.save = true,
            "q" | "quit" | "q!" => out.close = true,
            "wq" | "x" | "wq!" => {
                out.save = true;
                out.close = true;
            }
            "noh" | "nohlsearch" => out.noh = true,
            "" => {}
            _ if c.chars().all(|ch| ch.is_ascii_digit()) => {
                let n: usize = c.parse().unwrap_or(1);
                let chars = Self::chars(text);
                self.pos = first_nb(&chars, line_start_n(&chars, n.saturating_sub(1).min(line_count(&chars) - 1)));
                out.scroll_center = true;
            }
            _ if c.starts_with("s/") || c.starts_with("%s/") => {
                let whole = c.starts_with('%');
                let body = &c[if whole { 3 } else { 2 }..];
                let parts: Vec<&str> = split_unescaped(body);
                if parts.len() < 2 {
                    self.message = "Ungültig: :s/alt/neu/[g]".into();
                    return;
                }
                let (pat, rep, flags) = (parts[0], parts[1], parts.get(2).copied().unwrap_or(""));
                let re = match regex::Regex::new(pat) {
                    Ok(r) => r,
                    Err(_) => regex::Regex::new(&regex::escape(pat)).unwrap(),
                };
                let rep = rep.replace("\\/", "/").replace('&', "$0");
                let chars = Self::chars(text);
                let (a, b) = if whole { (0, chars.len()) } else { (ls(&chars, self.pos), le(&chars, self.pos)) };
                let ab = crate::editor::char_to_byte(text, a);
                let bb = crate::editor::char_to_byte(text, b);
                let seg = &text[ab..bb];
                let mut count = 0;
                let new_seg = if whole || flags.contains('g') {
                    // whole file: substitute per line (first match per line unless g)
                    seg.split('\n')
                        .map(|l| {
                            count += re.find_iter(l).count().min(if flags.contains('g') { usize::MAX } else { 1 });
                            if flags.contains('g') { re.replace_all(l, rep.as_str()).to_string() } else { re.replace(l, rep.as_str()).to_string() }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    count = re.find(seg).map_or(0, |_| 1);
                    re.replace(seg, rep.as_str()).to_string()
                };
                if count > 0 {
                    text.replace_range(ab..bb, &new_seg);
                    out.changed = true;
                    self.message = format!("{count} Ersetzung(en)");
                } else {
                    self.message = format!("Muster nicht gefunden: {pat}");
                }
            }
            _ => self.message = format!("Unbekannter Befehl: :{c}"),
        }
    }

    // ───────────────────────────── egui adapter ─────────────────────────────

    /// Translate egui input for the focused buffer. Returns actions for the caller.
    pub fn handle(&mut self, ctx: &egui::Context, buf: &mut crate::editor::Buffer, completion_open: bool) -> VimOut {
        use egui::{Event, Key};
        if self.buf_id != Some(buf.id) {
            self.buf_id = Some(buf.id);
            self.mode = Mode::Normal;
            self.pos = buf.cursor.min(buf.sel_end);
            self.last_set = None;
            self.pending.clear();
            self.cmdline = None;
        }
        // adopt cursor changes made with the mouse / by TextEdit
        let sel = (buf.cursor.min(buf.sel_end), buf.cursor.max(buf.sel_end));
        if self.last_set != Some(sel) {
            if self.mode == Mode::Insert {
                self.pos = buf.cursor;
            } else if sel.1 > sel.0 {
                self.mode = Mode::Visual;
                self.anchor = buf.sel_end;
                self.pos = buf.cursor.saturating_sub(if buf.cursor > buf.sel_end { 1 } else { 0 });
            } else {
                if matches!(self.mode, Mode::Visual | Mode::VisualLine | Mode::VisualBlock) {
                    self.mode = Mode::Normal;
                }
                self.pos = buf.cursor.min(buf.sel_end);
            }
        }

        let mut toks: Vec<VKey> = vec![];
        let consume_all = self.mode != Mode::Insert || self.cmdline.is_some();
        ctx.input_mut(|i| {
            let evs = std::mem::take(&mut i.events);
            let mut keep = vec![];
            for e in evs {
                let tok = match &e {
                    Event::Text(t) => {
                        if consume_all {
                            for ch in t.chars() {
                                toks.push(VKey::Ch(ch));
                            }
                            continue;
                        }
                        // insert mode: TextEdit inserts the text; we only log it for `.`
                        if self.recording {
                            for ch in t.chars() {
                                self.insert_log.push(VKey::Ch(ch));
                            }
                        }
                        None
                    }
                    Event::Key { key, pressed: true, modifiers, .. } => {
                        let ctrl = modifiers.ctrl || modifiers.command;
                        let t = match key {
                            Key::Escape => Some(VKey::Esc),
                            Key::Enter => Some(VKey::Enter),
                            Key::Backspace => Some(VKey::Back),
                            Key::Delete => Some(VKey::Del),
                            Key::Tab => Some(VKey::Tab),
                            Key::ArrowUp => Some(VKey::Up),
                            Key::ArrowDown => Some(VKey::Down),
                            Key::ArrowLeft => Some(VKey::Left),
                            Key::ArrowRight => Some(VKey::Right),
                            Key::Home => Some(VKey::Home),
                            Key::End => Some(VKey::End),
                            Key::OpenBracket if ctrl => Some(VKey::Ctrl('[')),
                            k if ctrl => k.symbol_or_name().chars().next().filter(|c| c.is_ascii_alphabetic()).map(|c| VKey::Ctrl(c.to_ascii_lowercase())),
                            _ => None,
                        };
                        if consume_all {
                            if let Some(t) = t {
                                toks.push(t);
                            }
                            continue;
                        }
                        // insert mode: only Esc / Ctrl-[ / Ctrl-c leave it
                        match t {
                            Some(VKey::Esc) | Some(VKey::Ctrl('[')) | Some(VKey::Ctrl('c')) if !completion_open => {
                                toks.push(VKey::Esc);
                                continue;
                            }
                            Some(VKey::Enter) if self.recording && !completion_open => self.insert_log.push(VKey::Enter),
                            Some(VKey::Back) if self.recording => self.insert_log.push(VKey::Back),
                            _ => {}
                        }
                        None
                    }
                    Event::Key { pressed: false, .. } if consume_all => continue,
                    // egui-winit turns Ctrl+V / Ctrl+C into clipboard events – in Normal/Visual
                    // mode they mean visual-block and "escape" like in Vim
                    Event::Paste(_) if consume_all => {
                        toks.push(VKey::Ctrl('v'));
                        continue;
                    }
                    Event::Copy if consume_all => {
                        toks.push(VKey::Ctrl('c'));
                        continue;
                    }
                    Event::Cut if consume_all => continue,
                    _ => None,
                };
                let _: Option<()> = tok;
                keep.push(e);
            }
            i.events = keep;
        });

        let mut out = VimOut::default();
        if !toks.is_empty() {
            // Esc from insert mode: TextEdit owns the cursor while inserting
            if self.mode == Mode::Insert {
                self.pos = buf.cursor;
                self.recording = false;
            }
            self.message.clear();
            out = self.process(&mut buf.text, &toks);
        }
        if self.mode == Mode::Insert && !toks.is_empty() && self.cmdline.is_none() {
            // just entered insert: keep recording what TextEdit inserts
        }
        // reflect state in the TextEdit selection (block cursor in Normal mode)
        let c: Vec<char> = buf.text.chars().collect();
        self.pos = self.pos.min(c.len());
        let want = match self.mode {
            Mode::Insert => (self.pos, self.pos),
            Mode::Normal => (self.pos, self.pos),
            Mode::Visual | Mode::VisualLine => {
                let (a, b) = self.vrange(&c);
                (a, b)
            }
            Mode::VisualBlock => (self.pos, self.pos),
        };
        if self.last_set != Some(want) || !toks.is_empty() {
            if self.mode == Mode::Insert && toks.is_empty() {
                // don't fight TextEdit while typing
            } else {
                if self.pos < want.1 && matches!(self.mode, Mode::Visual | Mode::VisualLine) && self.pos < self.anchor {
                    buf.select(want.1, want.0);
                } else {
                    buf.select(want.0, want.1);
                }
            }
            self.last_set = Some(want);
        }
        if self.mode == Mode::Insert {
            self.last_set = Some((buf.cursor.min(buf.sel_end), buf.cursor.max(buf.sel_end)));
            if !toks.is_empty() {
                self.last_set = Some(want);
            }
        }
        out
    }

    /// Where to draw the block cursor (None = at the live TextEdit cursor, i.e. Insert).
    pub fn block_pos(&self) -> Option<usize> {
        match self.mode {
            Mode::Insert => None,
            _ => Some(self.pos),
        }
    }

    /// Text for the status bar: mode, pending keys, message.
    pub fn status(&self) -> (String, String) {
        let pend: String = self
            .pending
            .iter()
            .map(|k| match k {
                VKey::Ch(c) => c.to_string(),
                VKey::Ctrl(c) => format!("^{c}"),
                _ => String::new(),
            })
            .collect();
        (self.mode.label().to_string(), if self.message.is_empty() { pend } else { self.message.clone() })
    }
}

/// Text typed during an insert session (for replaying on other block lines).
fn typed_text(log: &[VKey]) -> String {
    let mut s = String::new();
    for k in log {
        match k {
            VKey::Ch(c) => s.push(*c),
            VKey::Tab => s.push_str("  "),
            VKey::Back => {
                s.pop();
            }
            VKey::Enter => s.push('\n'),
            _ => {}
        }
    }
    s
}

fn toks_count_explicit(n: usize) -> bool {
    n > 1
}

fn split_unescaped(s: &str) -> Vec<&str> {
    let mut parts = vec![];
    let b = s.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == b'/' {
            parts.push(&s[start..i]);
            start = i + 1;
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

fn is_change(toks: &[VKey]) -> bool {
    let first = toks.iter().find(|k| !matches!(k, VKey::Ch('0'..='9')));
    matches!(
        first,
        Some(VKey::Ch('d' | 'c' | 'x' | 'X' | 'p' | 'P' | 'r' | 'J' | '~' | '>' | '<' | 's' | 'S' | 'D' | 'C' | 'o' | 'O' | 'i' | 'a' | 'I' | 'A')) | Some(VKey::Del)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &str) -> Vec<VKey> {
        let mut v = vec![];
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '<' {
                let name: String = it.by_ref().take_while(|&c| c != '>').collect();
                v.push(match name.as_str() {
                    "esc" => VKey::Esc,
                    "cr" => VKey::Enter,
                    "bs" => VKey::Back,
                    n if n.starts_with("c-") => VKey::Ctrl(n.chars().nth(2).unwrap()),
                    _ => VKey::Ch('<'),
                });
            } else {
                v.push(VKey::Ch(c));
            }
        }
        v
    }

    fn run(text: &str, pos: usize, k: &str) -> (String, usize, VimState) {
        let mut v = VimState { pos, ..Default::default() };
        let mut t = text.to_string();
        v.process(&mut t, &keys(k));
        let p = v.pos;
        (t, p, v)
    }

    #[test]
    fn motions_and_deletes() {
        assert_eq!(run("hello world foo", 0, "dw").0, "world foo");
        assert_eq!(run("hello world foo", 0, "2dw").0, "foo");
        assert_eq!(run("hello world", 0, "de").0, " world");
        assert_eq!(run("hello world", 0, "d$").0, "");
        assert_eq!(run("abc", 0, "3x").0, "");
        assert_eq!(run("a\nb\nc", 2, "dd").0, "a\nc");
        assert_eq!(run("a\nb\nc", 4, "dd").0, "a\nb");
        assert_eq!(run("a\nb\nc", 0, "2dd").0, "c");
        assert_eq!(run("one two", 0, "w").1, 4);
        assert_eq!(run("one two", 6, "b").1, 4);
        assert_eq!(run("l1\nl2\nl3", 0, "G").1, 6);
        assert_eq!(run("l1\nl2\nl3", 6, "gg").1, 0);
        assert_eq!(run("l1\nl2\nl3", 0, "2G").1, 3);
        assert_eq!(run("a(b)c", 1, "%").1, 3);
    }

    #[test]
    fn change_and_text_objects() {
        assert_eq!(run("\\textbf{alt}", 9, "ci{neu<esc>").0, "\\textbf{neu}");
        assert_eq!(run("x $a+b$ y", 3, "di$").0, "x $$ y");
        assert_eq!(run("say \"hi there\" ok", 7, "ci\"yo<esc>").0, "say \"yo\" ok");
        assert_eq!(run("foo bar", 0, "cwbaz<esc>").0, "baz bar");
        assert_eq!(run("foo bar baz", 4, "diw").0, "foo  baz");
        assert_eq!(run("foo bar baz", 4, "daw").0, "foo baz");
        assert_eq!(run("line", 0, "ccnew<esc>").0, "new");
    }

    #[test]
    fn yank_put_and_repeat() {
        assert_eq!(run("a\nb", 0, "yyp").0, "a\na\nb");
        assert_eq!(run("a\nb", 2, "yyP").0, "a\nb\nb");
        assert_eq!(run("abc", 0, "xp").0, "bac");
        assert_eq!(run("a b c d", 0, "dw.").0, "c d");
        assert_eq!(run("x", 0, "ihi <esc>").0, "hi x");
        assert_eq!(run("a\nb", 0, "Ax<esc>j.").0, "ax\nbx");
        assert_eq!(run("a\nb\nc", 0, "J").0, "a b\nc");
        assert_eq!(run("abc", 0, "~~").0, "ABc");
        assert_eq!(run("abc", 0, "rz").0, "zbc");
        assert_eq!(run("a", 0, ">>").0, "  a");
        assert_eq!(run("a", 0, "oneu<esc>").0, "a\nneu");
    }

    #[test]
    fn delete_last_lines() {
        // cursor on the empty last line after a trailing newline
        let (t, p, _) = run("a\nb\n", 4, "dd");
        assert_eq!(t, "a\nb");
        assert_eq!(p, 2);
        // last real line of a file with trailing newline
        assert_eq!(run("a\nb\n", 2, "dd").0, "a\n");
        assert_eq!(run("a\nb\n", 2, "dddd").0, "a");
        // last line without trailing newline, cursor moves up
        let (t, p, _) = run("a\nbc", 3, "dd");
        assert_eq!((t.as_str(), p), ("a", 0));
        // count larger than remaining lines deletes to the end
        assert_eq!(run("a\nb\nc", 2, "5dd").0, "a");
        // only line
        assert_eq!(run("abc", 1, "dd").0, "");
        assert_eq!(run("\n", 1, "dd").0, "");
        // dj on the second-to-last line
        assert_eq!(run("a\nb\nc", 2, "dj").0, "a");
        // dd then p restores the line below
        assert_eq!(run("a\nb", 2, "ddp").0, "a\nb");
    }

    #[test]
    fn visual_block() {
        // comment out three lines with Ctrl-v j j I % Esc
        assert_eq!(run("a\nb\nc", 0, "<c-v>jjI% <esc>").0, "% a\n% b\n% c");
        // delete a column
        assert_eq!(run("abc\nabc\nabc", 1, "<c-v>jjd").0, "ac\nac\nac");
        // change a column
        assert_eq!(run("x1\nx2", 0, "<c-v>jcy<esc>").0, "y1\ny2");
        // append after block, padding short lines
        assert_eq!(run("ab\na\nab", 0, "<c-v>jjlA|<esc>").0, "ab|\na |\nab|");
        // yank block and paste
        assert_eq!(run("ab\ncd", 0, "<c-v>jyP").0, "aab\nccd");
        // replace block
        assert_eq!(run("abc\nabc", 0, "<c-v>jlrx").0, "xxc\nxxc");
        // append at line ends with $
        assert_eq!(run("a\nbcd", 0, "<c-v>j$A;<esc>").0, "a;\nbcd;");
    }

    #[test]
    fn visual_and_ex() {
        assert_eq!(run("hello world", 0, "vex").0, " world");
        assert_eq!(run("a\nb\nc", 0, "Vjd").0, "c");
        assert_eq!(run("a\nb\nc", 0, "Vjy").2.register, "a\nb\n");
        let (t, _, _) = run("foo foo\nfoo", 0, ":%s/foo/bar/g<cr>");
        assert_eq!(t, "bar bar\nbar");
        let (t, _, _) = run("foo foo", 0, ":s/foo/bar<cr>");
        assert_eq!(t, "bar foo");
        let (_, p, _) = run("a\nb\nc", 0, ":3<cr>");
        assert_eq!(p, 4);
        let mut v = VimState::default();
        let mut t = "x".to_string();
        let o = v.process(&mut t, &keys(":wq<cr>"));
        assert!(o.save && o.close);
        let (_, p, _) = run("one two one", 0, "/one<cr>");
        assert_eq!(p, 8);
    }
}
