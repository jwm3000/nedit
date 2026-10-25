//! Set up a minimal TeX system from inside nEdit – no admin rights, no installer
//! (Windows and macOS; on Linux the package manager does it).
//!
//! - TinyTeX-1: a small TeX Live with pdflatex, latexmk and tlmgr (on Windows with its own
//!   Perl, which latexmk needs; macOS ships Perl)
//! - the packages the built-in TU Graz templates need, plus biber and synctex, via tlmgr
//! - Windows only: Poppler (pdfinfo, pdftoppm, pdftotext) for the PDF preview – macOS
//!   renders PDFs itself
//!
//! Windows: everything in one folder (`%APPDATA%\nEdit`). macOS: `~/Library/TinyTeX`, where
//! TinyTeX's own installer puts it (an existing TinyTeX, e.g. from R, is reused). nEdit puts
//! the bin folder on PATH for its own processes at start; the system PATH is not touched.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

/// Always the current TinyTeX build (fixed names of the "daily" release).
const TINYTEX_BASE: &str = "https://github.com/rstudio/tinytex-releases/releases/download/daily/";
const POPPLER_API: &str = "https://api.github.com/repos/oschwartz10612/poppler-windows/releases/latest";

/// What the built-in thesis and slide templates load beyond TinyTeX-1 (taken from the files
/// a compiled thesis + presentation read), plus German hyphenation, biber and synctex.
pub const TEMPLATE_PACKAGES: &[&str] = &[
    "babel-english", "babel-german", "hyphen-german", "beamer", "biblatex", "biblatex-apa", "caption", "csquotes", "enumitem", "environ", "eso-pic",
    "fontawesome5", "fontname", "footmisc", "fpl", "koma-script", "lipsum", "listings", "listingsutf8", "logreq", "mathpazo", "microtype", "mptopdf",
    "oberdiek", "ocg-p", "palatino", "pdfcol", "pgf", "placeins", "sansmathaccent", "soul", "tcolorbox", "tikzfill", "translator", "trimspaces", "ulem",
    "units", "xpatch", "xurl", "biber", "synctex",
];

/// Is there a setup assistant for this system?
pub const SUPPORTED: bool = cfg!(any(windows, target_os = "macos"));

/// Folder for the tools (Windows). TeX cannot cope with spaces or non-ASCII characters in
/// paths, so – like TinyTeX's own installer – fall back to ProgramData if APPDATA has them.
pub fn base_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        return dirs::home_dir().unwrap_or_default().join("Library");
    }
    let ok = |p: &Path| p.to_str().is_some_and(|s| s.chars().all(|c| c.is_ascii_graphic()));
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).or_else(dirs::config_dir).unwrap_or_default();
    let base = if ok(&appdata) { appdata } else { std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or(appdata) };
    base.join("nEdit")
}

/// Download of the TinyTeX bundle for this system and its file name.
fn tinytex_file() -> &'static str {
    if cfg!(target_os = "macos") { "TinyTeX-1-darwin.tar.xz" } else { "TinyTeX-1-windows.exe" }
}

pub fn tinytex_dir(base: &Path) -> PathBuf {
    base.join("TinyTeX")
}

pub fn tinytex_bin(base: &Path) -> PathBuf {
    tinytex_dir(base).join("bin").join(if cfg!(target_os = "macos") { "universal-darwin" } else { "windows" })
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// The Poppler zip has a versioned top folder; find the folder with pdftoppm.exe.
pub fn poppler_bin(base: &Path) -> Option<PathBuf> {
    fn find(dir: &Path, depth: usize) -> Option<PathBuf> {
        if dir.join("pdftoppm.exe").is_file() {
            return Some(dir.to_path_buf());
        }
        if depth == 0 {
            return None;
        }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        subs.sort();
        subs.iter().find_map(|d| find(d, depth - 1))
    }
    find(&base.join("poppler"), 4)
}

pub fn tinytex_installed(base: &Path) -> bool {
    tinytex_bin(base).join(exe("pdflatex")).is_file()
}

/// Everything the assistant sets up is there.
pub fn ready(base: &Path) -> bool {
    tinytex_installed(base) && (!cfg!(windows) || poppler_bin(base).is_some())
}

/// tlmgr of the TinyTeX nEdit manages, if it is set up.
pub fn own_tlmgr() -> Option<PathBuf> {
    let base = base_dir();
    SUPPORTED.then(|| tinytex_bin(&base).join(if cfg!(windows) { "tlmgr.bat" } else { "tlmgr" })).filter(|p| p.is_file())
}

/// Bin folders set up by nEdit, for PATH.
pub fn bundled_paths() -> Vec<PathBuf> {
    if !SUPPORTED {
        return vec![];
    }
    let base = base_dir();
    let mut v = vec![];
    if tinytex_installed(&base) {
        v.push(tinytex_bin(&base));
    }
    if cfg!(windows) {
        v.extend(poppler_bin(&base));
    }
    v
}

/// `PATH` with `dirs` in front (each only once).
pub fn path_with(dirs: &[PathBuf], path: Option<std::ffi::OsString>) -> std::ffi::OsString {
    let old: Vec<PathBuf> = path.as_ref().map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    let mut all: Vec<PathBuf> = vec![];
    for d in dirs.iter().cloned().chain(old) {
        if !all.contains(&d) {
            all.push(d);
        }
    }
    std::env::join_paths(all).unwrap_or_default()
}

/// Make freshly set up tools visible to the processes nEdit starts (after the setup).
fn add_to_path() {
    crate::platform::add_tool_dirs(&bundled_paths());
}

#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// step title, bytes done, bytes total (0 = unknown)
    Download(String, u64, u64),
    Step(String),
    /// a line of tlmgr's output
    Line(String),
    Done,
    Failed(String),
}

