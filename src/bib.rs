//! A small, tolerant BibTeX parser and pretty-printer.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BibEntry {
    pub kind: String,
    pub key: String,
    pub fields: Vec<(String, String)>,
}

/// Parsed .bib file: entries plus verbatim non-entry blocks (@string, @preamble, @comment).
#[derive(Default, Debug)]
pub struct BibFile {
    pub entries: Vec<BibEntry>,
    pub extras: Vec<String>,
}

impl BibEntry {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, name: &str, value: &str) {
        let value = value.trim();
        if let Some(f) = self.fields.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            if value.is_empty() {
                let n = f.0.clone();
                self.fields.retain(|(k, _)| *k != n);
            } else {
                f.1 = value.to_string();
            }
        } else if !value.is_empty() {
            self.fields.push((name.to_lowercase(), value.to_string()));
        }
    }

    pub fn title(&self) -> String {
        clean(self.get("title").unwrap_or("(ohne Titel)"))
    }

    pub fn year(&self) -> String {
        self.get("year")
            .map(clean)
            .or_else(|| self.get("date").map(|d| clean(d).chars().take(4).collect()))
            .unwrap_or_default()
    }

    pub fn authors(&self) -> Vec<String> {
        let raw = self.get("author").or_else(|| self.get("editor")).unwrap_or("");
        split_authors(raw)
    }

    /// "Vaswani et al." / "Knuth & Lamport" / "Knuth"
    pub fn authors_short(&self) -> String {
        let a = self.authors();
        let last: Vec<String> = a.iter().map(|n| last_name(n)).collect();
        match last.len() {
            0 => "Unbekannt".into(),
            1 => last[0].clone(),
            2 => format!("{} & {}", last[0], last[1]),
            _ => format!("{} et al.", last[0]),
        }
    }

    pub fn authors_full(&self) -> String {
        self.authors().iter().map(|n| display_name(n)).collect::<Vec<_>>().join(", ")
    }

    pub fn venue(&self) -> String {
        for k in ["journal", "booktitle", "publisher", "school", "institution", "howpublished", "eprinttype", "archiveprefix"] {
            if let Some(v) = self.get(k) {
                return clean(v);
            }
        }
        String::new()
    }

    pub fn doi(&self) -> Option<String> {
        self.get("doi").map(|d| clean(d))
    }

    pub fn url(&self) -> Option<String> {
        if let Some(u) = self.get("url") {
            return Some(clean(u));
        }
        self.doi().map(|d| format!("https://doi.org/{d}"))
    }

    pub fn to_bibtex(&self) -> String {
        let width = self.fields.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        let mut s = format!("@{}{{{},\n", self.kind.to_lowercase(), self.key);
        for (i, (k, v)) in self.fields.iter().enumerate() {
            let is_num = !v.is_empty() && v.chars().all(|c| c.is_ascii_digit());
            let val = if is_num { v.clone() } else { format!("{{{v}}}") };
            s.push_str(&format!("  {:width$} = {}", k, val, width = width));
            s.push_str(if i + 1 < self.fields.len() { ",\n" } else { "\n" });
        }
        s.push('}');
        s
    }

    /// Generate a citation key like `vaswani2017attention`.
    pub fn suggest_key(&self) -> String {
        let author = self.authors().first().map(|a| ascii_fold(&last_name(a))).unwrap_or_else(|| "anon".into());
        let author: String = author.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
        let year: String = self.year().chars().filter(|c| c.is_ascii_digit()).collect();
        const STOP: &[&str] = &[
            "a", "an", "the", "on", "of", "for", "in", "and", "to", "with", "towards", "toward", "is", "are", "via",
            "der", "die", "das", "ein", "eine", "zur", "zum", "und", "von", "fur", "uber", "mit", "im",
        ];
        let word = ascii_fold(&self.title())
            .split(|c: char| !c.is_ascii_alphanumeric())
            .map(|w| w.to_lowercase())
            .find(|w| w.len() > 1 && !STOP.contains(&w.as_str()))
            .unwrap_or_default();
        format!("{}{}{}", if author.is_empty() { "anon".into() } else { author }, year, word)
    }
}

// ───────────────────────────── parsing ─────────────────────────────

