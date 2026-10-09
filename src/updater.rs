//! Self-update from GitHub releases (github.com/jwm3000/nedit).
//!
//! Each release carries a plain binary per platform (`nedit-linux-x86_64`,
//! `nedit-macos-arm64`, `nedit-windows-x86_64.exe`). The updater downloads the one for
//! this platform and swaps it in place of the running executable.

use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

pub const REPO: &str = "jwm3000/nedit";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub notes: String,
    pub html_url: String,
    pub asset_url: Option<String>,
}

pub enum UpdMsg {
    Checked { manual: bool, result: Result<Option<Release>, String> },
    Progress(u64, u64),
    Installed(Result<(), String>),
}

#[derive(Default)]
pub struct Updater {
    pub available: Option<Release>,
    pub checking: bool,
    pub status: String,
    pub progress: Option<(u64, u64)>,
    pub installing: bool,
    pub installed: bool,
    pub last_check_ok: bool,
    rx: Option<Receiver<UpdMsg>>,
    tx: Option<Sender<UpdMsg>>,
}

fn parse_version(s: &str) -> (u64, u64, u64) {
    let mut it = s.trim().trim_start_matches('v').split(['.', '-']).map(|p| p.parse::<u64>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

pub fn is_newer(tag: &str) -> bool {
    parse_version(tag) > parse_version(VERSION)
}

/// Release asset name for this platform, if we publish one.
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("nedit-linux-x86_64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("nedit-macos-arm64")
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some("nedit-windows-x86_64.exe")
    } else {
        None
    }
}

/// Running from a source checkout (`cargo run` / target dir)? Then updating means `git pull`.
pub fn source_checkout() -> Option<PathBuf> {
    let exe = exe_path()?;
    let mut dir = exe.parent()?;
    while let Some(p) = dir.parent() {
        if dir.file_name().is_some_and(|n| n == "target") && p.join("Cargo.toml").exists() && p.join(".git").exists() {
            return Some(p.to_path_buf());
        }
        dir = p;
    }
    None
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(600))).build().into()
}

fn fetch_latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = agent()
        .get(&url)
        .header("User-Agent", "nEdit-updater")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("GitHub nicht erreichbar: {e}"))?;
    let txt = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("Keine Releases gefunden")?.to_string();
    let asset_url = asset_name().and_then(|name| {
        v["assets"].as_array()?.iter().find(|a| a["name"].as_str() == Some(name)).and_then(|a| a["browser_download_url"].as_str()).map(String::from)
    });
    Ok(Release {
        tag,
        notes: v["body"].as_str().unwrap_or("").to_string(),
        html_url: v["html_url"].as_str().unwrap_or("").to_string(),
        asset_url,
    })
}

/// Put the new binary in place of the running one.
fn replace_exe(bytes: &[u8]) -> Result<(), String> {
    let exe = exe_path().ok_or("Programmpfad unbekannt")?;
    let tmp = exe.with_extension("update-new");
    std::fs::write(&tmp, bytes).map_err(|e| format!("Kann nicht nach {} schreiben: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    #[cfg(windows)]
    {
        // a running .exe can be renamed but not overwritten
        let old = exe.with_extension("old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &exe).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Austauschen fehlgeschlagen: {e}")
    })
}

static EXE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Path of our executable, captured at startup. After an in-place update Linux reports
/// the *old* inode as "… (deleted)", so we must not ask again later.
pub fn exe_path() -> Option<PathBuf> {
    if let Some(p) = EXE.get() {
        return Some(p.clone());
    }
    let p = std::env::current_exe().ok()?;
    let s = p.to_string_lossy();
    let p = PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s));
    let p = p.canonicalize().unwrap_or(p);
    Some(EXE.get_or_init(|| p).clone())
}

/// Remove leftovers of a previous Windows update.
pub fn cleanup() {
    let _ = exe_path();
    if let Some(exe) = exe_path() {
        let _ = std::fs::remove_file(exe.with_extension("old.exe"));
        let _ = std::fs::remove_file(exe.with_extension("update-new"));
    }
}

/// Start the (new) executable as a detached process; the caller then closes the window.
pub fn restart() -> Result<(), String> {
    let exe = exe_path().ok_or("Programmpfad unbekannt")?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // survive the parent closing
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("Neustart fehlgeschlagen ({}): {e}", exe.display()))
}

