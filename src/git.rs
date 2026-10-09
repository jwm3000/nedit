//! Thin wrapper around the `git` command line for project versioning.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Debug)]
pub struct Change {
    pub code: String, // porcelain XY, e.g. " M", "??", "A "
    pub path: String,
}

impl Change {
    pub fn label(&self) -> (&'static str, char) {
        let c = self.code.trim();
        match c {
            "??" => (tr!("Neu" | "New"), 'N'),
            _ if c.contains('D') => (tr!("Gelöscht" | "Deleted"), 'D'),
            _ if c.contains('A') => (tr!("Hinzugefügt" | "Added"), 'A'),
            _ if c.contains('R') => (tr!("Umbenannt" | "Renamed"), 'R'),
            _ if c.contains('U') => (tr!("Konflikt" | "Conflict"), 'U'),
            _ => (tr!("Geändert" | "Modified"), 'M'),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub time: i64,
    pub subject: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DiffKind {
    Header,
    Hunk,
    Add,
    Del,
    Ctx,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarkKind {
    Added,
    Modified,
    Deleted,
}

/// Changed line range in the working file compared to HEAD (1-based, inclusive start).
#[derive(Clone, Copy, Debug)]
pub struct LineMark {
    pub start: usize,
    pub count: usize,
    pub kind: MarkKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GitView {
    WorkingFile(String),
    Commit(String),
}

pub struct GitState {
    pub is_repo: bool,
    pub git_available: bool,
    pub branch: String,
    pub remote: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub changes: Vec<Change>,
    pub log: Vec<Commit>,
    pub message: String,
    pub status: String,
    pub busy: bool,
    pub last_refresh: f64,
    pub view: Option<GitView>,
    pub view_lines: Vec<(DiffKind, String)>,
    pub view_files: Vec<Change>,
    pub view_title: String,
    pub head: String,
    /// Files left out of the next commit.
    pub excluded: std::collections::HashSet<String>,
    marks: std::collections::HashMap<String, (u64, String, Vec<LineMark>)>,
    rx: Option<Receiver<Result<String, String>>>,
    status_rx: Option<Receiver<StatusSnapshot>>,
}

impl Default for GitState {
    fn default() -> Self {
        GitState {
            is_repo: false,
            git_available: true,
            branch: String::new(),
            remote: None,
            ahead: 0,
            behind: 0,
            changes: vec![],
            log: vec![],
            message: String::new(),
            status: String::new(),
            busy: false,
            last_refresh: -100.0,
            view: None,
            view_lines: vec![],
            view_files: vec![],
            view_title: String::new(),
            head: String::new(),
            excluded: Default::default(),
            marks: Default::default(),
            rx: None,
            status_rx: None,
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = crate::platform::cmd("git")
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .map_err(|e| trf!("git nicht gefunden: {e}" | "git not found: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let err = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
        Err(err)
    }
}

/// The project folder itself must be the repository root (so a thesis inside some
/// other repository is not mixed up with it).
fn repo_root(root: &Path) -> Option<PathBuf> {
    git(root, &["rev-parse", "--show-toplevel"]).ok().map(|s| PathBuf::from(s.trim()))
}

impl GitState {
    /// Full refresh: status and history.
    pub fn refresh(&mut self, root: &Path, now: f64) {
        self.refresh_status(root, now);
        if self.is_repo {
            self.refresh_log(root);
        }
    }

    /// Cheap refresh used in the background: repository state and changed files.
    pub fn refresh_status(&mut self, root: &Path, now: f64) {
        self.last_refresh = now;
        let snap = status_snapshot(root);
        self.apply(snap);
    }

    /// Background status refresh (does not block the UI); results arrive via `poll_status`.
    pub fn request_status(&mut self, root: &Path, now: f64, ctx: &egui::Context) {
        if self.status_rx.is_some() {
            return;
        }
        self.last_refresh = now;
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(status_snapshot(&root));
            ctx.request_repaint();
        });
        self.status_rx = Some(rx);
    }

    /// Apply a finished background refresh. Returns true if something changed.
    pub fn poll_status(&mut self) -> bool {
        let Some(rx) = &self.status_rx else { return false };
        match rx.try_recv() {
            Ok(snap) => {
                self.status_rx = None;
                let before: Vec<(String, String)> = self.changes.iter().map(|c| (c.code.clone(), c.path.clone())).collect();
                let head_before = self.head.clone();
                self.apply(snap);
                let after: Vec<(String, String)> = self.changes.iter().map(|c| (c.code.clone(), c.path.clone())).collect();
                before != after || head_before != self.head
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.status_rx = None;
                false
            }
            Err(_) => false,
        }
    }

    /// Ask for a refresh as soon as possible (e.g. after saving or creating files).
    pub fn invalidate(&mut self) {
        self.last_refresh = -100.0;
    }

    fn apply(&mut self, s: StatusSnapshot) {
        self.git_available = s.git_available;
        self.is_repo = s.is_repo;
        if !s.is_repo {
            self.changes.clear();
            self.log.clear();
            return;
        }
        self.branch = s.branch;
        self.remote = s.remote;
        self.ahead = s.ahead;
        self.behind = s.behind;
        self.changes = s.changes;
        self.head = s.head;
        let paths: Vec<String> = self.changes.iter().map(|c| c.path.clone()).collect();
        self.excluded.retain(|p| paths.contains(p));
    }

    pub fn refresh_log(&mut self, root: &Path) {
        self.log = git(root, &["log", "-n", "300", "--pretty=format:%H%x1f%h%x1f%an%x1f%ct%x1f%s"])
            .map(|s| {
                s.lines()
                    .filter_map(|l| {
                        let p: Vec<&str> = l.split('\u{1f}').collect();
                        (p.len() == 5).then(|| Commit { hash: p[0].into(), short: p[1].into(), author: p[2].into(), time: p[3].parse().unwrap_or(0), subject: p[4].into() })
                    })
                    .collect()
            })
            .unwrap_or_default();
    }

    pub fn init(&mut self, root: &Path) -> Result<(), String> {
        let gi = root.join(".gitignore");
        let mut ignore = std::fs::read_to_string(&gi).unwrap_or_default();
        for line in [".nedit/build/", "*.aux", "*.log", "*.synctex.gz", "*.fdb_latexmk", "*.fls"] {
            if !ignore.lines().any(|l| l.trim() == line) {
                ignore.push_str(line);
                ignore.push('\n');
            }
        }
        std::fs::write(&gi, ignore).map_err(|e| e.to_string())?;
        git(root, &["init", "-b", "main"])?;
        git(root, &["add", "-A"])?;
        git(root, &["commit", "-m", tr!("Erste Version" | "First version")])?;
        Ok(())
    }

    pub fn commit(&mut self, root: &Path) -> Result<(), String> {
        let msg = if self.message.trim().is_empty() { trf!("Stand vom {}" | "State of {}", chrono_like_now()) } else { self.message.trim().to_string() };
        if self.excluded.is_empty() {
            git(root, &["add", "-A"])?;
            git(root, &["commit", "-m", &msg])?;
        } else {
            let sel: Vec<String> = self.changes.iter().filter(|c| !self.excluded.contains(&c.path)).map(|c| c.path.clone()).collect();
            if sel.is_empty() {
                return Err(tr!("Keine Datei ausgewählt" | "No file selected").into());
            }
            let mut add = vec!["add", "-A", "--"];
            add.extend(sel.iter().map(String::as_str));
            git(root, &add)?;
            let mut commit = vec!["commit", "-m", &msg, "--"];
            commit.extend(sel.iter().map(String::as_str));
            git(root, &commit)?;
        }
        self.message.clear();
        Ok(())
    }

    /// Status letter for a file in the working tree, if changed.
    pub fn file_status(&self, rel: &str) -> Option<char> {
        self.changes.iter().find(|c| c.path == rel).map(|c| c.label().1)
    }

    pub fn dir_has_changes(&self, dir: &str) -> bool {
        let pre = format!("{dir}/");
        self.changes.iter().any(|c| c.path.starts_with(&pre))
    }

    /// Changed line ranges of a (tracked) file vs. HEAD; cached per file version and HEAD.
    pub fn line_marks(&mut self, root: &Path, rel: &str, stamp: u64) -> Vec<LineMark> {
        if !self.is_repo || self.file_status(rel).is_none_or(|c| c == 'N') {
            return vec![];
        }
        if let Some((st, head, m)) = self.marks.get(rel) {
            if *st == stamp && *head == self.head {
                return m.clone();
            }
        }
        let text = git(root, &["diff", "-U0", "--no-color", "HEAD", "--", rel]).unwrap_or_default();
        let re = regex::Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap();
        let mut marks = vec![];
        for l in text.lines() {
            if let Some(c) = re.captures(l) {
                let old_n: usize = c.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1));
                let new_start: usize = c[3].parse().unwrap_or(1);
                let new_n: usize = c.get(4).map_or(1, |m| m.as_str().parse().unwrap_or(1));
                let mark = if new_n == 0 {
                    LineMark { start: new_start + 1, count: 0, kind: MarkKind::Deleted }
                } else if old_n == 0 {
                    LineMark { start: new_start, count: new_n, kind: MarkKind::Added }
                } else {
                    LineMark { start: new_start, count: new_n, kind: MarkKind::Modified }
                };
                marks.push(mark);
            }
        }
        self.marks.insert(rel.to_string(), (stamp, self.head.clone(), marks.clone()));
        marks
    }

    /// Push / pull run in the background (network).
    pub fn sync(&mut self, root: &Path, pull: bool, ctx: egui::Context) {
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        self.busy = true;
        self.status = if pull { tr!("Hole Änderungen …" | "Fetching changes …").into() } else { tr!("Lade hoch …" | "Uploading …").into() };
        std::thread::spawn(move || {
            let r = if pull {
                git(&root, &["pull", "--rebase", "--autostash"]).map(|_| tr!("Aktualisiert" | "Updated").to_string())
            } else {
                git(&root, &["push", "-u", "origin", "HEAD"]).map(|_| tr!("Hochgeladen" | "Uploaded").to_string())
            };
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    /// Returns a finished background result, if any.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let r = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        self.busy = false;
        Some(r)
    }

    pub fn open_working_diff(&mut self, root: &Path, path: &str) {
        let untracked = self.changes.iter().any(|c| c.path == path && c.code == "??");
        let text = if untracked {
            std::fs::read_to_string(root.join(path)).map(|t| t.lines().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n")).unwrap_or_default()
        } else {
            git(root, &["diff", "HEAD", "--", path]).unwrap_or_else(|e| e)
        };
        self.view_lines = parse_diff(&text);
        self.view_files.clear();
        self.view_title = path.to_string();
        self.view = Some(GitView::WorkingFile(path.to_string()));
    }

    pub fn open_commit(&mut self, root: &Path, hash: &str) {
        let text = git(root, &["show", "--format=", "--patch", hash]).unwrap_or_else(|e| e);
        self.view_lines = parse_diff(&text);
        self.view_files = git(root, &["show", "--format=", "--name-status", hash])
            .map(|s| {
                s.lines()
                    .filter_map(|l| {
                        let mut it = l.split('\t');
                        let code = it.next()?.to_string();
                        let path = it.last()?.to_string();
                        Some(Change { code, path })
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.view_title = self.log.iter().find(|c| c.hash == hash).map(|c| c.subject.clone()).unwrap_or_default();
        self.view = Some(GitView::Commit(hash.to_string()));
    }

    pub fn restore_file(root: &Path, hash: &str, path: &str) -> Result<(), String> {
        git(root, &["checkout", hash, "--", path]).map(|_| ())
    }

    /// Changed files at or below `path` ("" = whole project).
    pub fn changes_under(&self, path: &str) -> Vec<Change> {
        let pre = format!("{}/", path.trim_end_matches('/'));
        self.changes.iter().filter(|c| path.is_empty() || c.path == path || c.path.starts_with(&pre)).cloned().collect()
    }

    /// Revert files to the last commit. Untracked/new files are deleted.
    /// Returns the affected paths (so open editors can be updated).
    pub fn revert(&self, root: &Path, paths: &[String]) -> Result<Vec<String>, String> {
        let mut affected = vec![];
        let mut errors = vec![];
        let mut seen = std::collections::HashSet::new();
        for p in paths {
            for c in self.changes_under(p) {
                if !seen.insert(c.path.clone()) {
                    continue;
                }
                let code = c.code.trim();
                let r = if code == "??" {
                    std::fs::remove_file(root.join(&c.path)).map_err(|e| e.to_string())
                } else if c.code.starts_with('A') {
                    // newly added (staged) file: unstage and delete
                    git(root, &["reset", "-q", "--", &c.path]).and_then(|_| std::fs::remove_file(root.join(&c.path)).map_err(|e| e.to_string()))
                } else {
                    git(root, &["reset", "-q", "--", &c.path]).ok();
                    git(root, &["checkout", "HEAD", "--", &c.path]).map(|_| ())
                };
                match r {
                    Ok(()) => affected.push(c.path.clone()),
                    Err(e) => errors.push(format!("{}: {e}", c.path)),
                }
            }
        }
        // remove folders that became empty through deleting new files
        for a in &affected {
            let mut dir = root.join(a);
            while dir.pop() && dir.starts_with(root) && dir != root {
                if std::fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
                    let _ = std::fs::remove_dir(&dir);
                } else {
                    break;
                }
            }
        }
        if !errors.is_empty() && affected.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(affected)
    }
}

pub struct StatusSnapshot {
    git_available: bool,
    is_repo: bool,
    branch: String,
    remote: Option<String>,
    ahead: usize,
    behind: usize,
    changes: Vec<Change>,
    head: String,
}

/// Collect repository state (runs on a worker thread for background refreshes).
fn status_snapshot(root: &Path) -> StatusSnapshot {
    let mut s = StatusSnapshot { git_available: true, is_repo: false, branch: String::new(), remote: None, ahead: 0, behind: 0, changes: vec![], head: String::new() };
    if crate::platform::cmd("git").arg("--version").output().is_err() {
        s.git_available = false;
        return s;
    }
    let canon = root.canonicalize().unwrap_or(root.to_path_buf());
    s.is_repo = repo_root(root).is_some_and(|r| r.canonicalize().unwrap_or(r) == canon);
    if !s.is_repo {
        return s;
    }
    s.branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|x| x.trim().to_string()).unwrap_or_else(|_| "main".into());
    s.remote = git(root, &["remote"]).ok().and_then(|x| x.lines().next().map(String::from));
    if let Ok(x) = git(root, &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"]) {
        let v: Vec<usize> = x.split_whitespace().filter_map(|n| n.parse().ok()).collect();
        if v.len() == 2 {
            s.behind = v[0];
            s.ahead = v[1];
        }
    }
    s.changes = git(root, &["status", "--porcelain=v1", "-uall"])
        .map(|x| {
            x.lines()
                .filter(|l| l.len() > 3)
                .map(|l| {
                    let path = l[3..].trim().trim_matches('"').to_string();
                    let path = path.rsplit(" -> ").next().unwrap_or(&path).to_string();
                    Change { code: l[..2].to_string(), path }
                })
                .collect()
        })
        .unwrap_or_default();
    s.head = git(root, &["rev-parse", "HEAD"]).map(|x| x.trim().to_string()).unwrap_or_default();
    s
}

/// One row of a side-by-side diff.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitRow {
    File(String),
    Hunk(String),
    Line { old: Option<(usize, String)>, new: Option<(usize, String)> },
}

/// Turn a unified diff into side-by-side rows: context lines on both sides, runs of
/// deletions and additions paired up line by line.
pub fn split_rows(lines: &[(DiffKind, String)]) -> Vec<SplitRow> {
    let re = regex::Regex::new(r"^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@").unwrap();
    let mut out = vec![];
    let (mut o, mut n) = (1usize, 1usize);
    let mut dels: Vec<(usize, String)> = vec![];
    let mut adds: Vec<(usize, String)> = vec![];
    fn flush(out: &mut Vec<SplitRow>, dels: &mut Vec<(usize, String)>, adds: &mut Vec<(usize, String)>) {
        let k = dels.len().max(adds.len());
        for i in 0..k {
            out.push(SplitRow::Line { old: dels.get(i).cloned(), new: adds.get(i).cloned() });
        }
        dels.clear();
        adds.clear();
    }
    for (k, l) in lines {
        match k {
            DiffKind::Header => {
                flush(&mut out, &mut dels, &mut adds);
                if let Some(rest) = l.strip_prefix("diff --git a/") {
                    let name = rest.split(" b/").last().unwrap_or(rest).to_string();
                    out.push(SplitRow::File(name));
                }
            }
            DiffKind::Hunk => {
                flush(&mut out, &mut dels, &mut adds);
                if let Some(c) = re.captures(l) {
                    o = c[1].parse().unwrap_or(1);
                    n = c[2].parse().unwrap_or(1);
                }
                out.push(SplitRow::Hunk(l.clone()));
            }
            DiffKind::Del => {
                if !adds.is_empty() {
                    flush(&mut out, &mut dels, &mut adds);
                }
                dels.push((o, l[1..].to_string()));
                o += 1;
            }
            DiffKind::Add => {
                adds.push((n, l[1..].to_string()));
                n += 1;
            }
            DiffKind::Ctx => {
                flush(&mut out, &mut dels, &mut adds);
                let t = l.strip_prefix(' ').unwrap_or(l).to_string();
                if !l.starts_with('\\') {
                    out.push(SplitRow::Line { old: Some((o, t.clone())), new: Some((n, t)) });
                    o += 1;
                    n += 1;
                }
            }
        }
    }
    flush(&mut out, &mut dels, &mut adds);
    out
}

/// Changed middle part of two lines (char ranges) for intra-line highlighting.
pub fn changed_span(a: &str, b: &str) -> ((usize, usize), (usize, usize)) {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut p = 0;
    while p < a.len() && p < b.len() && a[p] == b[p] {
        p += 1;
    }
    let mut s = 0;
    while s < a.len() - p && s < b.len() - p && a[a.len() - 1 - s] == b[b.len() - 1 - s] {
        s += 1;
    }
    ((p, a.len() - s), (p, b.len() - s))
}

fn parse_diff(text: &str) -> Vec<(DiffKind, String)> {
    text.lines()
        .take(6000)
        .map(|l| {
            let k = if l.starts_with("diff ") || l.starts_with("index ") || l.starts_with("+++") || l.starts_with("---") || l.starts_with("new file") || l.starts_with("deleted file") {
                DiffKind::Header
            } else if l.starts_with("@@") {
                DiffKind::Hunk
            } else if l.starts_with('+') {
                DiffKind::Add
            } else if l.starts_with('-') {
                DiffKind::Del
            } else {
                DiffKind::Ctx
            };
            (k, l.to_string())
        })
        .collect()
}

fn chrono_like_now() -> String {
    chrono::Local::now().format("%d.%m.%Y %H:%M").to_string()
}

/// "vor 5 Minuten", "gestern", "vor 3 Tagen", "12.03.2026"
pub fn relative_time(ts: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(ts);
    let d = (now - ts).max(0);
    match d {
        0..=59 => "gerade eben".into(),
        60..=3599 => format!("vor {} Min.", d / 60),
        3600..=86399 => format!("vor {} Std.", d / 3600),
        86400..=172799 => "gestern".into(),
        _ if d < 86400 * 30 => format!("vor {} Tagen", d / 86400),
        _ => chrono::DateTime::from_timestamp(ts, 0)
            .map(|d| d.with_timezone(&chrono::Local).format("%d.%m.%Y").to_string())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_view() {
        let d = parse_diff("diff --git a/x.tex b/x.tex\n--- a/x.tex\n+++ b/x.tex\n@@ -3,3 +3,4 @@\n eins\n-zwei alt\n+zwei neu\n+drei\n vier");
        let rows = split_rows(&d);
        assert_eq!(rows[0], SplitRow::File("x.tex".into()));
        assert_eq!(rows[2], SplitRow::Line { old: Some((3, "eins".into())), new: Some((3, "eins".into())) });
        assert_eq!(rows[3], SplitRow::Line { old: Some((4, "zwei alt".into())), new: Some((4, "zwei neu".into())) });
        assert_eq!(rows[4], SplitRow::Line { old: None, new: Some((5, "drei".into())) });
        assert_eq!(rows[5], SplitRow::Line { old: Some((5, "vier".into())), new: Some((6, "vier".into())) });
        assert_eq!(changed_span("zwei alt", "zwei neu"), ((5, 8), (5, 8)));
    }

    #[test]
    fn init_commit_history_restore() {
        let root = std::env::temp_dir().join(format!("nedit-git-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.tex"), "Version 1\n").unwrap();
        let mut g = GitState::default();
        g.refresh(&root, 0.0);
        assert!(!g.is_repo);
        // isolate from user config
        let _ = git(&root, &["init", "-b", "main"]);
        let _ = git(&root, &["config", "user.name", "Test"]);
        let _ = git(&root, &["config", "user.email", "t@e.st"]);
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-m", "Erste Version"]).unwrap();
        std::fs::write(root.join("main.tex"), "Version 2\n").unwrap();
        g.refresh(&root, 0.0);
        assert!(g.is_repo);
        assert_eq!(g.changes.len(), 1);
        g.message = "Zweite".into();
        g.commit(&root).unwrap();
        g.refresh(&root, 0.0);
        assert!(g.changes.is_empty());
        assert_eq!(g.log.len(), 2);
        let first = g.log[1].hash.clone();
        g.open_commit(&root, &g.log[0].hash.clone());
        assert!(g.view_lines.iter().any(|(k, l)| *k == DiffKind::Add && l == "+Version 2"));
        std::fs::write(root.join("main.tex"), "Version 2\nneu\n").unwrap();
        g.refresh(&root, 0.0);
        assert_eq!(g.file_status("main.tex"), Some('M'));
        let m = g.line_marks(&root, "main.tex", 1);
        assert!(m.iter().any(|x| x.kind == MarkKind::Added && x.start == 2), "{m:?}");
        // revert: modified + untracked file in a new folder + staged new file + deleted file
        std::fs::write(root.join("a.tex"), "A\n").unwrap();
        git(&root, &["add", "a.tex"]).unwrap();
        git(&root, &["commit", "-qm", "a"]).unwrap();
        std::fs::write(root.join("main.tex"), "kaputt\n").unwrap();
        std::fs::create_dir_all(root.join("neu")).unwrap();
        std::fs::write(root.join("neu/x.tex"), "x").unwrap();
        std::fs::write(root.join("staged.tex"), "s").unwrap();
        git(&root, &["add", "staged.tex"]).unwrap();
        std::fs::remove_file(root.join("a.tex")).unwrap();
        g.refresh(&root, 0.0);
        assert_eq!(g.changes.len(), 4, "{:?}", g.changes);
        let affected = g.revert(&root, &["neu".to_string()]).unwrap();
        assert_eq!(affected, vec!["neu/x.tex".to_string()]);
        assert!(!root.join("neu").exists(), "empty folder removed");
        g.refresh(&root, 0.0);
        let affected = g.revert(&root, &[String::new()]).unwrap();
        assert_eq!(affected.len(), 3);
        g.refresh(&root, 0.0);
        assert!(g.changes.is_empty(), "{:?}", g.changes);
        assert!(root.join("a.tex").exists() && !root.join("staged.tex").exists());
        assert_eq!(std::fs::read_to_string(root.join("main.tex")).unwrap().replace("\r\n", "\n"), "Version 2\n");
        GitState::restore_file(&root, &first, "main.tex").unwrap();
        // git may convert line endings on Windows (core.autocrlf)
        assert_eq!(std::fs::read_to_string(root.join("main.tex")).unwrap().replace("\r\n", "\n"), "Version 1\n");
        let _ = std::fs::remove_dir_all(root);
    }
}
