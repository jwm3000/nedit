//! Suggestions for compile errors: many errors usually share one cause (a missing package,
//! the wrong font encoding, a typo in a package name). Each suggestion explains the cause
//! and carries an action nEdit can carry out with one click.

use crate::compile::{Issue, Level};
use crate::deps::{Key, Pm};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum FixAction {
    /// Insert lines right after `\documentclass` in `file`.
    Preamble { file: String, lines: Vec<String> },
    /// Replace `from` by `to` in `file` around `line` (1-based).
    Replace { file: String, line: usize, from: String, to: String },
    /// Run an install command in a terminal (password prompt, progress).
    Install { command: String },
    /// No TeX at all: start nEdit's own TeX setup (Windows, macOS).
    SetupTex,
    /// Only an explanation.
    None,
}

#[derive(Clone, Debug)]
pub struct Fix {
    pub title: String,
    pub detail: String,
    /// number of errors this explains
    pub covers: usize,
    pub action: FixAction,
}

/// Suggestions for the issues of one compile run. Looks things up in the TeX package
/// database – run it off the UI thread.
pub fn suggest(issues: &[Issue], root: &Path, main: &str, engine: &str, latexmk_out: &str) -> Vec<Fix> {
    let mut fixes = vec![];
    let errors: Vec<&Issue> = issues.iter().filter(|i| i.level == Level::Error).collect();

    // ── font encoding: \textquotedbl & co. exist only in T1 ──
    let re_enc = regex::Regex::new(r"Command (\\[A-Za-z@]+) unavailable in encoding OT1").unwrap();
    let enc: Vec<&&Issue> = errors.iter().filter(|i| i.message.contains("unavailable in encoding OT1")).collect();
    let docs = if enc.is_empty() { vec![] } else { document(root, main) };
    let class = if !enc.is_empty() && engine == "pdflatex" && !project_loads(&docs, "fontenc") { class_file(&docs) } else { None };
    if let Some(class) = class {
        let mut cmds: Vec<String> = enc.iter().filter_map(|i| re_enc.captures(&i.message).map(|c| c[1].to_string())).collect();
        cmds.sort();
        cmds.dedup();
        let mut lines = vec![r"\usepackage[T1]{fontenc}".to_string()];
        let with_lm = !project_loads(&docs, "lmodern") && crate::platform::kpsewhich("lmodern.sty").is_some();
        if with_lm {
            lines.push(r"\usepackage{lmodern}".to_string());
        }
        let listings = project_mentions(&docs, "upquote=true");
        fixes.push(Fix {
            title: tr!("Schriftkodierung T1 aktivieren" | "Switch to the T1 font encoding").into(),
            detail: trf!(
                "{} Fehler haben dieselbe Ursache: {} gibt es nur in der Schriftkodierung T1, das Dokument nutzt aber noch die alte Standardkodierung OT1.{}\n\nnEdit fügt in {class} direkt nach \\documentclass ein:\n{}{}"
                    | "{} errors share one cause: {} only exists in the T1 font encoding, but the document still uses the old default OT1.{}\n\nnEdit adds to {class} right after \\documentclass:\n{}{}",
                enc.len(),
                cmds.join(", "),
                if listings { tr!(" Auslöser ist listings mit upquote=true (gerade Anführungszeichen im Code)." | " It is triggered by listings with upquote=true (straight quotes in code).") } else { "" },
                lines.iter().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n"),
                if with_lm { tr!("\n\nlmodern sorgt dafür, dass die Schrift in T1 scharf bleibt (Vektor statt Bitmap)." | "\n\nlmodern keeps the font sharp in T1 (vector instead of bitmap).") } else { "" }
            ),
            covers: enc.len(),
            action: FixAction::Preamble { file: class, lines },
        });
    }

    // ── missing files: packages, classes, fonts, languages, bibliography styles ──
    let mut missing: BTreeMap<String, (usize, Option<(String, usize)>)> = BTreeMap::new();
    for i in &errors {
        for f in missing_files(&format!("{}\n{}", i.message, i.context)) {
            let e = missing.entry(f).or_insert((0, None));
            e.0 += 1;
            if e.1.is_none() {
                e.1 = i.file.clone().zip(i.line);
            }
        }
    }
    if !missing.is_empty() {
        fixes.extend(missing_file_fixes(&missing, root));
    }

    // ── tools ──
    if latexmk_out.contains("Using biber") && !crate::platform::has_tool("biber") {
        fixes.push(tool_fix(Key::Biber, "biber", tr!("Das Dokument nutzt biblatex mit backend=biber – ohne biber fehlt das Literaturverzeichnis." | "The document uses biblatex with backend=biber – without biber the bibliography is missing.")));
    }
    if !crate::platform::has_tool("latexmk") && (latexmk_out.contains("latexmk konnte nicht gestartet werden") || latexmk_out.contains("Could not start latexmk")) {
        if crate::texsetup::SUPPORTED && !crate::platform::has_tool("pdflatex") {
            fixes.push(Fix {
                title: tr!("TeX einrichten" | "Set up TeX").into(),
                detail: tr!(
                    "Auf diesem Rechner ist noch kein TeX installiert. nEdit lädt TinyTeX und die Pakete der Vorlagen direkt herunter – ohne Administratorrechte, etwa 100 MB." | "There is no TeX on this computer yet. nEdit downloads TinyTeX and the packages of the templates right here – no admin rights, about 100 MB."
                )
                .into(),
                covers: 1,
                action: FixAction::SetupTex,
            });
        } else {
            fixes.push(tool_fix(Key::Latexmk, "latexmk", tr!("nEdit kompiliert mit latexmk – es ist nicht installiert." | "nEdit compiles with latexmk – it is not installed.")));
        }
    }
    fixes.sort_by(|a, b| b.covers.cmp(&a.covers));
    fixes
}

