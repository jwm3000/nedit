//! "Plan": tasks and notes of a project (stored in `.nedit/plan.json`), quick capture in
//! natural language ("Kapitel 3 überarbeiten bis Freitag #methodik !"), and TODOs found in
//! the LaTeX sources (`% TODO …`, `\todo{…}`).

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Task {
    pub id: u64,
    pub text: String,
    pub done: bool,
    /// due date, ISO `YYYY-MM-DD`
    pub due: Option<String>,
    pub important: bool,
    pub tags: Vec<String>,
    pub created: i64,
    pub done_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Note {
    pub id: u64,
    pub title: String,
    pub body: String,
    /// index into the note palette
    pub color: u8,
    pub pinned: bool,
    pub created: i64,
    pub updated: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Plan {
    pub tasks: Vec<Task>,
    pub notes: Vec<Note>,
    /// submission date, ISO
    pub deadline: Option<String>,
    pub word_goal: Option<u32>,
    pub page_goal: Option<u32>,
    /// hide finished tasks (they are shown struck through otherwise)
    pub hide_done: bool,
    /// finished pomodoros per day (ISO date → count)
    pub pomodoros: std::collections::BTreeMap<String, u32>,
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    saved: String,
}

impl Plan {
    pub fn load(root: &Path) -> Plan {
        let path = root.join(".nedit/plan.json");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut p: Plan = serde_json::from_str(&text).unwrap_or_default();
        p.path = path;
        p.saved = serde_json::to_string_pretty(&p).unwrap_or_default();
        p
    }

    /// Writes only when something changed.
    pub fn save(&mut self) {
        let s = serde_json::to_string_pretty(&*self).unwrap_or_default();
        if s == self.saved || self.path.as_os_str().is_empty() {
            return;
        }
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if std::fs::write(&self.path, &s).is_ok() {
            self.saved = s;
        }
    }

    pub fn next_id(&self) -> u64 {
        self.tasks.iter().map(|t| t.id).chain(self.notes.iter().map(|n| n.id)).max().unwrap_or(0) + 1
    }

    pub fn add_task(&mut self, c: Capture) {
        let id = self.next_id();
        self.tasks.push(Task { id, text: c.text, done: false, due: c.due.map(|d| d.to_string()), important: c.important, tags: c.tags, created: now(), done_at: None });
    }

    pub fn add_note(&mut self, title: &str, body: &str) -> u64 {
        let id = self.next_id();
        let color = (self.notes.len() % NOTE_COLORS) as u8;
        self.notes.push(Note { id, title: title.to_string(), body: body.to_string(), color, pinned: false, created: now(), updated: now() });
        id
    }

    pub fn toggle(&mut self, id: u64) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            t.done = !t.done;
            t.done_at = t.done.then(now);
        }
    }

    pub fn all_tags(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tasks.iter().filter(|t| !t.done).flat_map(|t| t.tags.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}

pub const NOTE_COLORS: usize = 6;

pub fn now() -> i64 {
    chrono::Local::now().timestamp()
}

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

pub fn parse_iso(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

// ───────────────────────────── quick capture ─────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capture {
    pub text: String,
    pub due: Option<NaiveDate>,
    pub important: bool,
    pub tags: Vec<String>,
}

fn weekday(w: &str) -> Option<Weekday> {
    Some(match w {
        "mo" | "montag" | "mon" | "monday" => Weekday::Mon,
        "di" | "dienstag" | "tue" | "tues" | "tuesday" => Weekday::Tue,
        "mi" | "mittwoch" | "wed" | "wednesday" => Weekday::Wed,
        "do" | "donnerstag" | "thu" | "thurs" | "thursday" => Weekday::Thu,
        "fr" | "freitag" | "fri" | "friday" => Weekday::Fri,
        "sa" | "samstag" | "sat" | "saturday" => Weekday::Sat,
        "so" | "sonntag" | "sun" | "sunday" => Weekday::Sun,
        _ => return None,
    })
}

/// Next occurrence of a weekday (today counts as "next week's").
fn next_weekday(today: NaiveDate, w: Weekday) -> NaiveDate {
    let mut d = today + Duration::days(1);
    while d.weekday() != w {
        d += Duration::days(1);
    }
    d
}

/// "15.11.", "15.11.2026", "2026-11-15"
fn explicit_date(tok: &str, today: NaiveDate) -> Option<NaiveDate> {
    if let Some(d) = parse_iso(tok) {
        return Some(d);
    }
    let parts: Vec<&str> = tok.trim_end_matches('.').split('.').collect();
    let (d, m, y) = match parts.as_slice() {
        [d, m] => (d.parse().ok()?, m.parse().ok()?, None),
        [d, m, y] => (d.parse().ok()?, m.parse().ok()?, Some(y.parse::<i32>().ok()?)),
        _ => return None,
    };
    let y = y.map(|y| if y < 100 { 2000 + y } else { y });
    match y {
        Some(y) => NaiveDate::from_ymd_opt(y, m, d),
        None => {
            // without a year: the next such date
            let this = NaiveDate::from_ymd_opt(today.year(), m, d)?;
            if this >= today { Some(this) } else { NaiveDate::from_ymd_opt(today.year() + 1, m, d) }
        }
    }
}

/// Parse a quick-capture line: dates, `#tags`, `!` (important). `@citekeys` stay in the text.
pub fn parse_capture(input: &str, today: NaiveDate) -> Capture {
    let toks: Vec<&str> = input.split_whitespace().collect();
    let mut keep = vec![true; toks.len()];
    let mut c = Capture::default();
    let lower: Vec<String> = toks.iter().map(|t| t.to_lowercase().trim_matches(|ch: char| matches!(ch, ',' | ';')).to_string()).collect();
    let mut i = 0;
    while i < toks.len() {
        let t = lower[i].as_str();
        let mut date: Option<(NaiveDate, usize)> = None; // (date, tokens used)
        match t {
            "heute" | "today" => date = Some((today, 1)),
            "morgen" | "tomorrow" => date = Some((today + Duration::days(1), 1)),
            "übermorgen" => date = Some((today + Duration::days(2), 1)),
            "nächste" | "naechste" | "next" if lower.get(i + 1).is_some_and(|n| n == "woche" || n == "week") => {
                date = Some((next_weekday(today, Weekday::Mon), 2));
            }
            "in" => {
                if let (Some(n), Some(unit)) = (lower.get(i + 1).and_then(|n| n.parse::<i64>().ok()), lower.get(i + 2)) {
                    let days = match unit.as_str() {
                        "tag" | "tagen" | "tage" | "day" | "days" => Some(n),
                        "woche" | "wochen" | "week" | "weeks" => Some(n * 7),
                        _ => None,
                    };
                    if let Some(dd) = days {
                        date = Some((today + Duration::days(dd), 3));
                    }
                }
            }
            "!" | "!!" | "!!!" => {
                c.important = true;
                keep[i] = false;
            }
            _ if t.starts_with('#') && t.len() > 1 => {
                let tag = toks[i][1..].trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '_').to_string();
                if !tag.is_empty() && !c.tags.contains(&tag) {
                    c.tags.push(tag);
                }
                keep[i] = false;
            }
            _ => {
                if let Some(w) = weekday(t.trim_end_matches('.')) {
                    // short forms ("so", "do", "mi" …) only after "bis"/"am"/"by"/"on"
                    let short = t.trim_end_matches('.').len() <= 3;
                    let prep = i > 0 && matches!(lower[i - 1].as_str(), "bis" | "am" | "ab" | "by" | "on" | "until" | "due");
                    if !short || prep {
                        date = Some((next_weekday(today, w), 1));
                    }
                } else if let Some(d) = explicit_date(t, today) {
                    date = Some((d, 1));
                }
            }
        }
        if let Some((d, n)) = date {
            c.due = Some(d);
            for k in i..i + n {
                keep[k] = false;
            }
            // drop a preposition right before the date ("bis Freitag", "am 15.11.")
            if i > 0 && keep[i - 1] && matches!(lower[i - 1].as_str(), "bis" | "am" | "ab" | "by" | "on" | "until" | "due") {
                keep[i - 1] = false;
            }
            i += n;
            continue;
        }
        i += 1;
    }
    c.text = toks.iter().zip(&keep).filter(|(_, k)| **k).map(|(t, _)| *t).collect::<Vec<_>>().join(" ");
    c
}

// ───────────────────────────── grouping ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Overdue,
    Today,
    Tomorrow,
    Week,
    Later,
    NoDate,
}

