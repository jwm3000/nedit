//! User templates: imported from a ZIP (e.g. Overleaf "Download → Source"), from a folder,
//! or saved from the current project. Stored in `<data dir>/nedit/templates/<id>/` with a
//! `template.json` describing name, icon and the main documents.

use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UserTemplate {
    /// folder name
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub icon: String,
    pub thesis_main: String,
    pub slides_main: Option<String>,
    /// where it came from ("Overleaf-ZIP", "Ordner", "Projekt")
    pub source: String,
}

impl Default for UserTemplate {
    fn default() -> Self {
        UserTemplate { id: String::new(), name: String::new(), icon: crate::icons::FILE_TEXT.into(), thesis_main: "main.tex".into(), slides_main: None, source: String::new() }
    }
}

pub fn templates_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("NEDIT_TEMPLATES") {
        return PathBuf::from(p);
    }
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("nedit").join("templates")
}

pub fn list() -> Vec<UserTemplate> {
    let mut v: Vec<UserTemplate> = std::fs::read_dir(templates_dir())
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let mut t: UserTemplate = serde_json::from_str(&std::fs::read_to_string(p.join("template.json")).ok()?).ok()?;
                    t.id = e.file_name().to_string_lossy().to_string();
                    Some(t)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|t| t.name.to_lowercase());
    v
}

pub fn delete(id: &str) -> std::io::Result<()> {
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return Err(std::io::Error::other("invalid template id"));
    }
    std::fs::remove_dir_all(templates_dir().join(id))
}

/// Files that never belong into a template.
fn skip(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or("");
    if matches!(first, ".nedit" | ".git" | "__MACOSX" | "build" | "out") || rel.split('/').any(|c| c == ".DS_Store") {
        return true;
    }
    let lower = rel.to_lowercase();
    [".aux", ".log", ".toc", ".out", ".bbl", ".blg", ".bcf", ".run.xml", ".fls", ".fdb_latexmk", ".synctex.gz", ".lof", ".lot", ".nav", ".snm", ".xdv"]
        .iter()
        .any(|e| lower.ends_with(e))
}

fn slug(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "vorlage".into() } else { s }
}

/// A fresh, unused folder for a template called `name`.
fn new_dir(name: &str) -> PathBuf {
    let base = templates_dir();
    let s = slug(name);
    let mut p = base.join(&s);
    let mut n = 2;
    while p.exists() {
        p = base.join(format!("{s}-{n}"));
        n += 1;
    }
    p
}

/// Main documents: the thesis (has \begin{document}, prefers main.tex and
/// files in the top folder) and an optional beamer presentation.
pub fn detect_mains(dir: &Path) -> (Option<String>, Option<String>) {
    let mut thesis: Vec<(i32, String)> = vec![];
    let mut slides: Vec<(i32, String)> = vec![];
    walk(dir, dir, &mut |rel, path| {
        if !rel.ends_with(".tex") {
            return;
        }
        let Ok(t) = std::fs::read_to_string(path) else { return };
        let code: String = t.lines().filter(|l| !l.trim_start().starts_with('%')).collect::<Vec<_>>().join("\n");
        // \begin{document} makes a main document (\documentclass is often in an \input file)
        if !code.contains("\\begin{document}") {
            return;
        }
        let depth = rel.matches('/').count() as i32;
        let beamer = regex::Regex::new(r"\\documentclass(\[[^\]]*\])?\{beamer\}").unwrap().is_match(&code);
        let mut score = 10 - depth * 3;
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if matches!(name, "main.tex" | "thesis.tex" | "masterarbeit.tex" | "arbeit.tex") {
            score += 5;
        }
        if beamer {
            slides.push((score, rel.to_string()));
        } else {
            thesis.push((score, rel.to_string()));
        }
    });
    thesis.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    slides.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    (thesis.first().map(|t| t.1.clone()), slides.first().map(|t| t.1.clone()))
}