fn tool_fix(k: Key, name: &str, why: &str) -> Fix {
    let hint = crate::deps::install_hint(k);
    let is_cmd = !hint.starts_with("http");
    Fix {
        title: trf!("{name} installieren" | "Install {name}"),
        detail: if is_cmd { format!("{why}\n\n  $ {hint}") } else { format!("{why}\n\n{hint}") },
        covers: 1,
        action: if is_cmd { FixAction::Install { command: hint } } else { FixAction::None },
    }
}

/// Files TeX reports missing in an error text.
fn missing_files(text: &str) -> Vec<String> {
    let mut out = vec![];
    let res = [
        // ! LaTeX Error: File `minted.sty' not found.
        r"File `([^'`/\\]+\.[A-Za-z]+)' not found",
        // ! Font \T1/foo/m/n/10=foo10 at 10pt not loadable: Metric (TFM) file not found.
        r"=([A-Za-z0-9_-]+)(?: at [0-9.]+pt)? not loadable: Metric \(TFM\) file",
        // bibtex: I couldn't open style file foo.bst
        r"couldn't open style file ([A-Za-z0-9_-]+\.bst)",
    ];
    for (k, re) in res.iter().enumerate() {
        let re = regex::Regex::new(re).unwrap();
        for c in re.captures_iter(text) {
            out.push(if k == 1 { format!("{}.tfm", &c[1]) } else { c[1].to_string() });
        }
    }
    // babel: Unknown option 'ngerman' → ngerman.ldf
    if text.contains("babel") {
        let re = regex::Regex::new(r"Unknown option '([A-Za-z]+)'").unwrap();
        out.extend(re.captures_iter(text).map(|c| format!("{}.ldf", &c[1])));
    }
    out
}