pub fn parse(src: &str) -> BibFile {
    let chars: Vec<char> = src.chars().collect();
    let mut out = BibFile::default();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '@' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let mut kind = String::new();
        while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
            kind.push(chars[i]);
            i += 1;
        }
        skip_ws(&chars, &mut i);
        if i >= chars.len() || (chars[i] != '{' && chars[i] != '(') {
            continue;
        }
        let close = if chars[i] == '{' { '}' } else { ')' };
        let lk = kind.to_lowercase();
        if lk == "comment" || lk == "string" || lk == "preamble" {
            // keep verbatim
            let end = find_balanced(&chars, i);
            let block: String = chars[start..end.min(chars.len())].iter().collect();
            out.extras.push(block);
            i = end;
            continue;
        }
        i += 1;
        skip_ws(&chars, &mut i);
        let mut key = String::new();
        while i < chars.len() && chars[i] != ',' && chars[i] != close && !chars[i].is_whitespace() {
            key.push(chars[i]);
            i += 1;
        }
        skip_ws(&chars, &mut i);
        let mut entry = BibEntry { kind: lk, key, fields: vec![] };
        if i < chars.len() && chars[i] == ',' {
            i += 1;
        }
        loop {
            skip_ws_commas(&chars, &mut i);
            if i >= chars.len() {
                break;
            }
            if chars[i] == close {
                i += 1;
                break;
            }
            let mut name = String::new();
            while i < chars.len() && (chars[i].is_alphanumeric() || "_-:.".contains(chars[i])) {
                name.push(chars[i]);
                i += 1;
            }
            skip_ws(&chars, &mut i);
            if name.is_empty() || i >= chars.len() || chars[i] != '=' {
                // malformed: skip to next comma or close
                while i < chars.len() && chars[i] != ',' && chars[i] != close {
                    i += 1;
                }
                continue;
            }
            i += 1;
            let value = parse_value(&chars, &mut i, close);
            entry.fields.push((name.to_lowercase(), value));
        }
        if !entry.key.is_empty() {
            out.entries.push(entry);
        }
    }
    out
}

fn skip_ws(c: &[char], i: &mut usize) {
    while *i < c.len() && c[*i].is_whitespace() {
        *i += 1;
    }
}

fn skip_ws_commas(c: &[char], i: &mut usize) {
    while *i < c.len() && (c[*i].is_whitespace() || c[*i] == ',') {
        *i += 1;
    }
}

