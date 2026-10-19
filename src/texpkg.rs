//! TeX package manager: which TeX Live package provides a file, which package of this
//! system brings it, whether it is installed, and installing it.
//!
//! The source of truth is the TeX Live package database (`texlive.tlpdb`), which ships with
//! TeX Live and with distribution packages (Arch: /usr/share/tlpkg). It lists every package
//! – installed or not – with its files and the collection it belongs to.

use crate::deps::Pm;
use crate::platform::has_tool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Debug, Default)]
pub struct TexPackage {
    pub name: String,
    pub shortdesc: String,
    /// "collection-latexextra" – what distributions package as one unit
    pub collection: Option<String>,
    /// a characteristic runfile (relative to TEXMFROOT) to tell whether it is installed
    pub probe: Option<String>,
}

pub struct Db {
    pub packages: Vec<TexPackage>,
    /// basename ("minted.sty") → package index
    by_file: HashMap<String, usize>,
    root: PathBuf,
}

/// File types that are looked up when TeX reports them missing.
const LOOKUP_EXT: &[&str] = &["sty", "cls", "ldf", "def", "bst", "tfm", "clo", "cfg", "fd", "bbx", "cbx", "lbx", "dbx", "otf", "ttf", "pfb", "map", "enc"];

/// Collections in order of preference when a package is in several of them.
const PREFERRED: &[&str] = &[
    "collection-basic",
    "collection-latex",
    "collection-latexrecommended",
    "collection-fontsrecommended",
    "collection-latexextra",
    "collection-pictures",
    "collection-mathscience",
    "collection-bibtexextra",
    "collection-fontsextra",
    "collection-binextra",
    "collection-plaingeneric",
    "collection-publishers",
    "collection-humanities",
];

fn tlpdb_path() -> Option<PathBuf> {
    let mut cands = vec![];
    if let Ok(o) = crate::platform::cmd("kpsewhich").args(["-var-value", "TEXMFROOT"]).output() {
        let r = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !r.is_empty() {
            cands.push(PathBuf::from(r).join("tlpkg/texlive.tlpdb"));
        }
    }
    cands.extend(["/usr/share/tlpkg/texlive.tlpdb", "/usr/share/texlive/tlpkg/texlive.tlpdb"].map(PathBuf::from));
    if let Ok(rd) = std::fs::read_dir("/usr/local/texlive") {
        let mut years: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("tlpkg/texlive.tlpdb")).collect();
        years.sort();
        cands.extend(years.into_iter().rev());
    }
    cands.into_iter().find(|p| p.is_file())
}

static DB: OnceLock<Option<Db>> = OnceLock::new();

/// The database if it is loaded already (never blocks – for the UI).
pub fn ready() -> Option<Option<&'static Db>> {
    DB.get().map(Option::as_ref)
}

/// Load the database in the background (for the package manager view).
pub fn preload(ctx: &egui::Context) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            db();
            ctx.request_repaint();
        });
    }
}

/// The package database (parsed once; ~0.2 s – call it off the UI thread).
pub fn db() -> Option<&'static Db> {
    DB.get_or_init(|| {
        let path = tlpdb_path()?;
        let text = std::fs::read_to_string(&path).ok()?;
        // tlpdb lives in <TEXMFROOT>/tlpkg/
        let root = path.parent()?.parent()?.to_path_buf();
        Some(parse(&text, root))
    })
    .as_ref()
}

/// Platform binaries ("minted.x86_64-linux") and installer internals are not packages
/// anyone would install by name. Collections carry no runfiles, so they never get files.
fn keep(name: &str) -> bool {
    !name.contains('.') && !name.starts_with("00")
}