fn missing_file_fixes(missing: &BTreeMap<String, (usize, Option<(String, usize)>)>, root: &Path) -> Vec<Fix> {
    let pm = Pm::detect();
    let Some(db) = crate::texpkg::db() else {
        // no package database: ask the package manager by file name where possible
        let files: Vec<&String> = missing.keys().collect();
        let covers = missing.values().map(|v| v.0).sum();
        let pkgs: Vec<String> = files.iter().map(|f| format!("tex({f})")).collect();
        return vec![Fix {
            title: tr!("Fehlende TeX-Dateien" | "Missing TeX files").into(),
            detail: trf!("Nicht gefunden: {}" | "Not found: {}", files.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
            covers,
            action: if matches!(pm, Pm::Dnf | Pm::Zypper) { FixAction::Install { command: crate::texpkg::install_command(pm, &pkgs) } } else { FixAction::None },
        }];
    };
    let mut fixes = vec![];
    let mut install: Vec<String> = vec![];
    let mut lines: Vec<String> = vec![];
    let mut covers = 0;
    let mut stale = vec![];
    for (file, (n, at)) in missing {
        match db.provider(file) {
            Some(p) if db.is_installed(p) => stale.push(file.clone()),
            Some(p) => {
                let sys = crate::texpkg::system_packages(pm, p, Some(file));
                let what = if p.shortdesc.is_empty() { String::new() } else { format!(" – {}", p.shortdesc) };
                lines.push(format!("  {file}  →  {}{what}  ({})", p.name, if sys.is_empty() { "?".into() } else { sys.join(", ") }));
                for s in sys {
                    if !install.contains(&s) {
                        install.push(s);
                    }
                }
                covers += n;
            }
            None => {
                let (stem, ext) = file.rsplit_once('.').unwrap_or((file.as_str(), ""));
                match (db.did_you_mean(file), at) {
                    (Some(s), Some((src, line))) => {
                        let to = s.rsplit_once('.').map_or(s.as_str(), |(st, _)| st).to_string();
                        fixes.push(Fix {
                            title: trf!("Tippfehler? „{stem}“ → „{to}“" | "Typo? “{stem}” → “{to}”"),
                            detail: trf!("„{file}“ gibt es in TeX Live nicht, „{s}“ schon. nEdit korrigiert den Namen in {src}:{line}." | "“{file}” does not exist in TeX Live, “{s}” does. nEdit fixes the name in {src}:{line}."),
                            covers: *n,
                            action: FixAction::Replace { file: src.clone(), line: *line, from: stem.to_string(), to },
                        });
                    }
                    _ => fixes.push(Fix {
                        title: trf!("„{file}“ ist unbekannt" | "“{file}” is unknown"),
                        detail: if root.join(file).exists() || ext == "tex" {
                            tr!("Die Datei gehört zum Projekt, wird aber nicht gefunden – Pfad prüfen." | "The file belongs to the project but is not found – check the path.").into()
                        } else {
                            tr!("Weder installiert noch in TeX Live enthalten. Stammt sie aus einer Vorlage, die .sty/.cls-Datei ins Projekt kopieren." | "Neither installed nor part of TeX Live. If it comes with a template, copy the .sty/.cls file into the project.").into()
                        },
                        covers: *n,
                        action: FixAction::None,
                    }),
                }
            }
        }
    }
    if !install.is_empty() {
        let command = crate::texpkg::install_command(pm, &install);
        fixes.push(Fix {
            title: trf!("Fehlende TeX-Pakete installieren: {}" | "Install missing TeX packages: {}", install.join(", ")),
            detail: trf!("Diese Dateien fehlen in der TeX-Installation:\n{}\n\n  $ {command}" | "These files are missing in the TeX installation:\n{}\n\n  $ {command}", lines.join("\n")),
            covers,
            action: FixAction::Install { command },
        });
    }
    if !stale.is_empty() {
        fixes.push(Fix {
            title: tr!("Dateiliste von TeX aktualisieren" | "Refresh TeX's file list").into(),
            detail: trf!("{} ist installiert, wird aber nicht gefunden – die Dateinamen-Datenbank von TeX ist veraltet." | "{} is installed but not found – TeX's file name database is out of date.", stale.join(", ")),
            covers: stale.iter().map(|f| missing[f].0).sum(),
            action: FixAction::Install { command: if cfg!(windows) { "initexmf --update-fndb".into() } else { "sudo mktexlsr".into() } },
        });
    }
    fixes
}

/// Uncommented \usepackage / \RequirePackage of `pkg` in the document.
fn project_loads(docs: &[(String, String)], pkg: &str) -> bool {
    let re = regex::Regex::new(&format!(r"(?m)^[^%\n]*\\(?:usepackage|RequirePackage)\s*(?:\[[^\]]*\])?\s*\{{[^}}]*\b{}\b[^}}]*\}}", regex::escape(pkg))).unwrap();
    docs.iter().any(|(_, t)| re.is_match(t))
}

/// Uncommented occurrence of `needle` in the document.
fn project_mentions(docs: &[(String, String)], needle: &str) -> bool {
    docs.iter().any(|(_, t)| t.lines().any(|l| strip_comment(l).contains(needle)))
}

/// The files that make up the document, starting at `main`: everything it \input s or
/// \include s, and local .sty/.cls files it loads. Other files in the project folder (the
/// slides with their own theme, old drafts) do not count.
fn document(root: &Path, main: &str) -> Vec<(String, String)> {
    let re_input = regex::Regex::new(r"\\(?:input|include|subfile)\s*\{([^}\\]+)\}").unwrap();
    let re_pkg = regex::Regex::new(r"\\(?:usepackage|RequirePackage|documentclass)\s*(?:\[[^\]]*\])?\s*\{([^}]+)\}").unwrap();
    let main_dir = Path::new(main).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    let local = |name: &str| -> Option<String> {
        [name.to_string(), if main_dir.is_empty() { String::new() } else { format!("{main_dir}/{name}") }].into_iter().find(|c| !c.is_empty() && root.join(c).is_file())
    };
    let mut out: Vec<(String, String)> = vec![];
    let mut queue = std::collections::VecDeque::from([main.to_string()]);
    let mut seen = std::collections::HashSet::new();
    while let Some(f) = queue.pop_front() {
        if !seen.insert(f.clone()) || seen.len() > 200 {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(&f)) else { continue };
        for line in text.lines() {
            let code = strip_comment(line);
            for c in re_input.captures_iter(code) {
                let n = c[1].trim();
                let n = if n.ends_with(".tex") { n.to_string() } else { format!("{n}.tex") };
                if let Some(p) = local(&n) {
                    queue.push_back(p);
                }
            }
            for c in re_pkg.captures_iter(code) {
                for n in c[1].split(',') {
                    for ext in ["sty", "cls"] {
                        if let Some(p) = local(&format!("{}.{ext}", n.trim())) {
                            queue.push_back(p);
                        }
                    }
                }
            }
        }
        out.push((f, text));
    }
    out
}

/// Carry out a file-changing fix. Returns the changed project file.
pub fn apply(root: &Path, action: &FixAction) -> Result<String, String> {
    match action {
        FixAction::Preamble { file, lines } => {
            let path = root.join(file);
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let at = after_documentclass(&text).ok_or_else(|| trf!("Kein \\documentclass in {file} gefunden" | "No \\documentclass found in {file}"))?;
            let block = format!("{}\n{}\n", tr!("% von nEdit ergänzt (Vorschlag zu Kompilierfehlern)" | "% added by nEdit (suggestion for compile errors)"), lines.join("\n"));
            let new = format!("{}{block}{}", &text[..at], &text[at..]);
            std::fs::write(&path, new).map_err(|e| e.to_string())?;
            Ok(file.clone())
        }
        FixAction::Replace { file, line, from, to } => {
            let path = root.join(file);
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
            // TeX reports the line where it noticed the problem – look around it
            let idx = (line.saturating_sub(4)..(line + 3).min(lines.len()))
                .find(|&i| lines.get(i).is_some_and(|l| l.contains(from.as_str())))
                .ok_or_else(|| trf!("„{from}“ nicht in {file}:{line} gefunden" | "“{from}” not found at {file}:{line}"))?;
            lines[idx] = lines[idx].replacen(from.as_str(), to, 1);
            std::fs::write(&path, lines.join("\n")).map_err(|e| e.to_string())?;
            Ok(file.clone())
        }
        FixAction::Install { .. } | FixAction::SetupTex | FixAction::None => Err("not a file fix".into()),
    }
}

/// Project file with `\documentclass`: the main file, or a file it `\input`s (templates
/// like the TU Graz KOMA one build the class call in template/preamble.tex).
fn class_file(docs: &[(String, String)]) -> Option<String> {
    docs.iter().find(|(_, t)| after_documentclass(t).is_some()).map(|(f, _)| f.clone())
}

/// Code part of a line (an unescaped % starts a comment).
fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    (0..b.len()).find(|&i| b[i] == b'%' && (i == 0 || b[i - 1] != b'\\')).map_or(line, |i| &line[..i])
}

