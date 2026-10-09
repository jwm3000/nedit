//! Running latexmk in the background, parsing the log, and SyncTeX lookups.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error,
    Warning,
    BadBox,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub level: Level,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub message: String,
    pub context: String,
}

pub struct CompileResult {
    pub pdf: Option<PathBuf>,
    pub issues: Vec<Issue>,
    pub raw_log: String,
    pub duration: Duration,
    pub ok: bool,
}

pub struct CompileJob {
    pub rx: Receiver<CompileResult>,
}

#[derive(Clone)]
pub struct CompileSpec {
    pub root: PathBuf,
    pub main: String,   // relative to root, e.g. "main.tex"
    pub outdir: String, // relative to root
    pub engine: String, // pdflatex | xelatex | lualatex
}

impl CompileSpec {
    pub fn pdf_path(&self) -> PathBuf {
        self.root.join(&self.outdir).join(format!("{}.pdf", self.stem()))
    }
    pub fn stem(&self) -> String {
        Path::new(&self.main).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "main".into())
    }
    fn texinputs(&self) -> String {
        let main_dir = self.root.join(&self.main).parent().map(Path::to_path_buf).unwrap_or(self.root.clone());
        let sep = crate::platform::PATH_LIST_SEP;
        format!("{}{sep}{}{sep}", main_dir.display(), self.root.display())
    }
}

pub fn start(spec: CompileSpec, ctx: egui::Context) -> CompileJob {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = run(&spec);
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    CompileJob { rx }
}