fn parse(text: &str, root: PathBuf) -> Db {
    let mut packages: Vec<TexPackage> = vec![];
    let mut by_file = HashMap::new();
    let mut coll_deps: Vec<(String, Vec<String>)> = vec![];
    let mut cur: Option<TexPackage> = None;
    let mut category = String::new();
    let mut deps: Vec<String> = vec![];
    let mut in_runfiles = false;
    let flush = |cur: &mut Option<TexPackage>, category: &str, deps: &mut Vec<String>, packages: &mut Vec<TexPackage>, coll_deps: &mut Vec<(String, Vec<String>)>| {
        if let Some(p) = cur.take() {
            if category == "Collection" {
                coll_deps.push((p.name.clone(), std::mem::take(deps)));
            } else if keep(&p.name) {
                packages.push(p);
            }
        }
        deps.clear();
    };
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("name ") {
            flush(&mut cur, &category, &mut deps, &mut packages, &mut coll_deps);
            cur = Some(TexPackage { name: name.trim().to_string(), ..Default::default() });
            category.clear();
            in_runfiles = false;
            continue;
        }
        let Some(p) = cur.as_mut() else { continue };
        if let Some(f) = line.strip_prefix(' ') {
            if !in_runfiles {
                continue;
            }
            let f = f.split_whitespace().next().unwrap_or("");
            let base = f.rsplit('/').next().unwrap_or(f);
            let ext = base.rsplit_once('.').map_or("", |(_, e)| e);
            if LOOKUP_EXT.contains(&ext) && keep(&p.name) {
                if p.probe.is_none() || (ext == "sty" && !p.probe.as_deref().unwrap_or("").ends_with(".sty")) {
                    p.probe = Some(f.to_string());
                }
                // index the package gets when it is flushed; first owner wins
                by_file.entry(base.to_string()).or_insert(packages.len());
            }
            continue;
        }
        in_runfiles = line.starts_with("runfiles");
        if let Some(c) = line.strip_prefix("category ") {
            category = c.trim().to_string();
        } else if let Some(d) = line.strip_prefix("shortdesc ") {
            p.shortdesc = d.trim().to_string();
        } else if let Some(d) = line.strip_prefix("depend ") {
            deps.push(d.trim().to_string());
        }
    }
    flush(&mut cur, &category, &mut deps, &mut packages, &mut coll_deps);
    let rank = |c: &str| PREFERRED.iter().position(|p| *p == c).unwrap_or(PREFERRED.len());
    let mut coll_of: HashMap<&str, &str> = HashMap::new();
    for (c, ds) in &coll_deps {
        for d in ds {
            let e = coll_of.entry(d.as_str()).or_insert(c.as_str());
            if rank(c) < rank(e) {
                *e = c.as_str();
            }
        }
    }
    for p in packages.iter_mut() {
        p.collection = coll_of.get(p.name.as_str()).map(|s| s.to_string());
    }
    Db { packages, by_file, root }
}

impl Db {
    /// TeX Live package that contains `file` ("minted.sty").
    pub fn provider(&self, file: &str) -> Option<&TexPackage> {
        self.by_file.get(file).map(|i| &self.packages[*i])
    }

    /// Cheap file-system check (used per visible row in the package view).
    pub fn is_installed(&self, p: &TexPackage) -> bool {
        p.probe.as_ref().is_some_and(|f| self.root.join(f).exists())
    }

    /// Closest known file name for a misspelled one ("lisitngs.sty" → "listings.sty").
    pub fn did_you_mean(&self, file: &str) -> Option<String> {
        let ext = file.rsplit_once('.').map_or("", |(_, e)| e);
        let max = (file.len() / 4).clamp(1, 3);
        self.by_file
            .keys()
            .filter(|k| k.ends_with(&format!(".{ext}")) && k.len().abs_diff(file.len()) <= max)
            .map(|k| (levenshtein(k, file), k))
            .filter(|(d, _)| *d <= max)
            .min()
            .map(|(_, k)| k.clone())
    }

