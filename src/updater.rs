//! Self-update from GitHub releases (github.com/jwm3000/nedit).
//!
//! Each release carries a plain binary per platform (`nedit-linux-x86_64`,
//! `nedit-macos-universal`, `nedit-windows-x86_64.exe`). The updater downloads the one for
//! this platform and swaps it in place of the running executable. On macOS, when nEdit runs
//! as `nEdit.app`, the whole app bundle is replaced instead (`nEdit-macOS.zip`), so its
//! version, icon and signature stay consistent.

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

/// Release asset names for this platform, in order of preference.
pub fn asset_names() -> Vec<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        vec!["nedit-linux-x86_64"]
    } else if cfg!(target_os = "macos") {
        let mut v = if app_bundle().is_some() { vec![MAC_APP_ZIP] } else { vec![] };
        v.push("nedit-macos-universal");
        if cfg!(target_arch = "aarch64") {
            v.push("nedit-macos-arm64"); // releases before the universal build
        }
        v
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        vec!["nedit-windows-x86_64.exe"]
    } else {
        vec![]
    }
}

/// The zipped app bundle in each release (macOS).
pub const MAC_APP_ZIP: &str = "nEdit-macOS.zip";

/// `…/nEdit.app` when running from an app bundle (macOS).
pub fn app_bundle() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let exe = exe_path()?;
    let bundle = exe.parent()?.parent()?.parent()?;
    (exe.parent()?.ends_with("Contents/MacOS") && bundle.extension().is_some_and(|e| e == "app")).then(|| bundle.to_path_buf())
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
        .map_err(|e| trf!("GitHub nicht erreichbar: {e}" | "GitHub unreachable: {e}"))?;
    let txt = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or(tr!("Keine Releases gefunden" | "No releases found"))?.to_string();
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let asset_url = asset_names()
        .into_iter()
        .find_map(|name| assets.iter().find(|a| a["name"].as_str() == Some(name)).and_then(|a| a["browser_download_url"].as_str()).map(String::from));
    Ok(Release {
        tag,
        notes: v["body"].as_str().unwrap_or("").to_string(),
        html_url: v["html_url"].as_str().unwrap_or("").to_string(),
        asset_url,
    })
}

/// Install a downloaded update: the app bundle (macOS zip) or the bare binary.
fn install_update(url: &str, bytes: &[u8]) -> Result<(), String> {
    if url.ends_with(MAC_APP_ZIP) {
        if let Some(bundle) = app_bundle() {
            return replace_bundle(&bundle, bytes);
        }
    }
    replace_exe(bytes)
}

