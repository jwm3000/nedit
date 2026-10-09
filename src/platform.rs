//! Small helpers that hide differences between Linux, macOS and Windows.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

/// Separator for search-path variables like TEXINPUTS / BIBINPUTS.
pub const PATH_LIST_SEP: &str = if cfg!(windows) { ";" } else { ":" };

/// `Command::new` that does not flash a console window on Windows.
pub fn cmd(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
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

/// Is `name` an executable somewhere on PATH?
pub fn has_tool(name: &str) -> bool {
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or(".EXE;.BAT;.CMD".into()).split(';').map(|s| s.to_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|dir| exts.iter().any(|e| dir.join(format!("{name}{e}")).is_file()))
    })
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