    /// Simple search for the package manager view: name first, then description.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&TexPackage> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return vec![];
        }
        let mut hits: Vec<(u8, &TexPackage)> = self
            .packages
            .iter()
            .filter_map(|p| {
                let n = p.name.to_lowercase();
                let score = if n == q {
                    0
                } else if n.starts_with(&q) {
                    1
                } else if n.contains(&q) {
                    2
                } else if p.shortdesc.to_lowercase().contains(&q) {
                    3
                } else {
                    return None;
                };
                Some((score, p))
            })
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.len().cmp(&b.1.name.len())));
        hits.into_iter().take(limit).map(|(_, p)| p).collect()
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// What has to be installed on this system to get a TeX Live package (and `file` from it).
pub fn system_packages(pm: Pm, p: &TexPackage, file: Option<&str>) -> Vec<String> {
    let coll = p.collection.as_deref().and_then(|c| c.strip_prefix("collection-"));
    match pm {
        // Arch splits TeX Live exactly along the collections
        Pm::Pacman => coll.map(|c| vec![format!("texlive-{c}")]).unwrap_or_default(),
        Pm::Apt => coll.map(|c| vec![debian_name(c)]).unwrap_or_else(|| vec!["texlive-full".into()]),
        // Fedora / openSUSE provide tex(<file>)
        Pm::Dnf | Pm::Zypper => match file.or(p.probe.as_deref().map(|f| f.rsplit('/').next().unwrap_or(f))) {
            Some(f) => vec![format!("tex({f})")],
            None => vec![format!("texlive-{}", p.name)],
        },
        Pm::Xbps | Pm::Apk => vec!["texlive-full".into()],
        // tlmgr / MiKTeX know the TeX Live names
        Pm::Brew | Pm::MacNoBrew | Pm::WinTexlive | Pm::WinMiktex | Pm::Winget | Pm::Unknown => vec![p.name.clone()],
    }
}

fn debian_name(coll: &str) -> String {
    let fixed = match coll {
        "basic" => "texlive-base",
        "latex" => "texlive-latex-base",
        "latexrecommended" => "texlive-latex-recommended",
        "latexextra" => "texlive-latex-extra",
        "fontsrecommended" => "texlive-fonts-recommended",
        "fontsextra" => "texlive-fonts-extra",
        "mathscience" => "texlive-science",
        "bibtexextra" => "texlive-bibtex-extra",
        "binextra" => "texlive-extra-utils",
        "plaingeneric" => "texlive-plain-generic",
        "fontutils" => "texlive-font-utils",
        "formatsextra" => "texlive-formats-extra",
        "context" => "context",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_string();
    }
    match coll.strip_prefix("lang") {
        Some(l) => format!("texlive-lang-{l}"),
        None => format!("texlive-{coll}"),
    }
}

/// Install command as the user would type it.
pub fn install_command(pm: Pm, pkgs: &[String]) -> String {
    let quoted: Vec<String> = pkgs.iter().map(|p| if p.contains('(') { format!("'{p}'") } else { p.clone() }).collect();
    match pm {
        Pm::Brew | Pm::MacNoBrew => format!("sudo tlmgr install {}", quoted.join(" ")),
        Pm::Unknown if has_tool("tlmgr") => format!("tlmgr install {}", quoted.join(" ")),
        Pm::WinTexlive => format!("tlmgr install {}", quoted.join(" ")),
        Pm::Winget => format!("miktex packages install {}", quoted.join(" ")),
        _ => {
            let refs: Vec<&str> = quoted.iter().map(String::as_str).collect();
            pm.install_cmd(&refs)
        }
    }
}

/// Run the install in a terminal window, where the password prompt and progress are
/// visible. `done` is created (with the exit code) when the command has finished.
pub fn install_in_terminal(command: &str, done: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(done);
    #[cfg(windows)]
    {
        let script = format!("{command} & echo %ERRORLEVEL% > \"{}\" & pause", done.display());
        return crate::platform::cmd("cmd").args(["/C", "start", "nEdit", "cmd", "/C", &script]).spawn().map(|_| ()).map_err(|e| e.to_string());
    }
    #[cfg(not(windows))]
    {
        let banner = tr!("nEdit installiert TeX-Pakete" | "nEdit is installing TeX packages");
        let close = tr!("Fertig – Enter schließt das Fenster." | "Done – press Enter to close this window.");
        let script = format!("printf '\\n  %s\\n\\n  $ %s\\n\\n' '{banner}' \"$1\"; sh -c \"$1\"; rc=$?; echo $rc > \"$2\"; printf '\\n  {close} '; read _", );
        #[cfg(target_os = "macos")]
        {
            let line = format!("sh -c {} nedit {} {}", shell_quote(&script), shell_quote(command), shell_quote(&done.display().to_string()));
            let osa = format!("tell application \"Terminal\" to do script {}", applescript_quote(&line));
            return crate::platform::cmd("osascript").args(["-e", &osa, "-e", "tell application \"Terminal\" to activate"]).spawn().map(|_| ()).map_err(|e| e.to_string());
        }
        #[cfg(not(target_os = "macos"))]
        {
            let done = done.display().to_string();
            let args = ["sh", "-c", script.as_str(), "nedit", command, done.as_str()];
            let mut terms: Vec<(String, Vec<&str>)> = vec![];
            if let Ok(t) = std::env::var("TERMINAL") {
                if !t.is_empty() && has_tool(&t) {
                    let sep: Vec<&str> = if t.contains("xdg-terminal-exec") { vec![] } else { vec!["-e"] };
                    terms.push((t, sep));
                }
            }
            for (t, sep) in [("xdg-terminal-exec", vec![]), ("x-terminal-emulator", vec!["-e"]), ("gnome-terminal", vec!["--"]), ("konsole", vec!["-e"]), ("alacritty", vec!["-e"]), ("kitty", vec![]), ("foot", vec![]), ("ghostty", vec!["-e"]), ("xterm", vec!["-e"])] {
                if has_tool(t) {
                    terms.push((t.to_string(), sep));
                }
            }
            let (t, sep) = terms.into_iter().next().ok_or_else(|| tr!("Kein Terminal gefunden" | "No terminal found").to_string())?;
            crate::platform::cmd(&t).args(sep).args(args).spawn().map(|_| ()).map_err(|e| e.to_string())
        }
    }
}

