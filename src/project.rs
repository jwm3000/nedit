//! Projects on disk, templates, file tree, outline and label scanning.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub name: String,
    pub thesis_main: String,
    pub slides_main: String,
    pub engine: String,
    pub slides_engine: String,
    pub talk_minutes: u32,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        ProjectConfig {
            name: "Projekt".into(),
            thesis_main: "main.tex".into(),
            slides_main: "praesentation/folien.tex".into(),
            engine: "pdflatex".into(),
            slides_engine: "pdflatex".into(),
            talk_minutes: 20,
        }
    }
}

pub struct Project {
    pub root: PathBuf,
    pub config: ProjectConfig,
}

const TEMPLATE: &[(&str, &str)] = &[
    ("main.tex", include_str!("../templates/masterarbeit/main.tex")),
    ("references.bib", include_str!("../templates/masterarbeit/references.bib")),
    ("kapitel/titelseite.tex", include_str!("../templates/masterarbeit/kapitel/titelseite.tex")),
    ("kapitel/abstract.tex", include_str!("../templates/masterarbeit/kapitel/abstract.tex")),
    ("kapitel/einleitung.tex", include_str!("../templates/masterarbeit/kapitel/einleitung.tex")),
    ("kapitel/grundlagen.tex", include_str!("../templates/masterarbeit/kapitel/grundlagen.tex")),
    ("kapitel/methodik.tex", include_str!("../templates/masterarbeit/kapitel/methodik.tex")),
    ("kapitel/ergebnisse.tex", include_str!("../templates/masterarbeit/kapitel/ergebnisse.tex")),
    ("kapitel/fazit.tex", include_str!("../templates/masterarbeit/kapitel/fazit.tex")),
    ("kapitel/anhang.tex", include_str!("../templates/masterarbeit/kapitel/anhang.tex")),
    ("kapitel/deutsch-fallback.tex", include_str!("../templates/masterarbeit/kapitel/deutsch-fallback.tex")),
    ("praesentation/folien.tex", include_str!("../templates/masterarbeit/praesentation/folien.tex")),
    ("praesentation/beamerthemeTUGraz.sty", include_str!("../templates/masterarbeit/praesentation/beamerthemeTUGraz.sty")),
    ("abbildungen/.keep", ""),
    ("papers/.keep", ""),
];

pub fn projects_dir() -> PathBuf {
    if let Ok(p) = std::env::var("NEDIT_PROJECTS") {
        return PathBuf::from(p);
    }
    dirs::document_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Documents")).join("nEdit-Projekte")
}

pub fn list_projects() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(projects_dir())
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join(".nedit/project.json").exists() || p.join("main.tex").exists()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

impl Project {
    pub fn open(root: &Path) -> std::io::Result<Self> {
        let cfg_path = root.join(".nedit/project.json");
        let config: ProjectConfig = std::fs::read_to_string(&cfg_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| ProjectConfig { name: root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), ..Default::default() });
        let p = Project { root: root.to_path_buf(), config };
        p.save_config()?;
        Ok(p)
    }

    pub fn create(name: &str, author: &str) -> std::io::Result<Self> {
        let slug: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        let root = projects_dir().join(slug);
        if root.join(".nedit/project.json").exists() {
            return Project::open(&root);
        }
        std::fs::create_dir_all(root.join(".nedit"))?;
        for (rel, content) in TEMPLATE {
            let p = root.join(rel);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            if !p.exists() {
                let c = if author.is_empty() { content.to_string() } else { content.replace("Norbert Winter", author) };
                std::fs::write(&p, c)?;
            }
        }
        std::fs::write(root.join(".gitignore"), ".nedit/build/\n")?;
        let p = Project { root, config: ProjectConfig { name: name.to_string(), ..Default::default() } };
        p.save_config()?;
        Ok(p)
    }

    /// Writes `.nedit/project.json` only if its content actually changes (keeps git clean).
    pub fn save_config(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.root.join(".nedit"))?;
        let path = self.root.join(".nedit/project.json");
        let new = serde_json::to_string_pretty(&self.config).unwrap_or_default();
        let same = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<ProjectConfig>(&s).ok()).is_some_and(|c| serde_json::to_string_pretty(&c).unwrap_or_default() == new);
        if same {
            return Ok(());
        }
        std::fs::write(path, new)
    }

    pub fn has_slides(&self) -> bool {
        self.root.join(&self.config.slides_main).exists()
    }
}

// ───────────────────────────── file tree ─────────────────────────────

#[derive(Clone, Debug)]
pub struct FileNode {
    pub name: String,
    pub rel: String,
    pub is_dir: bool,
    pub children: Vec<FileNode>,
}

pub fn file_tree(root: &Path) -> Vec<FileNode> {
    fn walk(root: &Path, dir: &Path, depth: usize) -> Vec<FileNode> {
        let mut out = vec![];
        let Ok(rd) = std::fs::read_dir(dir) else { return out };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.ends_with(".aux") || name.ends_with(".synctex.gz") {
                continue;
            }
            let p = e.path();
            let rel = p.strip_prefix(root).map(crate::platform::slash).unwrap_or_default();
            let is_dir = p.is_dir();
            let children = if is_dir && depth < 6 { walk(root, &p, depth + 1) } else { vec![] };
            out.push(FileNode { name, rel, is_dir, children });
        }
        out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(natural_key(&a.name).cmp(&natural_key(&b.name))));
        out
    }
    walk(root, root, 0)
}

