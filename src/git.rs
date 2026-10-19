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
    /// A stash entry, e.g. "stash@{0}".
    Stash(String),
}

/// A local branch, or a remote branch without a local counterpart.
#[derive(Clone, Debug, Default)]
pub struct Branch {
    pub name: String, // "main", remote: "origin/feature"
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// upstream was deleted on the remote
    pub gone: bool,
    /// fully contained in HEAD (safe to delete)
    pub merged: bool,
    pub time: i64,
    pub subject: String,
}

impl Branch {
    /// Local branch name ("origin/feature" → "feature").
    pub fn local_name(&self) -> &str {
        if self.remote { self.name.split_once('/').map_or(self.name.as_str(), |(_, n)| n) } else { &self.name }
    }
}

#[derive(Clone, Debug)]
pub struct StashFile {
    pub code: char, // A / M / D
    pub path: String,
    pub added: usize,
    pub deleted: usize,
}

#[derive(Clone, Debug)]
pub struct Stash {
    pub refname: String, // "stash@{0}"
    /// commit the changes were parked on
    pub base: String,
    pub branch: String,
    /// own description; empty for "WIP" stashes
    pub message: String,
    /// parked automatically when switching away from `branch`
    pub auto: bool,
    pub time: i64,
    pub files: Vec<StashFile>,
}

impl Stash {
    pub fn added(&self) -> usize {
        self.files.iter().map(|f| f.added).sum()
    }
    pub fn deleted(&self) -> usize {
        self.files.iter().map(|f| f.deleted).sum()
    }
    pub fn title(&self) -> String {
        if !self.message.is_empty() {
            self.message.clone()
        } else if self.auto {
            trf!("Beim Wechsel von {} geparkt" | "Parked when leaving {}", self.branch)
        } else {
            trf!("Zwischenstand auf {}" | "Work in progress on {}", self.branch)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub branch: String,
    pub base: String,
    pub author: String,
    pub draft: bool,
    /// "APPROVED", "CHANGES_REQUESTED", "REVIEW_REQUIRED" or ""
    pub review: String,
    pub url: String,
    pub updated: i64,
    pub additions: usize,
    pub deletions: usize,
}

/// Pull requests come from the GitHub CLI (`gh`).
#[derive(Clone, Debug, PartialEq)]
pub enum PrError {
    NoGh,
    Failed(String),
}

/// Where a switch goes: a branch, or the branch of a pull request.
#[derive(Clone, Debug)]
pub enum SwitchTarget {
    Branch(Branch),
    Pr(u64),
}

/// Stash message used when changes are parked by a branch switch.
const PARK: &str = "nedit-park:";

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
    pub branches: Vec<Branch>,
    pub stashes: Vec<Stash>,
    /// target for new pull requests ("main")
    pub default_branch: String,
    /// a merge stopped with conflicts
    pub merging: bool,
    pub prs: Vec<PullRequest>,
    pub pr_error: Option<PrError>,
    pub pr_loading: bool,
    /// PR list should be (re)loaded when the PR view is shown
    pub pr_stale: bool,
    /// the running background job may have changed files on disk
    pub touched_files: bool,
    marks: std::collections::HashMap<String, (u64, String, Vec<LineMark>)>,
    rx: Option<Receiver<Result<String, String>>>,
    status_rx: Option<Receiver<StatusSnapshot>>,
    pr_rx: Option<Receiver<Result<Vec<PullRequest>, PrError>>>,
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
            branches: vec![],
            stashes: vec![],
            default_branch: "main".into(),
            merging: false,
            prs: vec![],
            pr_error: None,
            pr_loading: false,
            pr_stale: true,
            touched_files: false,
            marks: Default::default(),
            rx: None,
            status_rx: None,
            pr_rx: None,
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

/// GitHub CLI, used for pull requests.
fn gh(root: &Path, args: &[&str]) -> Result<String, PrError> {
    let out = crate::platform::cmd("gh")
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()
        .map_err(|_| PrError::NoGh)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(PrError::Failed(if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err }))
    }
}