/// Download a release asset; `progress(done, total)` is called while reading.
fn download(url: &str, progress: &dyn Fn(u64, u64)) -> Result<Vec<u8>, String> {
    let mut resp = agent().get(url).header("User-Agent", "nEdit-updater").call().map_err(|e| format!("Download fehlgeschlagen: {e}"))?;
    let total: u64 = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut reader = resp.body_mut().with_config().limit(500 * 1024 * 1024).reader();
    let mut bytes = Vec::with_capacity(total as usize);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        progress(bytes.len() as u64, total);
    }
    if bytes.len() < 1024 * 1024 || (total > 0 && bytes.len() as u64 != total) {
        return Err("Download unvollständig".into());
    }
    Ok(bytes)
}

/// `nedit --update`: check and install from the command line.
pub fn cli_update() -> Result<String, String> {
    if let Some(src) = source_checkout() {
        return Err(format!("nEdit läuft aus dem Quellcode – aktualisieren mit: cd {} && git pull && ./install.sh", src.display()));
    }
    let rel = fetch_latest()?;
    if !is_newer(&rel.tag) {
        return Ok(format!("nEdit {VERSION} ist aktuell (neuestes Release: {})", rel.tag));
    }
    let url = rel.asset_url.ok_or("Für diese Plattform gibt es kein fertiges Programm")?;
    eprintln!("Lade {} …", rel.tag);
    let bytes = download(&url, &|d, t| {
        if t > 0 {
            eprint!("\r  {:>5.1} / {:.1} MB", d as f64 / 1e6, t as f64 / 1e6);
        }
    })?;
    eprintln!();
    replace_exe(&bytes)?;
    Ok(format!("nEdit wurde von {VERSION} auf {} aktualisiert", rel.tag))
}

impl Updater {
    fn channel(&mut self) -> Sender<UpdMsg> {
        if self.tx.is_none() {
            let (tx, rx) = channel();
            self.tx = Some(tx);
            self.rx = Some(rx);
        }
        self.tx.clone().unwrap()
    }

    pub fn check(&mut self, manual: bool, ctx: &egui::Context) {
        if self.checking {
            return;
        }
        self.checking = true;
        if manual {
            self.status = "Suche nach Updates …".into();
        }
        let tx = self.channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = fetch_latest().map(|r| is_newer(&r.tag).then_some(r));
            let _ = tx.send(UpdMsg::Checked { manual, result });
            ctx.request_repaint();
        });
    }

    pub fn install(&mut self, ctx: &egui::Context) {
        let Some(rel) = self.available.clone() else { return };
        let Some(url) = rel.asset_url.clone() else {
            self.status = "Für diese Plattform gibt es kein fertiges Programm – bitte aus dem Quellcode bauen.".into();
            return;
        };
        self.installing = true;
        self.progress = Some((0, 0));
        self.status = format!("Lade {} …", rel.tag);
        let tx = self.channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = download(&url, &|d, t| {
                let _ = tx.send(UpdMsg::Progress(d, t));
                ctx.request_repaint();
            })
            .and_then(|bytes| replace_exe(&bytes));
            let _ = tx.send(UpdMsg::Installed(result));
            ctx.request_repaint();
        });
    }

    /// Returns `true` if a new update was just discovered (so the UI can announce it).
    pub fn poll(&mut self) -> Option<String> {
        let rx = self.rx.as_ref()?;
        let mut announce = None;
        while let Ok(m) = rx.try_recv() {
            match m {
                UpdMsg::Checked { manual, result } => {
                    self.checking = false;
                    match result {
                        Ok(Some(r)) => {
                            self.status = format!("Version {} ist verfügbar", r.tag);
                            announce = Some(r.tag.clone());
                            self.available = Some(r);
                            self.last_check_ok = true;
                        }
                        Ok(None) => {
                            self.last_check_ok = true;
                            if manual {
                                self.status = format!("nEdit {VERSION} ist aktuell ✓");
                            }
                        }
                        Err(e) => {
                            if manual {
                                self.status = e;
                            }
                        }
                    }
                }
                UpdMsg::Progress(done, total) => self.progress = Some((done, total)),
                UpdMsg::Installed(r) => {
                    self.installing = false;
                    self.progress = None;
                    match r {
                        Ok(()) => {
                            self.installed = true;
                            self.status = "Update installiert – nEdit neu starten, um es zu verwenden.".into();
                        }
                        Err(e) => self.status = e,
                    }
                }
            }
        }
        announce
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(parse_version("v0.2.0") > parse_version("0.1.9"));
        assert!(parse_version("v1.0.0") > parse_version("0.10.3"));
        assert_eq!(parse_version("v0.1.0"), (0, 1, 0));
        assert!(!is_newer(&format!("v{VERSION}")));
    }
}