pub fn group_of(t: &Task, today: NaiveDate) -> Group {
    match t.due.as_deref().and_then(parse_iso) {
        None => Group::NoDate,
        Some(d) if d < today => Group::Overdue,
        Some(d) if d == today => Group::Today,
        Some(d) if d == today + Duration::days(1) => Group::Tomorrow,
        Some(d) if d <= today + Duration::days(7) => Group::Week,
        Some(_) => Group::Later,
    }
}

/// "heute", "morgen", "Fr 14.11.", "vor 2 Tagen" …
pub fn due_label(d: NaiveDate, today: NaiveDate) -> String {
    let diff = (d - today).num_days();
    let en = crate::i18n::en();
    match diff {
        0 => if en { "today".into() } else { "heute".into() },
        1 => if en { "tomorrow".into() } else { "morgen".into() },
        -1 => if en { "yesterday".into() } else { "gestern".into() },
        n if n < -1 => if en { format!("{} days ago", -n) } else { format!("vor {} Tagen", -n) },
        n if n < 7 => {
            let names = if en { ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] } else { ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"] };
            format!("{} {}.{}.", names[d.weekday().num_days_from_monday() as usize], d.day(), d.month())
        }
        _ => format!("{}.{}.{}", d.day(), d.month(), d.year()),
    }
}

// ───────────────────────────── TODOs in the sources ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct SourceTodo {
    pub file: String,
    pub line: usize,
    pub text: String,
}