fn find_balanced(c: &[char], open_at: usize) -> usize {
    let (open, close) = if c[open_at] == '{' { ('{', '}') } else { ('(', ')') };
    let mut depth = 0;
    let mut i = open_at;
    while i < c.len() {
        if c[i] == open {
            depth += 1;
        } else if c[i] == close {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
        i += 1;
    }
    c.len()
}

fn parse_value(c: &[char], i: &mut usize, close: char) -> String {
    let mut parts: Vec<String> = vec![];
    loop {
        skip_ws(c, i);
        if *i >= c.len() {
            break;
        }
        match c[*i] {
            '{' => {
                let end = find_balanced(c, *i);
                parts.push(c[*i + 1..end.saturating_sub(1).max(*i + 1)].iter().collect());
                *i = end;
            }
            '"' => {
                *i += 1;
                let mut s = String::new();
                let mut depth = 0;
                while *i < c.len() {
                    let ch = c[*i];
                    if ch == '{' {
                        depth += 1;
                    } else if ch == '}' {
                        depth -= 1;
                    } else if ch == '"' && depth <= 0 {
                        break;
                    }
                    s.push(ch);
                    *i += 1;
                }
                *i += 1;
                parts.push(s);
            }
            _ => {
                let mut s = String::new();
                while *i < c.len() && c[*i] != ',' && c[*i] != close && c[*i] != '#' && !c[*i].is_whitespace() {
                    s.push(c[*i]);
                    *i += 1;
                }
                parts.push(s);
            }
        }
        skip_ws(c, i);
        if *i < c.len() && c[*i] == '#' {
            *i += 1;
            continue;
        }
        break;
    }
    let joined = parts.join("");
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn serialize(file: &BibFile) -> String {
    let mut s = String::new();
    for x in &file.extras {
        s.push_str(x.trim());
        s.push_str("\n\n");
    }
    for e in &file.entries {
        s.push_str(&e.to_bibtex());
        s.push_str("\n\n");
    }
    s.trim_end().to_string() + "\n"
}

// ───────────────────────────── text helpers ─────────────────────────────

pub fn split_authors(raw: &str) -> Vec<String> {
    let mut out = vec![];
    let mut depth = 0;
    let mut cur = String::new();
    let words: Vec<&str> = raw.split(' ').collect();
    for w in words {
        depth += w.matches('{').count() as i32 - w.matches('}').count() as i32;
        if w == "and" && depth == 0 {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push_str(w);
            cur.push(' ');
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

pub fn last_name(name: &str) -> String {
    let n = clean(name);
    if let Some((last, _)) = n.split_once(',') {
        return last.trim().to_string();
    }
    n.split_whitespace().last().unwrap_or("").to_string()
}

pub fn display_name(name: &str) -> String {
    let n = clean(name);
    if let Some((last, first)) = n.split_once(',') {
        format!("{} {}", first.trim(), last.trim())
    } else {
        n
    }
}

/// Convert common LaTeX markup into readable Unicode text.
pub fn clean(s: &str) -> String {
    let mut t = s.to_string();
    const MAP: &[(&str, &str)] = &[
        ("\\\"a", "ä"), ("\\\"o", "ö"), ("\\\"u", "ü"), ("\\\"A", "Ä"), ("\\\"O", "Ö"), ("\\\"U", "Ü"),
        ("\\\"{a}", "ä"), ("\\\"{o}", "ö"), ("\\\"{u}", "ü"), ("\\\"{A}", "Ä"), ("\\\"{O}", "Ö"), ("\\\"{U}", "Ü"),
        ("\\'e", "é"), ("\\'{e}", "é"), ("\\`e", "è"), ("\\`{e}", "è"), ("\\'a", "á"), ("\\'{a}", "á"),
        ("\\'i", "í"), ("\\'{i}", "í"), ("\\'{\\i}", "í"), ("\\'o", "ó"), ("\\'{o}", "ó"), ("\\'u", "ú"), ("\\'{u}", "ú"),
        ("\\c{c}", "ç"), ("\\c c", "ç"), ("\\~n", "ñ"), ("\\~{n}", "ñ"), ("\\v{c}", "č"), ("\\v{s}", "š"), ("\\v{z}", "ž"),
        ("\\ss{}", "ß"), ("\\ss", "ß"), ("\\o{}", "ø"), ("\\aa{}", "å"), ("\\l{}", "ł"),
        ("\\&", "&"), ("\\%", "%"), ("\\_", "_"), ("\\$", "$"), ("\\#", "#"),
        ("\\TeX", "TeX"), ("\\LaTeX", "LaTeX"), ("\\textendash", "–"), ("\\textemdash", "—"),
        ("---", "—"), ("--", "–"), ("~", " "), ("\\textit", ""), ("\\textbf", ""), ("\\emph", ""), ("\\url", ""),
    ];
    for (a, b) in MAP {
        if t.contains(a) {
            t = t.replace(a, b);
        }
    }
    t.retain(|c| c != '{' && c != '}');
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn ascii_fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            'ä' => "ae".chars().collect::<Vec<_>>(),
            'ö' => "oe".chars().collect(),
            'ü' => "ue".chars().collect(),
            'Ä' => "Ae".chars().collect(),
            'Ö' => "Oe".chars().collect(),
            'Ü' => "Ue".chars().collect(),
            'ß' => "ss".chars().collect(),
            'é' | 'è' | 'ê' => vec!['e'],
            'á' | 'à' | 'â' | 'å' => vec!['a'],
            'í' | 'ì' => vec!['i'],
            'ó' | 'ò' | 'ø' => vec!['o'],
            'ú' | 'ù' => vec!['u'],
            'ç' | 'č' => vec!['c'],
            'ñ' => vec!['n'],
            'š' => vec!['s'],
            'ž' => vec!['z'],
            'ł' => vec!['l'],
            c => vec![c],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let src = r#"@string{acm = "ACM"}
@article{Vaswani_2017,
  title = {Attention Is All You Need},
  author = "Vaswani, Ashish and Shazeer, Noam and Parmar, Niki",
  year = 2017, journal = acm # " Press"
}"#;
        let f = parse(src);
        assert_eq!(f.entries.len(), 1);
        assert_eq!(f.extras.len(), 1);
        let e = &f.entries[0];
        assert_eq!(e.get("year"), Some("2017"));
        assert_eq!(e.authors_short(), "Vaswani et al.");
        assert_eq!(e.suggest_key(), "vaswani2017attention");
        let again = parse(&serialize(&f));
        assert_eq!(again.entries[0], f.entries[0]);
    }

    #[test]
    fn umlauts() {
        assert_eq!(clean("M{\\\"u}ller and {\\TeX}"), "Müller and TeX");
    }
}