pub struct Setup {
    rx: Receiver<Progress>,
    pub current: Progress,
    pub lines: Vec<String>,
    pub finished: Option<Result<(), String>>,
}

impl Setup {
    /// Take new progress messages; returns true once when the job has finished.
    pub fn poll(&mut self) -> bool {
        let mut ended = false;
        while let Ok(m) = self.rx.try_recv() {
            match &m {
                Progress::Line(l) => {
                    self.lines.push(l.clone());
                    if self.lines.len() > 200 {
                        self.lines.remove(0);
                    }
                }
                Progress::Done => {
                    self.finished = Some(Ok(()));
                    ended = true;
                }
                Progress::Failed(e) => {
                    self.finished = Some(Err(e.clone()));
                    ended = true;
                }
                _ => {}
            }
            if !matches!(m, Progress::Line(_)) {
                self.current = m;
            }
        }
        ended
    }

    pub fn running(&self) -> bool {
        self.finished.is_none()
    }
}

/// `nedit --setup-tex`: the same setup from a terminal, with its progress printed.
pub fn cli() -> Result<(), String> {
    if !SUPPORTED {
        return Err(tr!("Auf Linux installiert der Paketmanager TeX – siehe Einstellungen → TeX & Werkzeuge." | "On Linux the package manager installs TeX – see Settings → TeX & tools.").into());
    }
    let last = std::sync::Mutex::new(String::new());
    run(&base_dir(), &|m| match m {
        Progress::Download(t, d, total) => {
            let line = format!("{t}: {:.0} / {:.0} MB", d as f64 / 1e6, total as f64 / 1e6);
            let mut l = last.lock().unwrap();
            if *l != line {
                eprint!("\r{line}   ");
                *l = line;
            }
        }
        Progress::Step(t) => eprintln!("\n{t}"),
        Progress::Line(l) => eprintln!("  {l}"),
        Progress::Done | Progress::Failed(_) => {}
    })?;
    eprintln!("\n{}", trf!("Fertig: {}" | "Done: {}", tinytex_bin(&base_dir()).display()));
    Ok(())
}

/// Start the setup in the background.
pub fn start(ctx: &egui::Context) -> Setup {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let send = |m: Progress| {
            let _ = tx.send(m);
            ctx.request_repaint();
        };
        match run(&base_dir(), &send) {
            Ok(()) => send(Progress::Done),
            Err(e) => send(Progress::Failed(e)),
        }
    });
    Setup { rx, current: Progress::Step(tr!("Starte …" | "Starting …").into()), lines: vec![], finished: None }
}