/// Byte offset right after the line that closes `\documentclass[...]{...}`. Comments
/// inside the option list (common in templates) are skipped.
fn after_documentclass(text: &str) -> Option<usize> {
    let mut offset = 0;
    let start = text.split_inclusive('\n').find_map(|l| {
        let here = offset;
        offset += l.len();
        strip_comment(l).find(r"\documentclass").map(|i| here + i)
    })?;
    let mut depth_sq = 0i32;
    let mut in_arg = false;
    let mut pos = start;
    for line in text[start..].split_inclusive('\n') {
        let code = strip_comment(line);
        for c in code.chars() {
            match c {
                '[' if !in_arg => depth_sq += 1,
                ']' if !in_arg => depth_sq -= 1,
                '{' if depth_sq == 0 => in_arg = true,
                '}' if in_arg => return Some(pos + line.len()),
                _ => {}
            }
        }
        pos += line.len();
    }
    None
}

#[cfg(test)]
impl FixAction {
    fn lines(&self) -> Vec<String> {
        match self {
            FixAction::Preamble { lines, .. } => lines.clone(),
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(msg: &str, file: Option<(&str, usize)>) -> Issue {
        Issue { level: Level::Error, file: file.map(|f| f.0.to_string()), line: file.map(|f| f.1), message: msg.into(), context: String::new() }
    }

    fn project(files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("nedit-fix-{}-{}", std::process::id(), files.len()));
        let _ = std::fs::remove_dir_all(&root);
        for (f, t) in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        }
        root
    }

