//! Small helpers that hide differences between Linux, macOS and Windows.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

/// Separator for search-path variables like TEXINPUTS / BIBINPUTS.
pub const PATH_LIST_SEP: &str = if cfg!(windows) { ";" } else { ":" };

/// PATH with the tool folders nEdit added while running (after setting up TeX). Kept here
/// instead of changing the process environment, which is not thread-safe on Unix; every
/// command nEdit starts gets it (and std looks the program up in it).
static TOOL_PATH: std::sync::RwLock<Option<std::ffi::OsString>> = std::sync::RwLock::new(None);

fn tool_path() -> Option<std::ffi::OsString> {
    TOOL_PATH.read().ok().and_then(|p| p.clone()).or_else(|| std::env::var_os("PATH"))
}

/// Put `dirs` in front of the PATH nEdit uses for its tools.
pub fn add_tool_dirs(dirs: &[PathBuf]) {
    let p = crate::texsetup::path_with(dirs, tool_path());
    if let Ok(mut w) = TOOL_PATH.write() {
        *w = Some(p);
    }
}

/// `Command::new` that does not flash a console window on Windows and sees the tools nEdit
/// set up.
pub fn cmd(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    if let Some(p) = TOOL_PATH.read().ok().and_then(|p| p.clone()) {
        c.env("PATH", p);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Open a file or folder with the default application of the OS.
pub fn open_external(path: impl AsRef<OsStr>) {
    #[cfg(target_os = "macos")]
    let r = cmd("open").arg(path).spawn();
    #[cfg(windows)]
    let r = cmd("cmd").arg("/C").arg("start").arg("").arg(path).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let r = cmd("xdg-open").arg(path).spawn();
    let _ = r;
}

/// Full path of `name` on PATH, if it is there.
pub fn which(name: &str) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or(".EXE;.BAT;.CMD".into()).split(';').map(|s| s.to_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    let path = tool_path()?;
    std::env::split_paths(&path).find_map(|dir| exts.iter().map(|e| dir.join(format!("{name}{e}"))).find(|p| p.is_file()))
}

/// Folders where TeX, Homebrew & Co. live on macOS. Apps started from the Finder or the Dock
/// only get `/usr/bin:/bin:/usr/sbin:/sbin` – without these an installed MacTeX is not found.
#[cfg(target_os = "macos")]
fn mac_tool_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["/Library/TeX/texbin", "/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].iter().map(PathBuf::from).collect();
    // what the shell's path_helper would add (MacTeX writes /etc/paths.d/TeX)
    let mut files = vec![PathBuf::from("/etc/paths")];
    if let Ok(rd) = std::fs::read_dir("/etc/paths.d") {
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        v.sort();
        files.extend(v);
    }
    for f in files {
        for l in std::fs::read_to_string(f).unwrap_or_default().lines() {
            let l = l.trim();
            if l.starts_with('/') {
                dirs.push(PathBuf::from(l));
            }
        }
    }
    // a TeX Live installed with the official installer
    if let Ok(rd) = std::fs::read_dir("/usr/local/texlive") {
        let mut years: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("bin/universal-darwin")).filter(|p| p.is_dir()).collect();
        years.sort();
        dirs.extend(years.into_iter().rev());
    }
    dirs
}

/// Make the tools visible to nEdit and everything it starts: TeX that nEdit set up itself
/// (Windows, macOS) and, on macOS, the usual install folders. Call at start, before any
/// thread exists.
pub fn setup_path() {
    #[allow(unused_mut)]
    let mut dirs = crate::texsetup::bundled_paths();
    #[cfg(target_os = "macos")]
    dirs.extend(mac_tool_dirs());
    let dirs: Vec<PathBuf> = dirs.into_iter().filter(|d| d.is_dir()).collect();
    if dirs.is_empty() {
        return;
    }
    let p = crate::texsetup::path_with(&dirs, std::env::var_os("PATH"));
    // SAFETY: called first thing in main – no other thread can read the environment yet
    unsafe { std::env::set_var("PATH", p) };
}

/// Is a working `git` there? On macOS `/usr/bin/git` is only a stub without Apple's command
/// line tools: calling it opens an "install the developer tools" dialog every time, so it
/// must not be run then (`xcode-select -p` tells without a dialog).
pub fn has_git() -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::sync::Mutex;
        static CACHE: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);
        let mut c = CACHE.lock().unwrap();
        if let Some((t, ok)) = *c {
            if t.elapsed().as_secs() < 30 {
                return ok;
            }
        }
        let ok = match which("git") {
            Some(p) if p == std::path::Path::new("/usr/bin/git") => cmd("xcode-select").arg("-p").output().is_ok_and(|o| o.status.success()),
            Some(_) => true,
            None => false,
        };
        *c = Some((std::time::Instant::now(), ok));
        ok
    }
    #[cfg(not(target_os = "macos"))]
    has_tool("git")
}

/// Is `name` an executable somewhere on PATH?
pub fn has_tool(name: &str) -> bool {
    which(name).is_some()
}

/// Locate a file inside the TeX distribution (fonts etc.).
pub fn kpsewhich(file: &str) -> Option<PathBuf> {
    let out = cmd("kpsewhich").arg(file).output().ok()?;
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!p.is_empty()).then(|| PathBuf::from(p)).filter(|p| p.exists())
}

/// Project-relative paths always use `/`.
pub fn slash(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}
