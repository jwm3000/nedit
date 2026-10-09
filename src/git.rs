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
            "??" => ("Neu", 'N'),
            _ if c.contains('D') => ("Gelöscht", 'D'),
            _ if c.contains('A') => ("Hinzugefügt", 'A'),
            _ if c.contains('R') => ("Umbenannt", 'R'),
            _ if c.contains('U') => ("Konflikt", 'U'),
            _ => ("Geändert", 'M'),
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
        .map_err(|e| format!("git nicht gefunden: {e}"))?;
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
        if crate::platform::cmd("git").arg("--version").output().is_err() {
            self.git_available = false;
            return;
        }
        let canon = root.canonicalize().unwrap_or(root.to_path_buf());
        self.is_repo = repo_root(root).is_some_and(|r| r.canonicalize().unwrap_or(r) == canon);
        if !self.is_repo {
            self.changes.clear();
            self.log.clear();
            return;
        }
        self.branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_else(|_| "main".into());
        self.remote = git(root, &["remote"]).ok().and_then(|s| s.lines().next().map(String::from));
        let (mut ahead, mut behind) = (0, 0);
        if let Ok(s) = git(root, &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"]) {
            let v: Vec<usize> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() == 2 {
                behind = v[0];
                ahead = v[1];
            }
        }
        self.ahead = ahead;
        self.behind = behind;
        self.changes = git(root, &["status", "--porcelain=v1", "-uall"])
            .map(|s| {
                s.lines()
                    .filter(|l| l.len() > 3)
                    .map(|l| {
                        let path = l[3..].trim().trim_matches('"').to_string();
                        let path = path.rsplit(" -> ").next().unwrap_or(&path).to_string();
                        Change { code: l[..2].to_string(), path }
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.head = git(root, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default();
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
        git(root, &["commit", "-m", "Erste Version"])?;
        Ok(())
    }

    pub fn commit(&mut self, root: &Path) -> Result<(), String> {
        let msg = if self.message.trim().is_empty() { format!("Stand vom {}", chrono_like_now()) } else { self.message.trim().to_string() };
        if self.excluded.is_empty() {
            git(root, &["add", "-A"])?;
            git(root, &["commit", "-m", &msg])?;
        } else {
            let sel: Vec<String> = self.changes.iter().filter(|c| !self.excluded.contains(&c.path)).map(|c| c.path.clone()).collect();
            if sel.is_empty() {
                return Err("Keine Datei ausgewählt".into());
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
        self.status = if pull { "Hole Änderungen …".into() } else { "Lade hoch …".into() };
        std::thread::spawn(move || {
            let r = if pull {
                git(&root, &["pull", "--rebase", "--autostash"]).map(|_| "Aktualisiert".to_string())
            } else {
                git(&root, &["push", "-u", "origin", "HEAD"]).map(|_| "Hochgeladen".to_string())
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

    pub fn discard_file(&self, root: &Path, path: &str) -> Result<(), String> {
        if self.changes.iter().any(|c| c.path == path && c.code == "??") {
            std::fs::remove_file(root.join(path)).map_err(|e| e.to_string())
        } else {
            git(root, &["checkout", "HEAD", "--", path]).map(|_| ())
        }
    }
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
        GitState::restore_file(&root, &first, "main.tex").unwrap();
        // git may convert line endings on Windows (core.autocrlf)
        assert_eq!(std::fs::read_to_string(root.join("main.tex")).unwrap().replace("\r\n", "\n"), "Version 1\n");
        let _ = std::fs::remove_dir_all(root);
    }
}