fn run(spec: &CompileSpec) -> CompileResult {
    let t0 = Instant::now();
    let outdir = spec.root.join(&spec.outdir);
    let _ = std::fs::create_dir_all(&outdir);
    // \include{dir/file} writes dir/file.aux into the output directory; TeX cannot
    // create directories, so mirror the project's folder structure there.
    mirror_dirs(&spec.root, &spec.root, &outdir, 0);
    let engine_flag = match spec.engine.as_str() {
        "xelatex" => "-pdfxe",
        "lualatex" => "-pdflua",
        _ => "-pdf",
    };
    let output = crate::platform::cmd("latexmk")
        .current_dir(&spec.root)
        .env("TEXINPUTS", spec.texinputs())
        .env("BIBINPUTS", format!("{}{}", spec.root.display(), crate::platform::PATH_LIST_SEP))
        .env("max_print_line", "1000")
        .env("error_line", "254")
        .env("half_error_line", "238")
        .args([engine_flag, "-synctex=1", "-interaction=nonstopmode", "-file-line-error", "-f", "-g"])
        .arg(format!("-outdir={}", spec.outdir))
        .arg(&spec.main)
        .output();
    let mut raw_log = String::new();
    let mut ok = false;
    match output {
        Ok(o) => {
            ok = o.status.success();
            raw_log.push_str(&String::from_utf8_lossy(&o.stdout));
            raw_log.push_str(&String::from_utf8_lossy(&o.stderr));
        }
        Err(e) => raw_log = format!("latexmk konnte nicht gestartet werden: {e}\nIst TeX Live installiert?"),
    }
    let stem = spec.stem();
    let log = std::fs::read(outdir.join(format!("{stem}.log"))).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    let blg = std::fs::read_to_string(outdir.join(format!("{stem}.blg"))).unwrap_or_default();
    let mut issues = parse_log(&log, &spec.root);
    issues.extend(parse_blg(&blg));
    add_hints(&mut issues, &raw_log);
    if issues.iter().all(|i| i.level != Level::Error) && !ok && log.is_empty() {
        issues.insert(0, Issue { level: Level::Error, file: None, line: None, message: "Kompilieren fehlgeschlagen".into(), context: raw_log.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n") });
    }
    let pdf = spec.pdf_path();
    let pdf = pdf.exists().then_some(pdf);
    if !log.is_empty() {
        raw_log = log;
    }
    CompileResult { ok: ok && pdf.is_some(), pdf, issues, raw_log, duration: t0.elapsed() }
}

fn tool_available(name: &str) -> bool {
    crate::platform::has_tool(name)
}

/// Turn common "missing package/tool" situations into actionable messages.
fn add_hints(issues: &mut Vec<Issue>, latexmk_out: &str) {
    if latexmk_out.contains("Using biber") && !tool_available("biber") {
        issues.insert(
            0,
            Issue {
                level: Level::Error,
                file: None,
                line: None,
                message: "biber ist nicht installiert – das Dokument nutzt biblatex mit backend=biber, daher fehlt das Literaturverzeichnis.".into(),
                context: "Installieren mit:  sudo pacman -S biber".into(),
            },
        );
    }
    let re = regex::Regex::new(r"Unknown option '(\w+)'").unwrap();
    for is in issues.iter_mut() {
        if is.message.contains("babel Error") {
            if let Some(c) = re.captures(&is.message) {
                let lang = &c[1];
                let pkg = match lang {
                    "ngerman" | "german" | "austrian" | "naustrian" => "texlive-langgerman",
                    "french" => "texlive-langfrench",
                    "spanish" => "texlive-langspanish",
                    "italian" => "texlive-langitalian",
                    _ => "texlive-lang",
                };
                is.context = format!("Die Sprache „{lang}“ fehlt in TeX Live. Installieren mit:  sudo pacman -S {pkg}\n\n{}", is.context);
            }
        }
    }
}

fn mirror_dirs(root: &Path, dir: &Path, outdir: &Path, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if !p.is_dir() || name.to_string_lossy().starts_with('.') {
            continue;
        }
        if let Ok(r) = p.strip_prefix(root) {
            let _ = std::fs::create_dir_all(outdir.join(r));
        }
        mirror_dirs(root, &p, outdir, depth + 1);
    }
}

fn rel(root: &Path, file: &str) -> String {
    let f = file.trim_start_matches("./").trim_start_matches(".\\");
    let p = Path::new(f);
    if p.is_absolute() {
        if let Ok(r) = p.strip_prefix(root) {
            return crate::platform::slash(r);
        }
    }
    f.replace('\\', "/")
}

/// Parse a TeX log (compiled with -file-line-error).
pub fn parse_log(log: &str, root: &Path) -> Vec<Issue> {
    let re_fle = regex::Regex::new(r"^(\.?/?[^:\s][^:]*\.(?:tex|sty|cls|bib|bbl|def)):(\d+): (.*)$").unwrap();
    let re_line = regex::Regex::new(r"on input line (\d+)").unwrap();
    let re_box = regex::Regex::new(r"^((?:Over|Under)full \\[hv]box .*?)(?: in paragraph)? at lines? (\d+)").unwrap();
    let re_warn = regex::Regex::new(r"^(?:LaTeX|Package (\S+)|Class (\S+)) Warning: (.*)$").unwrap();

    let lines: Vec<&str> = log.lines().collect();
    let mut out: Vec<Issue> = vec![];
    // Track file stack by scanning parentheses
    let mut stack: Vec<String> = vec![];
    let re_open = regex::Regex::new(r"\((\.?/[^\s()]+\.(?:tex|bbl))").unwrap();

    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        // file tracking (approximate)
        for m in re_open.find_iter(l) {
            stack.push(rel(root, &m.as_str()[1..]));
        }
        let closes = l.matches(')').count().saturating_sub(l.matches('(').count());
        let current = stack.last().cloned();

        if let Some(c) = re_fle.captures(l) {
            let mut ctx = vec![];
            let mut j = i + 1;
            while j < lines.len() && j < i + 8 && !lines[j].trim().is_empty() {
                ctx.push(lines[j]);
                j += 1;
            }
            out.push(Issue {
                level: Level::Error,
                file: Some(rel(root, &c[1])),
                line: c[2].parse().ok(),
                message: c[3].trim().to_string(),
                context: ctx.join("\n"),
            });
        } else if let Some(m) = l.strip_prefix("! ") {
            let mut ln = None;
            let mut ctx = vec![];
            for k in i + 1..(i + 10).min(lines.len()) {
                if let Some(rest) = lines[k].strip_prefix("l.") {
                    ln = rest.split_whitespace().next().and_then(|n| n.parse().ok());
                    ctx.push(lines[k]);
                    break;
                }
                ctx.push(lines[k]);
            }
            out.push(Issue { level: Level::Error, file: current.clone(), line: ln, message: m.to_string(), context: ctx.join("\n") });
        } else if let Some(c) = re_warn.captures(l) {
            // multi-line warnings continue with indented lines
            let mut msg = c[3].to_string();
            let mut j = i + 1;
            while j < lines.len() && lines[j].starts_with(' ') && !lines[j].trim().is_empty() && j < i + 6 {
                msg.push(' ');
                msg.push_str(lines[j].trim());
                j += 1;
            }
            let msg = regex::Regex::new(r"\s*\((?:\w+)\)\s*").unwrap().replace_all(&msg, " ").to_string();
            let line = re_line.captures(&msg).and_then(|c| c[1].parse().ok());
            let pkg = c.get(1).or(c.get(2)).map(|m| format!("{}: ", m.as_str())).unwrap_or_default();
            out.push(Issue { level: Level::Warning, file: current.clone(), line, message: format!("{pkg}{}", msg.trim()), context: String::new() });
        } else if let Some(c) = re_box.captures(l) {
            out.push(Issue { level: Level::BadBox, file: current.clone(), line: c[2].parse().ok(), message: c[1].to_string(), context: String::new() });
        }
        for _ in 0..closes {
            stack.pop();
        }
        i += 1;
    }
    // dedupe
    let mut seen = std::collections::HashSet::new();
    out.retain(|x| seen.insert((x.level, x.file.clone(), x.line, x.message.clone())));
    out.sort_by_key(|x| x.level);
    out
}