fn gh_msg(e: PrError) -> String {
    match e {
        PrError::NoGh => tr!("Die GitHub-CLI „gh“ ist nicht installiert." | "The GitHub CLI “gh” is not installed.").into(),
        PrError::Failed(e) => e,
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
            self.refresh_details(root);
        }
    }

    /// History, branches and stashes (after the working tree or HEAD changed).
    pub fn refresh_details(&mut self, root: &Path) {
        self.refresh_log(root);
        self.branches = list_branches(root);
        self.stashes = list_stashes(root);
        self.default_branch = self.find_default_branch(root);
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
                let branch_before = self.branch.clone();
                self.apply(snap);
                let after: Vec<(String, String)> = self.changes.iter().map(|c| (c.code.clone(), c.path.clone())).collect();
                before != after || head_before != self.head || branch_before != self.branch
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
            self.branches.clear();
            self.stashes.clear();
            return;
        }
        self.merging = s.merging;
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
        let root = root.to_path_buf();
        if pull {
            self.run_bg(tr!("Hole Änderungen …" | "Fetching changes …"), true, ctx, move || {
                git(&root, &["pull", "--rebase", "--autostash"]).map(|_| tr!("Aktualisiert" | "Updated").to_string())
            });
        } else {
            self.run_bg(tr!("Lade hoch …" | "Uploading …"), false, ctx, move || {
                git(&root, &["push", "-u", "origin", "HEAD"]).map(|_| tr!("Hochgeladen" | "Uploaded").to_string())
            });
        }
    }

    /// Update remote branches without touching the working tree.
    pub fn fetch(&mut self, root: &Path, ctx: egui::Context) {
        let root = root.to_path_buf();
        self.run_bg(tr!("Frage Remote ab …" | "Fetching remote …"), false, ctx, move || {
            git(&root, &["fetch", "--all", "--prune"]).map(|_| tr!("Remote abgefragt" | "Remote fetched").to_string())
        });
    }

    /// Run a (network) job in the background; the result arrives via `poll`.
    fn run_bg(&mut self, status: &str, touches_files: bool, ctx: egui::Context, job: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let (tx, rx) = channel();
        self.busy = true;
        self.touched_files = touches_files;
        self.status = status.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    /// Returns a finished background result, if any.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let r = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        self.busy = false;
        self.pr_stale = true;
        Some(r)
    }

    // ───────────── branches ─────────────

    /// Create a branch from HEAD and switch to it; uncommitted changes come along.
    pub fn create_branch(&self, root: &Path, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() || git(root, &["check-ref-format", "--branch", name]).is_err() {
            return Err(trf!("„{name}“ ist kein gültiger Branch-Name (keine Leerzeichen, kein .. ~ ^ : ?)" | "“{name}” is not a valid branch name (no spaces, no .. ~ ^ : ?)"));
        }
        if self.branches.iter().any(|b| !b.remote && b.name == name) {
            return Err(trf!("Branch „{name}“ gibt es schon" | "Branch “{name}” already exists"));
        }
        git(root, &["switch", "-c", name])?;
        Ok(if self.changes.is_empty() {
            trf!("Branch {name} angelegt" | "Branch {name} created")
        } else {
            trf!("Branch {name} angelegt – deine Änderungen sind mitgekommen" | "Branch {name} created – your changes came along")
        })
    }

    /// Switch to a local branch or check out a remote one ("origin/x" → tracking branch "x").
    /// With `park`, uncommitted changes are stashed for the current branch and come back
    /// automatically when switching back to it.
    pub fn switch_branch(&self, root: &Path, target: &Branch, park: bool) -> Result<String, String> {
        let local = target.local_name().to_string();
        if local == self.branch {
            return Ok(String::new());
        }
        let has_local = self.branches.iter().any(|b| !b.remote && b.name == local);
        let remote = target.remote && !has_local;
        let name = target.name.clone();
        switch_with(root, &self.branch, park && !self.changes.is_empty(), || {
            if remote { git(root, &["switch", "--track", &name]) } else { git(root, &["switch", &local]) }
        })
    }

    pub fn delete_branch(root: &Path, name: &str) -> Result<String, String> {
        git(root, &["branch", "-D", name]).map(|_| trf!("Branch {name} gelöscht" | "Branch {name} deleted"))
    }

    /// Merge `name` into the current branch. Conflicts leave the repository in merge state.
    pub fn merge_branch(&self, root: &Path, name: &str) -> Result<String, String> {
        match git(root, &["merge", "--no-edit", name]) {
            Ok(out) if out.contains("Already up to date") => Ok(trf!("{} enthält {name} schon" | "{} already contains {name}", self.branch)),
            Ok(_) => Ok(trf!("{name} in {} übernommen" | "Merged {name} into {}", self.branch)),
            Err(e) if git(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok() => {
                Err(trf!("Konflikte beim Zusammenführen – Dateien mit „U“ bearbeiten und committen, oder Merge abbrechen. ({})" | "Merge conflicts – edit the files marked “U” and commit, or abort the merge. ({})", e.lines().last().unwrap_or("")))
            }
            Err(e) => Err(e),
        }
    }

    pub fn abort_merge(root: &Path) -> Result<String, String> {
        git(root, &["merge", "--abort"]).map(|_| tr!("Merge abgebrochen" | "Merge aborted").to_string())
    }

    /// Default branch of the remote (target for pull requests).
    fn find_default_branch(&self, root: &Path) -> String {
        if let Ok(s) = git(root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
            if let Some((_, b)) = s.trim().split_once('/') {
                return b.to_string();
            }
        }
        ["main", "master"].iter().find(|n| self.branches.iter().any(|b| !b.remote && b.name == **n)).unwrap_or(&"main").to_string()
    }

    // ───────────── stash ─────────────

    /// Park all uncommitted changes (including new files).
    pub fn stash_push(&self, root: &Path, message: &str) -> Result<String, String> {
        let mut args = vec!["stash", "push", "--include-untracked"];
        if !message.trim().is_empty() {
            args.extend(["-m", message.trim()]);
        }
        git(root, &args).map(|_| tr!("Änderungen geparkt" | "Changes parked").to_string())
    }

    /// Bring parked changes back; `keep` leaves the entry in the stash.
    pub fn stash_apply(root: &Path, refname: &str, keep: bool) -> Result<String, String> {
        git(root, &["stash", if keep { "apply" } else { "pop" }, refname]).map(|_| tr!("Geparkte Änderungen zurückgeholt" | "Parked changes restored").to_string())
    }

    pub fn stash_drop(root: &Path, refname: &str) -> Result<String, String> {
        git(root, &["stash", "drop", refname]).map(|_| tr!("Geparkte Änderungen gelöscht" | "Parked changes deleted").to_string())
    }

    pub fn open_stash(&mut self, root: &Path, refname: &str) {
        let text = git(root, &["stash", "show", "-p", "--include-untracked", refname])
            .or_else(|_| git(root, &["stash", "show", "-p", refname]))
            .unwrap_or_else(|e| e);
        self.view_lines = parse_diff(&text);
        let st = self.stashes.iter().find(|s| s.refname == refname);
        self.view_files = st.map(|s| s.files.iter().map(|f| Change { code: f.code.to_string(), path: f.path.clone() }).collect()).unwrap_or_default();
        self.view_title = st.map(|s| s.title()).unwrap_or_else(|| refname.to_string());
        self.view = Some(GitView::Stash(refname.to_string()));
    }

    // ───────────── pull requests (GitHub CLI) ─────────────

    pub fn load_prs(&mut self, root: &Path, ctx: &egui::Context) {
        if self.pr_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        let ctx = ctx.clone();
        self.pr_loading = true;
        self.pr_stale = false;
        std::thread::spawn(move || {
            let r = gh(&root, &["pr", "list", "--state", "open", "--limit", "50", "--json", "number,title,headRefName,baseRefName,author,isDraft,reviewDecision,url,updatedAt,additions,deletions"])
                .and_then(|s| parse_prs(&s).map_err(PrError::Failed));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.pr_rx = Some(rx);
    }

    pub fn poll_prs(&mut self) {
        let Some(rx) = &self.pr_rx else { return };
        match rx.try_recv() {
            Ok(r) => {
                self.pr_rx = None;
                self.pr_loading = false;
                match r {
                    Ok(v) => {
                        self.prs = v;
                        self.pr_error = None;
                    }
                    Err(e) => self.pr_error = Some(e),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pr_rx = None;
                self.pr_loading = false;
            }
            Err(_) => {}
        }
    }

    /// Open pull request of the current branch.
    pub fn current_pr(&self) -> Option<&PullRequest> {
        self.prs.iter().find(|p| p.branch == self.branch)
    }

    /// Check out the branch of a pull request (parking changes like a branch switch).
    pub fn checkout_pr(&mut self, root: &Path, number: u64, park: bool, ctx: egui::Context) {
        let root = root.to_path_buf();
        let current = self.branch.clone();
        let park = park && !self.changes.is_empty();
        self.run_bg(tr!("Hole Pull Request …" | "Checking out pull request …"), true, ctx, move || {
            switch_with(&root, &current, park, || gh(&root, &["pr", "checkout", &number.to_string()]).map_err(gh_msg))
        });
    }

    /// Push the current branch and open a pull request for it.
    pub fn create_pr(&mut self, root: &Path, title: String, body: String, base: String, draft: bool, ctx: egui::Context) {
        let root = root.to_path_buf();
        self.run_bg(tr!("Erstelle Pull Request …" | "Creating pull request …"), false, ctx, move || {
            git(&root, &["push", "-u", "origin", "HEAD"])?;
            let mut args = vec!["pr", "create", "--title", &title, "--body", &body, "--base", &base];
            if draft {
                args.push("--draft");
            }
            let url = gh(&root, &args).map_err(gh_msg)?;
            let n = url.trim().rsplit('/').next().unwrap_or("").to_string();
            Ok(trf!("Pull Request #{n} erstellt" | "Pull request #{n} created"))
        });
    }

    pub fn merge_pr(&mut self, root: &Path, number: u64, squash: bool, delete_branch: bool, ctx: egui::Context) {
        let root = root.to_path_buf();
        self.run_bg(tr!("Führe Pull Request zusammen …" | "Merging pull request …"), true, ctx, move || {
            let n = number.to_string();
            let mut args = vec!["pr", "merge", &n, if squash { "--squash" } else { "--merge" }];
            if delete_branch {
                args.push("--delete-branch");
            }
            gh(&root, &args).map_err(gh_msg)?;
            Ok(trf!("Pull Request #{number} zusammengeführt" | "Pull request #{number} merged"))
        });
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
    merging: bool,
}

/// Park changes of `current` (if asked), run the switch, then bring back changes that were
/// parked on the branch we arrive at. A failed switch restores the parked changes.
fn switch_with(root: &Path, current: &str, park: bool, switch: impl FnOnce() -> Result<String, String>) -> Result<String, String> {
    if park {
        git(root, &["stash", "push", "--include-untracked", "-m", &format!("{PARK}{current}")])?;
    }
    if let Err(e) = switch() {
        if park {
            let _ = git(root, &["stash", "pop"]);
        }
        return Err(e);
    }
    let now = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut msg = trf!("Gewechselt zu {now}" | "Switched to {now}");
    if park {
        msg += &trf!(" · Änderungen von {current} geparkt" | " · changes of {current} parked");
    }
    if let Some(r) = find_parked(root, &now) {
        match git(root, &["stash", "pop", &r]) {
            Ok(_) => msg += tr!(" · geparkte Änderungen zurückgeholt" | " · parked changes restored"),
            Err(_) => msg += tr!(" · geparkte Änderungen haben Konflikte – bitte prüfen" | " · parked changes have conflicts – please check"),
        }
    }
    Ok(msg)
}

/// Stash that was parked automatically when leaving `branch`.
fn find_parked(root: &Path, branch: &str) -> Option<String> {
    let want = format!("{PARK}{branch}");
    git(root, &["stash", "list", "--format=%gd%x1f%gs"]).ok()?.lines().find_map(|l| {
        let (r, gs) = l.split_once('\u{1f}')?;
        (gs.rsplit_once(": ").map(|(_, m)| m) == Some(want.as_str())).then(|| r.to_string())
    })
}

fn list_branches(root: &Path) -> Vec<Branch> {
    let merged: std::collections::HashSet<String> =
        git(root, &["for-each-ref", "--merged", "HEAD", "--format=%(refname:short)", "refs/heads"]).map(|s| s.lines().map(String::from).collect()).unwrap_or_default();
    let fmt = "--format=%(refname)%1f%(refname:short)%1f%(HEAD)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(committerdate:unix)%1f%(contents:subject)";
    let out = git(root, &["for-each-ref", "--sort=-committerdate", fmt, "refs/heads", "refs/remotes"]).unwrap_or_default();
    let mut v: Vec<Branch> = out
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            if p.len() < 7 || p[0].ends_with("/HEAD") {
                return None;
            }
            let track = p[4];
            let num = |key: &str| track.split(", ").find_map(|s| s.strip_prefix(key)).and_then(|n| n.trim().parse().ok()).unwrap_or(0);
            Some(Branch {
                name: p[1].to_string(),
                remote: p[0].starts_with("refs/remotes/"),
                current: p[2] == "*",
                upstream: (!p[3].is_empty()).then(|| p[3].to_string()),
                ahead: num("ahead "),
                behind: num("behind "),
                gone: track == "gone",
                merged: merged.contains(p[1]),
                time: p[5].parse().unwrap_or(0),
                subject: p[6].to_string(),
            })
        })
        .collect();
    // remote branches only where there is no local branch for them
    let local: std::collections::HashSet<String> = v.iter().filter(|b| !b.remote).flat_map(|b| [Some(b.name.clone()), b.upstream.clone()]).flatten().collect();
    v.retain(|b| !b.remote || !(local.contains(&b.name) || local.contains(b.local_name())));
    v
}

fn list_stashes(root: &Path) -> Vec<Stash> {
    let out = git(root, &["stash", "list", "--format=%gd%x1f%P%x1f%ct%x1f%gs"]).unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            if p.len() < 4 {
                return None;
            }
            // "WIP on main: 1a2b3c4 subject" | "On main: message"
            let gs = p[3];
            let wip = gs.starts_with("WIP on ");
            let rest = gs.strip_prefix("WIP on ").or_else(|| gs.strip_prefix("On ")).unwrap_or(gs);
            let (branch, msg) = rest.split_once(": ").unwrap_or(("", rest));
            let auto = msg.starts_with(PARK);
            Some(Stash {
                refname: p[0].to_string(),
                base: p[1].split(' ').next().unwrap_or("").to_string(),
                branch: branch.to_string(),
                message: if wip || auto { String::new() } else { msg.to_string() },
                auto,
                time: p[2].parse().unwrap_or(0),
                files: stash_files(root, p[0]),
            })
        })
        .collect()
}