fn run(base: &Path, send: &dyn Fn(Progress)) -> Result<(), String> {
    std::fs::create_dir_all(base).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        // latexmk and tlmgr are Perl scripts; macOS ships Perl (TinyTeX checks the same)
        let perl = crate::platform::cmd("perl").args(["-mFile::Find", "/dev/null"]).status().is_ok_and(|s| s.success());
        if !perl {
            return Err(tr!("Perl fehlt (latexmk und tlmgr brauchen es) – mit „xcode-select --install“ nachinstallieren." | "Perl is missing (latexmk and tlmgr need it) – install it with “xcode-select --install”.").into());
        }
    }
    // 1. TinyTeX
    if !tinytex_installed(base) {
        let title = tr!("TinyTeX herunterladen" | "Downloading TinyTeX").to_string();
        let url = format!("{TINYTEX_BASE}{}", tinytex_file());
        let bytes = crate::updater::download(&url, &|d, t| send(Progress::Download(title.clone(), d, t)))?;
        send(Progress::Step(tr!("TinyTeX entpacken" | "Unpacking TinyTeX").into()));
        let _ = std::fs::remove_dir_all(tinytex_dir(base));
        unpack_tinytex(base, &bytes)?;
        if !tinytex_installed(base) {
            return Err(tr!("TinyTeX konnte nicht entpackt werden" | "TinyTeX could not be unpacked").into());
        }
    }
    // 2. packages for the templates (+ biber, synctex)
    send(Progress::Step(trf!("{} TeX-Pakete installieren" | "Installing {} TeX packages", TEMPLATE_PACKAGES.len())));
    let tlmgr = own_tlmgr().ok_or(tr!("tlmgr fehlt in TinyTeX" | "tlmgr is missing in TinyTeX"))?;
    tlmgr_install(&tlmgr, TEMPLATE_PACKAGES, send)?;
    // 3. Windows: Poppler for the PDF preview (macOS renders PDFs itself)
    if cfg!(windows) && poppler_bin(base).is_none() {
        let url = poppler_url()?;
        let title = tr!("Poppler herunterladen (PDF-Vorschau)" | "Downloading Poppler (PDF preview)").to_string();
        let bytes = crate::updater::download(&url, &|d, t| send(Progress::Download(title.clone(), d, t)))?;
        send(Progress::Step(tr!("Poppler entpacken" | "Unpacking Poppler").into()));
        unzip(&bytes, &base.join("poppler"))?;
        if poppler_bin(base).is_none() {
            return Err(tr!("pdftoppm.exe nicht im Poppler-Archiv gefunden" | "pdftoppm.exe not found in the Poppler archive").into());
        }
    }
    add_to_path();
    Ok(())
}

/// Windows: a 7-Zip self-extractor (`-y` extracts into the working directory without asking).
/// macOS: a .tar.xz with a `TinyTeX/` top folder; the system's tar reads xz.
fn unpack_tinytex(base: &Path, bytes: &[u8]) -> Result<(), String> {
    let file = std::env::temp_dir().join(format!("nedit-{}", tinytex_file()));
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let st = if cfg!(windows) {
        let exe = base.join(tinytex_file());
        let _ = std::fs::rename(&file, &exe).or_else(|_| std::fs::copy(&file, &exe).map(|_| ()));
        let st = crate::platform::cmd(&exe).arg("-y").current_dir(base).status();
        let _ = std::fs::remove_file(&exe);
        st
    } else {
        crate::platform::cmd("tar").arg("-xf").arg(&file).arg("-C").arg(base).status()
    };
    let _ = std::fs::remove_file(&file);
    match st {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(trf!("Entpacken fehlgeschlagen ({s})" | "Unpacking failed ({s})")),
        Err(e) => Err(e.to_string()),
    }
}

/// Install packages with tlmgr; its output is passed on line by line.
pub fn tlmgr_install(tlmgr: &Path, packages: &[&str], send: &dyn Fn(Progress)) -> Result<(), String> {
    use std::io::BufRead;
    let mut cmd = if cfg!(windows) {
        let mut c = crate::platform::cmd("cmd");
        c.arg("/C").arg(tlmgr);
        c
    } else {
        crate::platform::cmd(tlmgr)
    };
    let mut child = cmd
        .arg("install")
        .args(packages)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| trf!("tlmgr nicht startbar: {e}" | "Cannot start tlmgr: {e}"))?;
    let err = child.stderr.take();
    let errs = std::thread::spawn(move || err.map(|e| std::io::BufReader::new(e).lines().map_while(Result::ok).collect::<Vec<_>>()).unwrap_or_default());
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                send(Progress::Line(line));
            }
        }
    }
    let st = child.wait().map_err(|e| e.to_string())?;
    let errs = errs.join().unwrap_or_default();
    for l in &errs {
        send(Progress::Line(l.clone()));
    }
    if st.success() {
        Ok(())
    } else {
        Err(trf!("tlmgr ist fehlgeschlagen: {}" | "tlmgr failed: {}", errs.last().cloned().unwrap_or_default()))
    }
}

/// Download address of the current Poppler build.
fn poppler_url() -> Result<String, String> {
    let json = crate::updater::get_text(POPPLER_API)?;
    poppler_asset(&json).ok_or_else(|| tr!("Kein Poppler-Download gefunden" | "No Poppler download found").into())
}

fn poppler_asset(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v["assets"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(".zip"))).and_then(|a| a["browser_download_url"].as_str()).map(String::from)
}