fn parse_blg(blg: &str) -> Vec<Issue> {
    blg.lines()
        .filter_map(|l| {
            let w = l.strip_prefix("Warning--")?;
            Some(Issue { level: Level::Warning, file: Some("references.bib".into()), line: None, message: format!("BibTeX: {w}"), context: String::new() })
        })
        .collect()
}

// ───────────────────────────── SyncTeX ─────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct SyncBox {
    pub page: usize, // 0-based
    pub x: f32,
    pub y: f32, // top in pt
    pub w: f32,
    pub h: f32,
}

/// Source → PDF.
pub fn synctex_view(root: &Path, file_rel: &str, line: usize, pdf: &Path) -> Option<SyncBox> {
    let input = root.join(file_rel);
    let out = crate::platform::cmd("synctex")
        .current_dir(root)
        .arg("view")
        .arg("-i")
        .arg(format!("{}:1:{}", line, input.display()))
        .arg("-o")
        .arg(pdf)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut page = None;
    let (mut h, mut v, mut w, mut hh) = (0.0, 0.0, 0.0, 0.0);
    for l in s.lines() {
        if let Some((k, val)) = l.split_once(':') {
            let f = val.trim().parse::<f32>().unwrap_or(0.0);
            match k {
                "Page" => {
                    if page.is_some() {
                        break; // first record only
                    }
                    page = val.trim().parse::<usize>().ok();
                }
                "h" => h = f,
                "v" => v = f,
                "W" => w = f,
                "H" => hh = f,
                _ => {}
            }
        }
    }
    let page = page?;
    Some(SyncBox { page: page.saturating_sub(1), x: h, y: v - hh, w, h: hh.max(10.0) })
}

/// PDF → source. Returns (relative file, 1-based line).
pub fn synctex_edit(root: &Path, page: usize, x: f32, y: f32, pdf: &Path) -> Option<(String, usize)> {
    let out = crate::platform::cmd("synctex")
        .current_dir(root)
        .arg("edit")
        .arg("-o")
        .arg(format!("{}:{}:{}:{}", page + 1, x, y, pdf.display()))
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut file = None;
    let mut line = None;
    for l in s.lines() {
        if let Some(v) = l.strip_prefix("Input:") {
            if file.is_none() {
                file = Some(v.trim().to_string());
            }
        } else if let Some(v) = l.strip_prefix("Line:") {
            if line.is_none() {
                line = v.trim().parse::<usize>().ok();
            }
        }
    }
    let file = file?;
    let p = PathBuf::from(&file);
    let canon_root = root.canonicalize().unwrap_or(root.to_path_buf());
    let canon = p.canonicalize().unwrap_or(p.clone());
    let r = canon
        .strip_prefix(&canon_root)
        .map(crate::platform::slash)
        .unwrap_or_else(|_| rel(root, &file));
    Some((r, line?.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs TeX Live"]
    fn error_is_located_in_subfile() {
        let root = std::env::temp_dir().join(format!("nedit-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("kapitel")).unwrap();
        std::fs::write(root.join("main.tex"), "\\documentclass{article}\n\\begin{document}\nHallo\n\n\\input{kapitel/a}\n\\end{document}\n").unwrap();
        std::fs::write(root.join("kapitel/a.tex"), "Zeile eins\n\nZeile drei \\undefinedmacro\n").unwrap();
        let spec = CompileSpec { root: root.clone(), main: "main.tex".into(), outdir: ".nedit/build".into(), engine: "pdflatex".into() };
        let r = run(&spec);
        let e = r.issues.iter().find(|i| i.level == Level::Error).expect("error expected");
        assert_eq!(e.file.as_deref(), Some("kapitel/a.tex"));
        assert_eq!(e.line, Some(3));
        assert!(r.pdf.is_some(), "PDF should still be produced with -f");
        // SyncTeX round trip
        let b = synctex_view(&root, "kapitel/a.tex", 1, &r.pdf.clone().unwrap()).expect("synctex view");
        let (f, _l) = synctex_edit(&root, b.page, b.x + b.w * 0.3, b.y + b.h * 0.5, &r.pdf.unwrap()).expect("synctex edit");
        assert_eq!(f, "kapitel/a.tex");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "needs TeX Live"]
    fn include_from_subdirectory_works() {
        let root = std::env::temp_dir().join(format!("nedit-incl-{}", std::process::id()));
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("main.tex"), "\\documentclass{report}\n\\begin{document}\n\\include{content/intro}\n\\end{document}\n").unwrap();
        std::fs::write(root.join("content/intro.tex"), "\\chapter{Intro}\nText.\n").unwrap();
        let spec = CompileSpec { root: root.clone(), main: "main.tex".into(), outdir: ".nedit/build".into(), engine: "pdflatex".into() };
        let r = run(&spec);
        assert!(r.issues.iter().all(|i| i.level != Level::Error), "{:?}", r.issues);
        assert!(r.pdf.is_some());
        let _ = std::fs::remove_dir_all(root);
    }
}