fn stash_files(root: &Path, refname: &str) -> Vec<StashFile> {
    // --include-untracked for `stash show` needs git 2.32
    let show = |what: &str| git(root, &["stash", "show", "--include-untracked", what, refname]).or_else(|_| git(root, &["stash", "show", what, refname])).unwrap_or_default();
    let counts: std::collections::HashMap<String, (usize, usize)> = show("--numstat")
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let a = it.next()?.parse().unwrap_or(0);
            let d = it.next()?.parse().unwrap_or(0);
            Some((it.next()?.to_string(), (a, d)))
        })
        .collect();
    show("--name-status")
        .lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let code = it.next()?.chars().next()?;
            let path = it.last()?.to_string();
            let (added, deleted) = counts.get(&path).copied().unwrap_or((0, 0));
            Some(StashFile { code, path, added, deleted })
        })
        .collect()
}

fn parse_prs(json: &str) -> Result<Vec<PullRequest>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let s = |x: &serde_json::Value, k: &str| x.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let n = |x: &serde_json::Value, k: &str| x.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .map(|p| PullRequest {
                    number: n(p, "number"),
                    title: s(p, "title"),
                    branch: s(p, "headRefName"),
                    base: s(p, "baseRefName"),
                    author: p.get("author").map(|a| s(a, "login")).unwrap_or_default(),
                    draft: p.get("isDraft").and_then(|v| v.as_bool()).unwrap_or(false),
                    review: s(p, "reviewDecision"),
                    url: s(p, "url"),
                    updated: chrono::DateTime::parse_from_rfc3339(&s(p, "updatedAt")).map(|d| d.timestamp()).unwrap_or(0),
                    additions: n(p, "additions") as usize,
                    deletions: n(p, "deletions") as usize,
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Collect repository state (runs on a worker thread for background refreshes).
fn status_snapshot(root: &Path) -> StatusSnapshot {
    let mut s = StatusSnapshot { git_available: true, is_repo: false, branch: String::new(), remote: None, ahead: 0, behind: 0, changes: vec![], head: String::new(), merging: false };
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
    s.merging = git(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok();
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
    fn branches_park_and_stash() {
        let root = std::env::temp_dir().join(format!("nedit-git-br-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.tex"), "eins\n").unwrap();
        let _ = git(&root, &["init", "-b", "main"]);
        let _ = git(&root, &["config", "user.name", "Test"]);
        let _ = git(&root, &["config", "user.email", "t@e.st"]);
        git(&root, &["add", "-A"]).unwrap();
        git(&root, &["commit", "-qm", "Erste Version"]).unwrap();
        let mut g = GitState::default();
        g.refresh(&root, 0.0);
        assert!(g.create_branch(&root, "kein gültiger name").is_err());
        // new branch takes the uncommitted change along
        std::fs::write(root.join("main.tex"), "eins\nkapitel\n").unwrap();
        g.refresh(&root, 0.0);
        g.create_branch(&root, "kapitel-2").unwrap();
        g.refresh(&root, 0.0);
        assert_eq!(g.branch, "kapitel-2");
        assert_eq!(g.changes.len(), 1);
        assert!(g.create_branch(&root, "kapitel-2").is_err());
        // switching with park: work is stashed for kapitel-2, main is clean
        std::fs::write(root.join("neu.tex"), "neu\n").unwrap();
        g.refresh(&root, 0.0);
        let main = g.branches.iter().find(|b| b.name == "main").unwrap().clone();
        g.switch_branch(&root, &main, true).unwrap();
        g.refresh(&root, 0.0);
        assert_eq!(g.branch, "main");
        assert!(g.changes.is_empty(), "{:?}", g.changes);
        assert_eq!(g.stashes.len(), 1);
        let s = &g.stashes[0];
        assert!(s.auto && s.branch == "kapitel-2", "{s:?}");
        assert_eq!(s.files.len(), 2, "{:?}", s.files);
        assert!(s.files.iter().any(|f| f.path == "neu.tex" && f.code == 'A' && f.added == 1));
        assert!(s.files.iter().any(|f| f.path == "main.tex" && f.code == 'M' && f.added == 1));
        g.open_stash(&root, "stash@{0}");
        assert!(g.view_lines.iter().any(|(k, l)| *k == DiffKind::Add && l == "+kapitel"));
        // coming back restores the parked work automatically
        let k2 = g.branches.iter().find(|b| b.name == "kapitel-2").unwrap().clone();
        let msg = g.switch_branch(&root, &k2, true).unwrap();
        assert!(msg.contains("kapitel-2"), "{msg}");
        g.refresh(&root, 0.0);
        assert!(g.stashes.is_empty());
        assert_eq!(g.changes.len(), 2);
        assert_eq!(std::fs::read_to_string(root.join("main.tex")).unwrap().replace("\r\n", "\n"), "eins\nkapitel\n");
        // manual stash with message, apply keeps it, drop removes it
        g.stash_push(&root, "Idee für später").unwrap();
        g.refresh(&root, 0.0);
        assert!(g.changes.is_empty());
        assert_eq!(g.stashes[0].title(), "Idee für später");
        GitState::stash_apply(&root, "stash@{0}", true).unwrap();
        g.refresh(&root, 0.0);
        assert_eq!(g.stashes.len(), 1);
        assert_eq!(g.changes.len(), 2);
        GitState::stash_drop(&root, "stash@{0}").unwrap();
        // commit on the branch, merge into main
        g.commit(&root).unwrap();
        g.refresh(&root, 0.0);
        let k2 = g.branches.iter().find(|b| b.name == "kapitel-2").unwrap();
        assert!(k2.current && !k2.remote);
        let main = g.branches.iter().find(|b| b.name == "main").unwrap().clone();
        assert!(main.merged);
        g.switch_branch(&root, &main, true).unwrap();
        g.refresh(&root, 0.0);
        assert!(!g.branches.iter().find(|b| b.name == "kapitel-2").unwrap().merged);
        g.merge_branch(&root, "kapitel-2").unwrap();
        g.refresh(&root, 0.0);
        assert!(root.join("neu.tex").exists() && !g.merging);
        assert!(g.branches.iter().find(|b| b.name == "kapitel-2").unwrap().merged);
        GitState::delete_branch(&root, "kapitel-2").unwrap();
        g.refresh(&root, 0.0);
        assert_eq!(g.branches.len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pr_json() {
        let prs = parse_prs(r#"[{"number":7,"title":"Kapitel 2","headRefName":"kapitel-2","baseRefName":"main","author":{"login":"nov"},"isDraft":true,"reviewDecision":"APPROVED","url":"https://github.com/a/b/pull/7","updatedAt":"2026-10-19T08:00:00Z","additions":12,"deletions":3}]"#).unwrap();
        assert_eq!(prs.len(), 1);
        let p = &prs[0];
        assert_eq!((p.number, p.branch.as_str(), p.author.as_str(), p.draft, p.additions, p.deletions), (7, "kapitel-2", "nov", true, 12, 3));
        assert!(p.updated > 0);
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