fn unzip(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name rejects absolute paths and ../ (zip slip)
        let Some(rel) = f.enclosed_name() else { continue };
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nedit-win-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_tools_and_builds_path() {
        let base = tmp("paths");
        assert!(!tinytex_installed(&base) && !ready(&base));
        let bin = tinytex_bin(&base);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(exe("pdflatex")), "").unwrap();
        assert!(tinytex_installed(&base));
        let pop = base.join("poppler/poppler-26.09.0/Library/bin");
        std::fs::create_dir_all(&pop).unwrap();
        std::fs::write(pop.join("pdftoppm.exe"), "").unwrap();
        assert_eq!(poppler_bin(&base), Some(pop.clone()));
        assert!(ready(&base));
        let old = std::env::join_paths([PathBuf::from("/usr/bin"), bin.clone()]).unwrap();
        let new: Vec<PathBuf> = std::env::split_paths(&path_with(&[bin.clone(), pop.clone()], Some(old))).collect();
        assert_eq!(new, vec![bin, pop, PathBuf::from("/usr/bin")], "own folders first, no duplicates");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn poppler_release_and_unzip() {
        let json = r#"{"tag_name":"v26.09.0-0","assets":[{"name":"Release-26.09.0-0.zip","browser_download_url":"https://github.com/oschwartz10612/poppler-windows/releases/download/v26.09.0-0/Release-26.09.0-0.zip"}]}"#;
        assert!(poppler_asset(json).unwrap().ends_with("Release-26.09.0-0.zip"));
        // a small zip with the Poppler layout, plus a zip-slip entry that must be skipped
        let mut buf = std::io::Cursor::new(vec![]);
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("poppler-26.09.0/Library/bin/pdftoppm.exe", o).unwrap();
            std::io::Write::write_all(&mut w, b"x").unwrap();
            w.start_file("../evil.txt", o).unwrap();
            w.finish().unwrap();
        }
        let base = tmp("unzip");
        unzip(buf.get_ref(), &base.join("poppler")).unwrap();
        assert_eq!(poppler_bin(&base), Some(base.join("poppler/poppler-26.09.0/Library/bin")));
        assert!(!base.join("evil.txt").exists());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn template_packages_exist_in_tex_live() {
        let Some(db) = crate::texpkg::db() else { return };
        for p in TEMPLATE_PACKAGES {
            assert!(db.packages.iter().any(|x| x.name == *p), "{p} is not a TeX Live package");
        }
    }

    #[test]
    fn base_dir_per_system() {
        let b = base_dir();
        if cfg!(target_os = "macos") {
            assert!(b.ends_with("Library"));
            assert!(tinytex_bin(&b).ends_with("Library/TinyTeX/bin/universal-darwin"));
        } else {
            assert!(b.ends_with("nEdit"));
        }
    }

    /// macOS: the real TinyTeX archive unpacks to TinyTeX/bin/universal-darwin (needs the
    /// network: NEDIT_NET_TESTS=1).
    #[test]
    #[cfg(target_os = "macos")]
    fn mac_tinytex_layout() {
        if std::env::var_os("NEDIT_NET_TESTS").is_none() {
            return;
        }
        let base = tmp("tinytex");
        let bytes = crate::updater::download(&format!("{TINYTEX_BASE}{}", tinytex_file()), &|_, _| {}).unwrap();
        unpack_tinytex(&base, &bytes).unwrap();
        assert!(tinytex_installed(&base), "pdflatex in {}", tinytex_bin(&base).display());
        assert!(tinytex_bin(&base).join("tlmgr").is_file() && tinytex_bin(&base).join("latexmk").is_file());
        let _ = std::fs::remove_dir_all(base);
    }
}

#[cfg(all(test, unix))]
mod tlmgr_tests {
    use super::*;

    /// tlmgr's output arrives line by line; its exit code decides success.
    #[test]
    fn tlmgr_output_and_exit_code() {
        let dir = std::env::temp_dir().join(format!("nedit-tlmgr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            let mut perm = std::fs::metadata(&p).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
            std::fs::set_permissions(&p, perm).unwrap();
            p
        };
        let ok = script("ok", "echo \"tlmgr: $1 $2 $3\"; echo; echo '[1/2] install: biber'");
        let lines = std::sync::Mutex::new(vec![]);
        tlmgr_install(&ok, &["biber", "synctex"], &|m| {
            if let Progress::Line(l) = m {
                lines.lock().unwrap().push(l)
            }
        })
        .unwrap();
        assert_eq!(*lines.lock().unwrap(), vec!["tlmgr: install biber synctex", "[1/2] install: biber"]);
        let bad = script("bad", "echo 'package foo not present' >&2; exit 1");
        let e = tlmgr_install(&bad, &["foo"], &|_| {}).unwrap_err();
        assert!(e.contains("package foo not present"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Screenshot mode on non-Windows systems: show the assistant with simulated progress.
pub fn demo(ctx: &egui::Context) -> Setup {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let total = 73_667_717u64;
        for i in 0..=40u64 {
            let _ = tx.send(Progress::Download(tr!("TinyTeX herunterladen" | "Downloading TinyTeX").into(), total * i / 100, total));
            ctx.request_repaint();
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        let _ = tx.send(Progress::Line("[12/41, 00:14/01:02] install: koma-script [7016k]".into()));
    });
    Setup { rx, current: Progress::Step(tr!("Starte …" | "Starting …").into()), lines: vec![], finished: None }
}