    #[test]
    fn encoding_fix_is_applied_after_documentclass() {
        let main = "% Vorlage\n\\documentclass[a4paper,\n  12pt]{scrreprt} % Klasse\n\\usepackage{listings}\n\\input{template/listings_swift}\n\\begin{document}\n\\end{document}\n";
        let root = project(&[("main.tex", main), ("template/listings_swift.tex", "\\lstset{upquote=true}\n")]);
        let issues: Vec<Issue> = (0..3).map(|_| err("LaTeX Error: Command \\textquotedbl unavailable in encoding OT1.", Some(("content/x.tex", 5)))).collect();
        let fixes = suggest(&issues, &root, "main.tex", "pdflatex", "");
        assert_eq!(fixes.len(), 1, "{fixes:?}");
        let f = &fixes[0];
        assert_eq!(f.covers, 3);
        assert!(f.detail.contains("\\textquotedbl") && f.detail.contains("upquote"), "{}", f.detail);
        apply(&root, &f.action).unwrap();
        let after = std::fs::read_to_string(root.join("main.tex")).unwrap();
        let docclass_end = after.find("% Klasse\n").unwrap() + "% Klasse\n".len();
        assert!(after[docclass_end..].starts_with("% "), "{after}");
        assert!(after.contains("\\usepackage[T1]{fontenc}\n"));
        assert!(after.find("fontenc").unwrap() < after.find("{listings}").unwrap());
        // once fontenc is there, no more suggestion (even with a commented-out copy elsewhere)
        assert!(suggest(&issues, &root, "main.tex", "pdflatex", "").is_empty());
        // xelatex does not use OT1/T1 like this
        let root2 = project(&[("main.tex", main)]);
        assert!(suggest(&issues, &root2, "main.tex", "xelatex", "").is_empty());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(root2);
    }