/// macOS: unpack `nEdit.app` next to the running one and swap the two folders.
fn replace_bundle(bundle: &std::path::Path, zip_bytes: &[u8]) -> Result<(), String> {
    let parent = bundle.parent().ok_or(tr!("Programmpfad unbekannt" | "Program path unknown"))?;
    let stage = parent.join(format!(".nEdit-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| trf!("Kann nicht nach {} schreiben: {e}" | "Cannot write to {}: {e}", parent.display()))?;
    let result = (|| {
        unzip_with_modes(zip_bytes, &stage)?;
        let new = std::fs::read_dir(&stage)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app") && p.join("Contents/MacOS/nedit").is_file())
            .ok_or(tr!("Das Update enthält keine nEdit.app" | "The update contains no nEdit.app"))?;
        let old = parent.join(format!(".nEdit-old-{}.app", std::process::id()));
        std::fs::rename(bundle, &old).map_err(|e| trf!("Austauschen fehlgeschlagen: {e}" | "Replacing failed: {e}"))?;
        if let Err(e) = std::fs::rename(&new, bundle) {
            let _ = std::fs::rename(&old, bundle);
            return Err(trf!("Austauschen fehlgeschlagen: {e}" | "Replacing failed: {e}"));
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// Unpack a zip keeping Unix permissions (the app's executable bit) and rejecting paths that
/// leave `dest`.
fn unzip_with_modes(bytes: &[u8], dest: &std::path::Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else { continue };
        if rel.components().next().is_some_and(|c| c.as_os_str() == "__MACOSX") {
            continue;
        }
        let out = dest.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut f, &mut w).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(mode) = f.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o7777));
        }
    }
    Ok(())
}

/// Put the new binary in place of the running one.
fn replace_exe(bytes: &[u8]) -> Result<(), String> {
    let exe = exe_path().ok_or(tr!("Programmpfad unbekannt" | "Program path unknown"))?;
    let tmp = exe.with_extension("update-new");
    std::fs::write(&tmp, bytes).map_err(|e| trf!("Kann nicht nach {} schreiben: {e}" | "Cannot write to {}: {e}", tmp.display()))?;
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
        trf!("Austauschen fehlgeschlagen: {e}" | "Replacing failed: {e}")
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

/// Remove leftovers of a previous update.
pub fn cleanup() {
    let _ = exe_path();
    if let Some(exe) = exe_path() {
        let _ = std::fs::remove_file(exe.with_extension("old.exe"));
        let _ = std::fs::remove_file(exe.with_extension("update-new"));
    }
    // macOS: an old bundle that could not be removed right after the swap
    if let Some(parent) = app_bundle().and_then(|b| b.parent().map(|p| p.to_path_buf())) {
        for e in std::fs::read_dir(parent).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with(".nEdit-old-") || n.starts_with(".nEdit-update-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Start the (new) executable as a detached process; the caller then closes the window.
pub fn restart() -> Result<(), String> {
    let exe = exe_path().ok_or(tr!("Programmpfad unbekannt" | "Program path unknown"))?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // survive the parent closing
    }
    cmd.spawn().map(|_| ()).map_err(|e| trf!("Neustart fehlgeschlagen ({}): {e}" | "Restart failed ({}): {e}", exe.display()))
}

/// GET a small text resource (GitHub API).
pub(crate) fn get_text(url: &str) -> Result<String, String> {
    let mut resp = agent()
        .get(url)
        .header("User-Agent", "nEdit")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| trf!("Nicht erreichbar: {e}" | "Unreachable: {e}"))?;
    resp.body_mut().read_to_string().map_err(|e| e.to_string())
}

/// Download a release asset; `progress(done, total)` is called while reading.
pub(crate) fn download(url: &str, progress: &dyn Fn(u64, u64)) -> Result<Vec<u8>, String> {
    let mut resp = agent().get(url).header("User-Agent", "nEdit-updater").call().map_err(|e| trf!("Download fehlgeschlagen: {e}" | "Download failed: {e}"))?;
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
        return Err(tr!("Download unvollständig" | "Download incomplete").into());
    }
    Ok(bytes)
}

/// `nedit --update`: check and install from the command line.
pub fn cli_update() -> Result<String, String> {
    if let Some(src) = source_checkout() {
        return Err(trf!("nEdit läuft aus dem Quellcode – aktualisieren mit: cd {} && git pull && ./install.sh" | "nEdit runs from source – update with: cd {} && git pull && ./install.sh", src.display()));
    }
    let rel = fetch_latest()?;
    if !is_newer(&rel.tag) {
        return Ok(trf!("nEdit {VERSION} ist aktuell (neuestes Release: {})" | "nEdit {VERSION} is up to date (latest release: {})", rel.tag));
    }
    let url = rel.asset_url.ok_or(tr!("Für diese Plattform gibt es kein fertiges Programm" | "There is no prebuilt program for this platform"))?;
    eprintln!("{}", trf!("Lade {} …" | "Downloading {} …", rel.tag));
    let bytes = download(&url, &|d, t| {
        if t > 0 {
            eprint!("\r  {:>5.1} / {:.1} MB", d as f64 / 1e6, t as f64 / 1e6);
        }
    })?;
    eprintln!();
    install_update(&url, &bytes)?;
    Ok(trf!("nEdit wurde von {VERSION} auf {} aktualisiert" | "nEdit was updated from {VERSION} to {}", rel.tag))
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
            self.status = tr!("Suche nach Updates …" | "Checking for updates …").into();
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
            self.status = tr!("Für diese Plattform gibt es kein fertiges Programm – bitte aus dem Quellcode bauen." | "There is no prebuilt program for this platform – please build from source.").into();
            return;
        };
        self.installing = true;
        self.progress = Some((0, 0));
        self.status = trf!("Lade {} …" | "Downloading {} …", rel.tag);
        let tx = self.channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = download(&url, &|d, t| {
                let _ = tx.send(UpdMsg::Progress(d, t));
                ctx.request_repaint();
            })
            .and_then(|bytes| install_update(&url, &bytes));
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
                            self.status = trf!("Version {} ist verfügbar" | "Version {} is available", r.tag);
                            announce = Some(r.tag.clone());
                            self.available = Some(r);
                            self.last_check_ok = true;
                        }
                        Ok(None) => {
                            self.last_check_ok = true;
                            if manual {
                                self.status = trf!("nEdit {VERSION} ist aktuell ✓" | "nEdit {VERSION} is up to date ✓");
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
                            self.status = tr!("Update installiert – nEdit neu starten, um es zu verwenden." | "Update installed – restart nEdit to use it.").into();
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

    #[test]
    fn asset_choice() {
        let names = asset_names();
        if cfg!(target_os = "linux") {
            assert_eq!(names, vec!["nedit-linux-x86_64"]);
        }
        if cfg!(target_os = "macos") {
            assert!(names.contains(&"nedit-macos-universal"));
        }
    }

    /// The bundle swap keeps the executable bit and drops __MACOSX junk.
    #[test]
    #[cfg(unix)]
    fn bundle_zip_unpacks_with_modes() {
        use std::os::unix::fs::PermissionsExt;
        let mut buf = std::io::Cursor::new(vec![]);
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let exe = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            let plain = zip::write::SimpleFileOptions::default().unix_permissions(0o644);
            w.start_file("nEdit.app/Contents/MacOS/nedit", exe).unwrap();
            std::io::Write::write_all(&mut w, b"bin").unwrap();
            w.start_file("nEdit.app/Contents/Info.plist", plain).unwrap();
            std::io::Write::write_all(&mut w, b"<plist/>").unwrap();
            w.start_file("__MACOSX/nEdit.app/._Info.plist", plain).unwrap();
            w.finish().unwrap();
        }
        let dir = std::env::temp_dir().join(format!("nedit-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unzip_with_modes(buf.get_ref(), &dir).unwrap();
        let mode = std::fs::metadata(dir.join("nEdit.app/Contents/MacOS/nedit")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(!dir.join("__MACOSX").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