fn natural_key(s: &str) -> String {
    s.to_lowercase()
}

pub fn flat_files(nodes: &[FileNode], out: &mut Vec<String>) {
    for n in nodes {
        if n.is_dir {
            flat_files(&n.children, out);
        } else {
            out.push(n.rel.clone());
        }
    }
}

pub fn is_text_file(rel: &str) -> bool {
    let l = rel.to_lowercase();
    [".tex", ".bib", ".sty", ".cls", ".txt", ".md", ".bst", ".cfg", ".def", ".csv", ".dat", ".gitignore", ".lua", ".py"].iter().any(|e| l.ends_with(e))
}

// ───────────────────────────── outline ─────────────────────────────

#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub level: usize,
    pub title: String,
    pub file: String,
    pub line: usize,
}

const LEVELS: &[(&str, usize)] = &[
    ("part", 0),
    ("chapter", 1),
    ("section", 2),
    ("subsection", 3),
    ("subsubsection", 4),
    ("frametitle", 3),
];

/// Walk from `main` following \input/\include, using `read` to get file contents.
pub fn outline(main: &str, read: &dyn Fn(&str) -> Option<String>) -> Vec<OutlineItem> {
    let re = regex::Regex::new(r"\\(part|chapter|section|subsection|subsubsection|input|include|begin\{frame\}(?:\[[^\]]*\])?)\*?(?:\[[^\]]*\])?\{([^}]*)\}").unwrap();
    let mut out = vec![];
    let mut stack = vec![(main.to_string(), 0usize)];
    let mut seen = std::collections::HashSet::new();
    fn go(file: &str, depth: usize, re: &regex::Regex, read: &dyn Fn(&str) -> Option<String>, out: &mut Vec<OutlineItem>, seen: &mut std::collections::HashSet<String>) {
        if depth > 8 || !seen.insert(file.to_string()) {
            return;
        }
        let Some(text) = read(file) else { return };
        let base = Path::new(file).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let _ = base;
        for (i, line) in text.lines().enumerate() {
            let code = strip_comment(line);
            for c in re.captures_iter(code) {
                let cmd = &c[1];
                let arg = c[2].trim();
                if cmd == "input" || cmd == "include" {
                    let mut f = arg.to_string();
                    if !f.ends_with(".tex") {
                        f.push_str(".tex");
                    }
                    go(&f, depth + 1, re, read, out, seen);
                } else if cmd.starts_with("begin{frame}") {
                    out.push(OutlineItem { level: 3, title: crate::bib::clean(arg), file: file.to_string(), line: i + 1 });
                } else if let Some((_, lvl)) = LEVELS.iter().find(|(n, _)| *n == cmd) {
                    out.push(OutlineItem { level: *lvl, title: crate::bib::clean(arg), file: file.to_string(), line: i + 1 });
                }
            }
        }
    }
    while let Some((f, d)) = stack.pop() {
        go(&f, d, &re, read, &mut out, &mut seen);
    }
    out
}

pub fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'%' && (i == 0 || b[i - 1] != b'\\') {
            return &line[..i];
        }
    }
    line
}

pub fn scan_labels(texts: &[String]) -> Vec<String> {
    let re = regex::Regex::new(r"\\label\{([^}]+)\}").unwrap();
    let mut v: Vec<String> = texts
        .iter()
        .flat_map(|t| t.lines().flat_map(|l| re.captures_iter(strip_comment(l)).map(|c| c[1].to_string()).collect::<Vec<_>>()))
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn word_count(text: &str) -> usize {
    let mut n = 0;
    for line in text.lines() {
        let l = strip_comment(line);
        n += l.split_whitespace().filter(|w| !w.starts_with('\\') && w.chars().any(|c| c.is_alphabetic())).count();
    }
    n
}

/// Files that make up the document body, in reading order (following \input/\include
/// after \begin{document}). Falls back to the main file for single-file documents.
pub fn document_files(main: &str, read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let re = regex::Regex::new(r"\\(input|include|subfile)\{([^}\\]+)\}").unwrap();
    fn resolve(name: &str, read: &dyn Fn(&str) -> Option<String>) -> Option<(String, String)> {
        let n = name.trim().trim_start_matches("./");
        let cand = if n.ends_with(".tex") { n.to_string() } else { format!("{n}.tex") };
        read(&cand).map(|t| (cand, t))
    }
    fn go(text: &str, re: &regex::Regex, read: &dyn Fn(&str) -> Option<String>, out: &mut Vec<String>, depth: usize) {
        if depth > 6 {
            return;
        }
        for line in text.lines() {
            for c in re.captures_iter(strip_comment(line)) {
                if let Some((f, t)) = resolve(&c[2], read) {
                    let internal = f.split('/').any(|d| matches!(d, "template" | "templates" | "style" | "preamble" | "setup"))
                        || t.contains("\\begin{titlepage}");
                    if !out.contains(&f) && !internal {
                        out.push(f);
                        go(&t, re, read, out, depth + 1);
                    }
                }
            }
        }
    }
    let Some(text) = read(main) else { return vec![] };
    let body = text.find("\\begin{document}").map(|p| &text[p..]).unwrap_or(&text);
    let mut out = vec![];
    go(body, &re, read, &mut out, 0);
    if out.is_empty() {
        out.push(main.to_string());
    }
    out
}