#[cfg(target_os = "macos")]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Exit code written by a finished terminal install, if it is done.
pub fn install_finished(done: &Path) -> Option<i32> {
    std::fs::read_to_string(done).ok().map(|s| s.trim().parse().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "name collection-latexextra
category Collection
shortdesc LaTeX additional packages
depend minted
depend lipsum

name collection-xetex
category Collection
depend minted

name minted
category Package
shortdesc Highlighted source code for LaTeX
runfiles size=20
 texmf-dist/tex/latex/minted/minted.sty
 texmf-dist/tex/latex/minted/minted2.sty

name minted.x86_64-linux
category Package
runfiles size=1
 bin/x86_64-linux/latexminted

name babel-german
category Package
shortdesc Babel support for German
runfiles size=5
 texmf-dist/tex/generic/babel-german/ngerman.ldf

name collection-langgerman
category Collection
depend babel-german
";

    #[test]
    fn lookup() {
        let db = parse(SAMPLE, PathBuf::from("/nonexistent"));
        let p = db.provider("minted.sty").unwrap();
        assert_eq!(p.name, "minted");
        // in two collections: the preferred one wins
        assert_eq!(p.collection.as_deref(), Some("collection-latexextra"));
        assert_eq!(p.shortdesc, "Highlighted source code for LaTeX");
        assert!(!db.packages.iter().any(|p| p.name.contains('.')));
        let g = db.provider("ngerman.ldf").unwrap();
        assert_eq!(g.collection.as_deref(), Some("collection-langgerman"));
        assert_eq!(system_packages(Pm::Pacman, p, Some("minted.sty")), vec!["texlive-latexextra"]);
        assert_eq!(system_packages(Pm::Apt, p, None), vec!["texlive-latex-extra"]);
        assert_eq!(system_packages(Pm::Apt, g, None), vec!["texlive-lang-german"]);
        assert_eq!(system_packages(Pm::Dnf, p, Some("minted.sty")), vec!["tex(minted.sty)"]);
        assert_eq!(install_command(Pm::Dnf, &system_packages(Pm::Dnf, p, Some("minted.sty"))), "sudo dnf install 'tex(minted.sty)'");
        assert_eq!(db.did_you_mean("mintd.sty").as_deref(), Some("minted.sty"));
        assert_eq!(db.did_you_mean("completelydifferent.sty"), None);
        assert_eq!(db.search("mint", 5)[0].name, "minted");
    }

    /// The real database of this machine, if there is one.
    #[test]
    fn real_tlpdb() {
        let Some(db) = db() else { return };
        assert!(db.packages.len() > 1000);
        let l = db.provider("listings.sty").unwrap();
        assert_eq!(l.name, "listings");
        assert_eq!(l.collection.as_deref(), Some("collection-latexrecommended"));
        assert_eq!(db.provider("scrreprt.cls").unwrap().name, "koma-script");
        assert_eq!(db.did_you_mean("lisitngs.sty").as_deref(), Some("listings.sty"));
    }
}