    /// TU Graz KOMA template: the class is set in template/preamble.tex, with comments
    /// (and brackets inside them) in the option list.
    #[test]
    fn encoding_fix_follows_input_to_the_class() {
        let main = "\\newcommand{\\myfontsize}{12pt}\n% \\input{old}\n\\input{template/preamble}%% DO NOT REMOVE THIS LINE!\n\\usepackage{listings}\n\\begin{document}\n\\input{kapitel/a}\n\\end{document}\n";
        let pre = "%doc% intro\n\\expandafter\\documentclass\\expandafter[%\nfontsize=\\myfontsize,%% size [pt] of the main text\nBCOR=0mm,  %% binding correction (depends on] binding)\noneside\n]{scrbook}\n\n\\usepackage{etoolbox}\n";
        // the slides' beamer theme loads fontenc – it is not part of the thesis
        let root = project(&[("main.tex", main), ("template/preamble.tex", pre), ("kapitel/a.tex", "x"), ("praesentation/beamerthemetugraz2018.sty", "\\RequirePackage[T1]{fontenc}\n")]);
        let issues = vec![err("LaTeX Error: Command \\textquotedbl unavailable in encoding OT1.", None)];
        let fixes = suggest(&issues, &root, "main.tex", "pdflatex", "");
        assert_eq!(fixes[0].action, FixAction::Preamble { file: "template/preamble.tex".into(), lines: fixes[0].action.lines() });
        apply(&root, &fixes[0].action).unwrap();
        let after = std::fs::read_to_string(root.join("template/preamble.tex")).unwrap();
        assert!(after.contains("]{scrbook}\n% "), "{after}");
        assert!(after.find("fontenc").unwrap() < after.find("etoolbox").unwrap());
        assert_eq!(std::fs::read_to_string(root.join("main.tex")).unwrap(), main);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn finds_missing_files() {
        assert_eq!(missing_files("! LaTeX Error: File `minted.sty' not found."), vec!["minted.sty"]);
        assert_eq!(missing_files("! Font \\T1/qtm/m/n/10=qtmr7t at 10pt not loadable: Metric (TFM) file not found."), vec!["qtmr7t.tfm"]);
        assert_eq!(missing_files("! Package babel Error: Unknown option 'ngerman'."), vec!["ngerman.ldf"]);
        assert!(missing_files("! LaTeX Error: File `kapitel/a.tex' not found.").is_empty());
    }

    #[test]
    fn typo_in_package_name() {
        if crate::texpkg::db().is_none() {
            return;
        }
        let root = project(&[("main.tex", "\\documentclass{article}\n\\usepackage{lisitngs}\n\\begin{document}\n\\end{document}\n")]);
        let issues = vec![err("LaTeX Error: File `lisitngs.sty' not found.", Some(("main.tex", 2)))];
        let fixes = suggest(&issues, &root, "main.tex", "pdflatex", "");
        assert_eq!(fixes.len(), 1, "{fixes:?}");
        apply(&root, &fixes[0].action).unwrap();
        assert!(std::fs::read_to_string(root.join("main.tex")).unwrap().contains("\\usepackage{listings}"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_package_is_installed_via_collection() {
        let Some(db) = crate::texpkg::db() else { return };
        // a package that is surely not installed: pretend with one that is in TeX Live
        let p = db.provider("minted.sty").unwrap();
        let pkgs = crate::texpkg::system_packages(Pm::Pacman, p, Some("minted.sty"));
        assert_eq!(pkgs, vec!["texlive-latexextra"]);
        assert_eq!(crate::texpkg::install_command(Pm::Pacman, &pkgs), "sudo pacman -S --needed texlive-latexextra");
    }
}