fn walk(root: &Path, dir: &Path, f: &mut dyn FnMut(&str, &Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let Ok(rel) = p.strip_prefix(root) else { continue };
        let rel = crate::platform::slash(rel);
        if skip(&rel) {
            continue;
        }
        if p.is_dir() {
            walk(root, &p, f);
        } else {
            f(&rel, &p);
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    let mut err = None;
    walk(from, from, &mut |rel, path| {
        let target = to.join(rel);
        if let Some(d) = target.parent() {
            if let Err(e) = std::fs::create_dir_all(d) {
                err = Some(e);
                return;
            }
        }
        match std::fs::copy(path, &target) {
            Ok(_) => n += 1,
            Err(e) => err = Some(e),
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(n),
    }
}

fn finish(dir: &Path, name: &str, icon: &str, source: &str) -> std::io::Result<UserTemplate> {
    let (thesis, slides) = detect_mains(dir);
    let Some(thesis_main) = thesis.or_else(|| slides.clone()) else {
        let _ = std::fs::remove_dir_all(dir);
        return Err(std::io::Error::other(tr!("Kein LaTeX-Hauptdokument gefunden (\\documentclass … \\begin{document})" | "No LaTeX main document found (\\documentclass … \\begin{document})")));
    };
    let slides_main = slides.filter(|s| *s != thesis_main);
    let t = UserTemplate {
        id: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        name: name.trim().to_string(),
        icon: icon.to_string(),
        thesis_main,
        slides_main,
        source: source.to_string(),
    };
    std::fs::write(dir.join("template.json"), serde_json::to_string_pretty(&t).unwrap_or_default())?;
    Ok(t)
}

/// Import a ZIP (Overleaf: Menu → Download → Source). A single top folder is removed.
pub fn import_zip(zip: &Path, name: &str, icon: &str) -> std::io::Result<UserTemplate> {
    let file = std::fs::File::open(zip)?;
    let mut ar = zip::ZipArchive::new(file).map_err(std::io::Error::other)?;
    let mut names = vec![];
    for i in 0..ar.len() {
        let f = ar.by_index(i).map_err(std::io::Error::other)?;
        if let Some(p) = f.enclosed_name() {
            if !f.is_dir() {
                names.push(crate::platform::slash(&p));
            }
        }
    }
    // common top folder ("MyThesis/main.tex", "MyThesis/…") → strip it
    let prefix = {
        let firsts: std::collections::HashSet<&str> = names.iter().filter(|n| !skip(n)).map(|n| n.split('/').next().unwrap_or("")).collect();
        let all_nested = names.iter().filter(|n| !skip(n)).all(|n| n.contains('/'));
        if firsts.len() == 1 && all_nested { firsts.into_iter().next().map(|s| format!("{s}/")) } else { None }
    };
    let dir = new_dir(name);
    std::fs::create_dir_all(&dir)?;
    for i in 0..ar.len() {
        let mut f = ar.by_index(i).map_err(std::io::Error::other)?;
        let Some(p) = f.enclosed_name() else { continue };
        if f.is_dir() {
            continue;
        }
        let mut rel = crate::platform::slash(&p);
        if let Some(pre) = &prefix {
            rel = rel.strip_prefix(pre.as_str()).unwrap_or(&rel).to_string();
        }
        if rel.is_empty() || skip(&rel) {
            continue;
        }
        let target = dir.join(&rel);
        if let Some(d) = target.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut buf = vec![];
        f.read_to_end(&mut buf)?;
        std::fs::write(target, buf)?;
    }
    finish(&dir, name, icon, "ZIP")
}

/// Import a folder (copied, without build files).
pub fn import_dir(src: &Path, name: &str, icon: &str) -> std::io::Result<UserTemplate> {
    let dir = new_dir(name);
    std::fs::create_dir_all(&dir)?;
    copy_tree(src, &dir)?;
    finish(&dir, name, icon, tr!("Ordner" | "Folder"))
}

/// Save the current project as template.
pub fn save_project(root: &Path, name: &str, icon: &str) -> std::io::Result<UserTemplate> {
    let dir = new_dir(name);
    std::fs::create_dir_all(&dir)?;
    copy_tree(root, &dir)?;
    finish(&dir, name, icon, tr!("Projekt" | "Project"))
}

/// Copy a template into a (new) project folder; returns (thesis main, slides main).
pub fn instantiate(t: &UserTemplate, root: &Path) -> std::io::Result<(String, Option<String>)> {
    let src = templates_dir().join(&t.id);
    let mut err = None;
    walk(&src, &src, &mut |rel, path| {
        if rel == "template.json" {
            return;
        }
        let target = root.join(rel);
        if target.exists() {
            return;
        }
        if let Some(d) = target.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::copy(path, &target) {
            err = Some(e);
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok((t.thesis_main.clone(), t.slides_main.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn overleaf_zip_round_trip() {
        let base = std::env::temp_dir().join(format!("nedit-tpl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // SAFETY: test-only, single-threaded use of the env var
        unsafe { std::env::set_var("NEDIT_TEMPLATES", base.join("templates")) };
        // an Overleaf-like ZIP with a top folder, a figure, build junk and slides
        let zp = base.join("overleaf.zip");
        {
            let f = std::fs::File::create(&zp).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            for (n, c) in [
                ("MyThesis/main.tex", "\\input{preamble}\n\\begin{document}\nHi\n\\end{document}\n"),
                ("MyThesis/preamble.tex", "\\documentclass{report}\n"),
                ("MyThesis/chapters/intro.tex", "\\chapter{Intro}\n"),
                ("MyThesis/figures/a.png", "PNG"),
                ("MyThesis/main.aux", "junk"),
                ("MyThesis/slides/talk.tex", "\\documentclass[aspectratio=169]{beamer}\n\\begin{document}\n\\end{document}\n"),
                ("__MACOSX/MyThesis/._main.tex", "junk"),
            ] {
                z.start_file(n, o).unwrap();
                z.write_all(c.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        let t = import_zip(&zp, "Overleaf Vorlage", crate::icons::BOOK).unwrap();
        assert_eq!(t.thesis_main, "main.tex");
        assert_eq!(t.slides_main.as_deref(), Some("slides/talk.tex"));
        let dir = templates_dir().join(&t.id);
        assert!(dir.join("chapters/intro.tex").exists() && dir.join("figures/a.png").exists());
        assert!(!dir.join("main.aux").exists() && !dir.join("__MACOSX").exists());
        assert_eq!(list().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Overleaf Vorlage"]);
        // a project from it
        let proj = base.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let (m, s) = instantiate(&t, &proj).unwrap();
        assert_eq!((m.as_str(), s.as_deref()), ("main.tex", Some("slides/talk.tex")));
        assert!(proj.join("chapters/intro.tex").exists() && !proj.join("template.json").exists());
        // saving that project again as a template (build output is left out)
        std::fs::create_dir_all(proj.join(".nedit/build")).unwrap();
        std::fs::write(proj.join(".nedit/build/x.pdf"), "x").unwrap();
        let t2 = save_project(&proj, "Overleaf Vorlage", crate::icons::STAR).unwrap();
        assert_ne!(t2.id, t.id, "second template gets its own folder");
        assert!(!templates_dir().join(&t2.id).join(".nedit").exists());
        delete(&t2.id).unwrap();
        assert_eq!(list().len(), 1);
        // a ZIP without a LaTeX document is rejected and leaves nothing behind
        let bad = base.join("bad.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&bad).unwrap());
            z.start_file("readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        assert!(import_zip(&bad, "Bad", crate::icons::BOOK).is_err());
        assert_eq!(list().len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }
}