/// `% TODO: …`, `% FIXME …` and `\todo[…]{…}` in a LaTeX file.
pub fn scan_todos(file: &str, text: &str) -> Vec<SourceTodo> {
    let re_comment = regex::Regex::new(r"%\s*(?:TODO|FIXME|ToDo|todo)\b[:\s]*(.*)$").unwrap();
    let re_cmd = regex::Regex::new(r"\\todo(?:\[[^\]]*\])?\{([^{}]*)\}").unwrap();
    let mut out = vec![];
    for (i, line) in text.lines().enumerate() {
        if let Some(c) = re_comment.captures(line) {
            // an escaped \% is not a comment
            let pos = c.get(0).unwrap().start();
            if pos == 0 || line.as_bytes()[pos - 1] != b'\\' {
                let t = c[1].trim();
                out.push(SourceTodo { file: file.to_string(), line: i + 1, text: if t.is_empty() { "TODO".into() } else { t.to_string() } });
                continue;
            }
        }
        if line.trim_start().starts_with('%') {
            continue;
        }
        for c in re_cmd.captures_iter(line) {
            out.push(SourceTodo { file: file.to_string(), line: i + 1, text: c[1].trim().to_string() });
        }
    }
    out
}

/// Rough word count of LaTeX text (commands, math and comments left out).
pub fn count_words(text: &str) -> usize {
    let re = regex::Regex::new(r"(?s)%[^\n]*|\$[^$]*\$|\\[A-Za-z]+\*?(\[[^\]]*\])?|[{}]").unwrap();
    let clean = re.replace_all(text, " ");
    clean.split_whitespace().filter(|w| w.chars().any(|c| c.is_alphabetic())).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn capture_parsing() {
        let today = d(2026, 10, 12); // a Monday
        let c = parse_capture("Kapitel 3 überarbeiten bis Freitag #methodik !", today);
        assert_eq!(c, Capture { text: "Kapitel 3 überarbeiten".into(), due: Some(d(2026, 10, 16)), important: true, tags: vec!["methodik".into()] });
        assert_eq!(parse_capture("Abstract schreiben morgen", today).due, Some(d(2026, 10, 13)));
        assert_eq!(parse_capture("Betreuerin mailen am 15.11.", today), Capture { text: "Betreuerin mailen".into(), due: Some(d(2026, 11, 15)), ..Default::default() });
        assert_eq!(parse_capture("Druck 3.1.", today).due, Some(d(2027, 1, 3)), "past date without year → next year");
        assert_eq!(parse_capture("Review in 3 Tagen", today).due, Some(d(2026, 10, 15)));
        assert_eq!(parse_capture("Plan nächste Woche", today).due, Some(d(2026, 10, 19)));
        assert_eq!(parse_capture("finish draft by friday #ch3", today).due, Some(d(2026, 10, 16)));
        // short weekday words are only dates after a preposition ("so" = "so")
        let c = parse_capture("das ist so gut", today);
        assert_eq!((c.text.as_str(), c.due), ("das ist so gut", None));
        assert_eq!(parse_capture("Treffen am Do", today).due, Some(d(2026, 10, 15)));
        // citation keys stay in the text
        assert_eq!(parse_capture("@malz2021ester lesen", today).text, "@malz2021ester lesen");
    }

    #[test]
    fn groups_and_labels() {
        let today = d(2026, 10, 12);
        let t = |due: Option<&str>| Task { due: due.map(String::from), ..Default::default() };
        assert_eq!(group_of(&t(Some("2026-10-10")), today), Group::Overdue);
        assert_eq!(group_of(&t(Some("2026-10-12")), today), Group::Today);
        assert_eq!(group_of(&t(Some("2026-10-13")), today), Group::Tomorrow);
        assert_eq!(group_of(&t(Some("2026-10-18")), today), Group::Week);
        assert_eq!(group_of(&t(Some("2026-11-30")), today), Group::Later);
        assert_eq!(group_of(&t(None), today), Group::NoDate);
        assert_eq!(due_label(d(2026, 10, 16), today), "Fr 16.10.");
    }

    #[test]
    fn source_todos_and_words() {
        let tex = "Text % TODO: Zahl prüfen\n\\todo{Quelle fehlt} more\n50\\% TODO not a comment\n% \\todo{commented out}\n";
        let v = scan_todos("content/a.tex", tex);
        assert_eq!(v.iter().map(|t| (t.line, t.text.as_str())).collect::<Vec<_>>(), [(1, "Zahl prüfen"), (2, "Quelle fehlt")]);
        assert_eq!(count_words("\\section{Intro} Bier ist alt. % Kommentar\n$x^2$ und \\textbf{gut}"), 6);
    }

    #[test]
    fn persistence() {
        let dir = std::env::temp_dir().join(format!("nedit-plan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = Plan::load(&dir);
        p.add_task(parse_capture("Test #a !", today()));
        let id = p.add_note("Idee", "Text");
        p.save();
        let q = Plan::load(&dir);
        assert_eq!(q.tasks.len(), 1);
        assert_eq!(q.notes[0].id, id);
        assert!(q.tasks[0].important);
        let _ = std::fs::remove_dir_all(dir);
    }
}
