//! The paper shelf: BibTeX entries (stored in `references.bib`) plus per-paper
//! metadata (attached PDF, reading status, tags, notes) in `.nedit/shelf.json`.

use crate::bib::{self, BibEntry, BibFile};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadStatus {
    #[default]
    Unread,
    Reading,
    Read,
}

impl ReadStatus {
    pub fn label(self) -> &'static str {
        match self {
            ReadStatus::Unread => tr!("Ungelesen" | "Unread"),
            ReadStatus::Reading => tr!("In Arbeit" | "Reading"),
            ReadStatus::Read => tr!("Gelesen" | "Read"),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PaperMeta {
    pub pdf: Option<String>,
    pub status: ReadStatus,
    pub tags: Vec<String>,
    pub notes: String,
    pub added: u64,
    pub favorite: bool,
}

#[derive(Clone, Debug)]
pub struct Paper {
    pub entry: BibEntry,
    pub meta: PaperMeta,
}

pub struct Shelf {
    pub root: PathBuf,
    pub papers: Vec<Paper>,
    extras: Vec<String>,
    bib_stamp: Option<SystemTime>,
    pub revision: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Shelf {
    pub fn bib_path(root: &Path) -> PathBuf {
        root.join("references.bib")
    }
    fn meta_path(root: &Path) -> PathBuf {
        root.join(".nedit/shelf.json")
    }

    pub fn load(root: &Path) -> Self {
        let mut s = Shelf { root: root.to_path_buf(), papers: vec![], extras: vec![], bib_stamp: None, revision: 0 };
        s.reload();
        s
    }

    pub fn reload(&mut self) {
        let bp = Self::bib_path(&self.root);
        let src = std::fs::read_to_string(&bp).unwrap_or_default();
        let file = bib::parse(&src);
        let meta: HashMap<String, PaperMeta> = std::fs::read_to_string(Self::meta_path(&self.root))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        self.papers = file
            .entries
            .into_iter()
            .map(|e| {
                let m = meta.get(&e.key).cloned().unwrap_or_default();
                Paper { entry: e, meta: m }
            })
            .collect();
        self.extras = file.extras;
        self.bib_stamp = std::fs::metadata(&bp).and_then(|m| m.modified()).ok();
        self.revision += 1;
    }

    /// Reload if `references.bib` changed on disk (e.g. edited in the editor).
    pub fn poll_external_change(&mut self) -> bool {
        let st = std::fs::metadata(Self::bib_path(&self.root)).and_then(|m| m.modified()).ok();
        if st != self.bib_stamp {
            self.reload();
            return true;
        }
        false
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let file = BibFile { entries: self.papers.iter().map(|p| p.entry.clone()).collect(), extras: self.extras.clone() };
        let bp = Self::bib_path(&self.root);
        std::fs::write(&bp, bib::serialize(&file))?;
        let meta: HashMap<String, PaperMeta> = self.papers.iter().map(|p| (p.entry.key.clone(), p.meta.clone())).collect();
        std::fs::create_dir_all(self.root.join(".nedit"))?;
        std::fs::write(Self::meta_path(&self.root), serde_json::to_string_pretty(&meta).unwrap_or_default())?;
        self.bib_stamp = std::fs::metadata(&bp).and_then(|m| m.modified()).ok();
        self.revision += 1;
        Ok(())
    }

    pub fn unique_key(&self, base: &str) -> String {
        if !self.papers.iter().any(|p| p.entry.key == base) {
            return base.to_string();
        }
        for c in 'a'..='z' {
            let k = format!("{base}{c}");
            if !self.papers.iter().any(|p| p.entry.key == k) {
                return k;
            }
        }
        format!("{base}{}", now())
    }

    /// Adds an entry with a freshly generated key; returns the key.
    /// If an entry with the same DOI already exists, returns its key instead.
    pub fn add(&mut self, mut entry: BibEntry, pdf_source: Option<&Path>) -> String {
        if let Some(doi) = entry.doi() {
            if let Some(p) = self.papers.iter_mut().find(|p| p.entry.doi().is_some_and(|d| d.eq_ignore_ascii_case(&doi))) {
                let key = p.entry.key.clone();
                if let Some(src) = pdf_source {
                    if p.meta.pdf.is_none() {
                        p.meta.pdf = copy_pdf(&self.root, src, &key);
                    }
                }
                let _ = self.save();
                return key;
            }
        }
        let keep = !entry.key.is_empty() && !self.papers.iter().any(|p| p.entry.key == entry.key) && entry.key.chars().all(|c| c.is_ascii_alphanumeric() || "_-:.".contains(c));
        if !keep {
            entry.key = self.unique_key(&entry.suggest_key());
        }
        if entry.kind.is_empty() {
            entry.kind = "article".into();
        }
        let mut meta = PaperMeta { added: now(), ..Default::default() };
        if let Some(src) = pdf_source {
            meta.pdf = copy_pdf(&self.root, src, &entry.key);
        }
        let key = entry.key.clone();
        self.papers.insert(0, Paper { entry, meta });
        let _ = self.save();
        key
    }

    pub fn attach_pdf(&mut self, idx: usize, src: &Path) {
        let key = self.papers[idx].entry.key.clone();
        self.papers[idx].meta.pdf = copy_pdf(&self.root, src, &key);
        let _ = self.save();
    }

    pub fn remove(&mut self, idx: usize) {
        if idx < self.papers.len() {
            self.papers.remove(idx);
            let _ = self.save();
        }
    }

    pub fn all_tags(&self) -> Vec<String> {
        let mut t: Vec<String> = self.papers.iter().flat_map(|p| p.meta.tags.iter().cloned()).collect();
        t.sort();
        t.dedup();
        t
    }
}

fn copy_pdf(root: &Path, src: &Path, key: &str) -> Option<String> {
    let dir = root.join("papers");
    std::fs::create_dir_all(&dir).ok()?;
    let rel = format!("papers/{key}.pdf");
    let dst = root.join(&rel);
    if src != dst {
        std::fs::copy(src, &dst).ok()?;
    }
    Some(rel)
}

// ───────────────────────────── background lookups ─────────────────────────────

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub doi: String,
    pub title: String,
    pub authors: String,
    pub year: String,
    pub venue: String,
}

pub enum ShelfMsg {
    Status(String),
    Fetched { entry: BibEntry, pdf: Option<PathBuf> },
    SearchResults(Vec<SearchHit>),
    Attach { key: String, path: PathBuf },
    Error(String),
}

pub enum Query {
    Doi(String),
    Arxiv(String),
    Search(String),
}

pub fn classify(input: &str) -> Query {
    let s = input.trim();
    let doi_re = regex::Regex::new(r"(?i)(10\.\d{4,9}/[^\s]+)").unwrap();
    let arxiv_re = regex::Regex::new(r"(?i)(?:arxiv[:/\s]*|abs/|pdf/)?(\d{4}\.\d{4,5})(v\d+)?").unwrap();
    if let Some(c) = doi_re.captures(s) {
        return Query::Doi(c[1].trim_end_matches(['.', ',', ';']).to_string());
    }
    if let Some(c) = arxiv_re.captures(s) {
        if s.len() < 60 {
            return Query::Arxiv(c[1].to_string());
        }
    }
    Query::Search(s.to_string())
}

fn http_get(url: &str, accept: &str) -> Result<String, String> {
    let mut resp = ureq::get(url)
        .header("Accept", accept)
        .header("User-Agent", "nEdit/0.1 (LaTeX editor; mailto:nedit@localhost)")
        .call()
        .map_err(|e| trf!("Netzwerkfehler: {e}" | "Network error: {e}"))?;
    resp.body_mut().read_to_string().map_err(|e| trf!("Antwort unlesbar: {e}" | "Unreadable response: {e}"))
}

pub fn fetch_doi(doi: &str) -> Result<BibEntry, String> {
    let url = format!("https://doi.org/{}", doi.trim());
    let txt = http_get(&url, "application/x-bibtex; charset=utf-8")?;
    let mut f = bib::parse(&txt);
    let mut e = f.entries.pop().ok_or_else(|| tr!("Kein BibTeX für diese DOI gefunden" | "No BibTeX found for this DOI").to_string())?;
    if e.get("doi").is_none() {
        e.set("doi", doi);
    }
    e.fields.retain(|(k, v)| !v.trim().is_empty() && !matches!(k.as_str(), "keywords" | "copyright" | "issn"));
    e.key.clear(); // generate our own readable key
    Ok(e)
}

pub fn search_crossref(q: &str) -> Result<Vec<SearchHit>, String> {
    let enc: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    let url = format!(
        "https://api.crossref.org/works?query.bibliographic={enc}&rows=8&select=DOI,title,author,issued,container-title,publisher"
    );
    let txt = http_get(&url, "application/json")?;
    let v: serde_json::Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    let items = v["message"]["items"].as_array().cloned().unwrap_or_default();
    Ok(items
        .iter()
        .map(|it| {
            let authors: Vec<String> = it["author"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x["family"].as_str().map(String::from)).collect())
                .unwrap_or_default();
            let authors = match authors.len() {
                0 => String::new(),
                1 => authors[0].clone(),
                2 => format!("{} & {}", authors[0], authors[1]),
                _ => format!("{} et al.", authors[0]),
            };
            SearchHit {
                doi: it["DOI"].as_str().unwrap_or("").to_string(),
                title: it["title"][0].as_str().unwrap_or(tr!("(ohne Titel)" | "(untitled)")).to_string(),
                authors,
                year: it["issued"]["date-parts"][0][0].as_i64().map(|y| y.to_string()).unwrap_or_default(),
                venue: it["container-title"][0].as_str().or(it["publisher"].as_str()).unwrap_or("").to_string(),
            }
        })
        .filter(|h| !h.doi.is_empty())
        .collect())
}

pub fn run_query(input: String, tx: Sender<ShelfMsg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let msg = match classify(&input) {
            Query::Doi(d) => {
                let _ = tx.send(ShelfMsg::Status(trf!("Lade DOI {d} …" | "Loading DOI {d} …")));
                match fetch_doi(&d) {
                    Ok(entry) => ShelfMsg::Fetched { entry, pdf: None },
                    Err(e) => ShelfMsg::Error(e),
                }
            }
            Query::Arxiv(id) => {
                let _ = tx.send(ShelfMsg::Status(trf!("Lade arXiv:{id} …" | "Loading arXiv:{id} …")));
                match fetch_doi(&format!("10.48550/arXiv.{id}")) {
                    Ok(mut entry) => {
                        if entry.get("eprint").is_none() {
                            entry.set("eprint", &id);
                            entry.set("archiveprefix", "arXiv");
                        }
                        ShelfMsg::Fetched { entry, pdf: None }
                    }
                    Err(e) => ShelfMsg::Error(e),
                }
            }
            Query::Search(q) => {
                let _ = tx.send(ShelfMsg::Status(tr!("Suche bei Crossref …" | "Searching Crossref …").into()));
                match search_crossref(&q) {
                    Ok(h) => ShelfMsg::SearchResults(h),
                    Err(e) => ShelfMsg::Error(e),
                }
            }
        };
        let _ = tx.send(msg);
        ctx.request_repaint();
    });
}

/// Import PDFs: try to find a DOI / arXiv id in the text, otherwise fall back to
/// the PDF's own metadata.
pub fn import_pdfs(files: Vec<PathBuf>, tx: Sender<ShelfMsg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        for f in files {
            let name = f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let _ = tx.send(ShelfMsg::Status(trf!("Analysiere {name} …" | "Analyzing {name} …")));
            ctx.request_repaint();
            let text = crate::platform::cmd("pdftotext")
                .args(["-l", "2", "-q"])
                .arg(&f)
                .arg("-")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            let doi_re = regex::Regex::new(r"(?i)\b(10\.\d{4,9}/[-._;()/:A-Za-z0-9]+[A-Za-z0-9])").unwrap();
            let arxiv_re = regex::Regex::new(r"arXiv:(\d{4}\.\d{4,5})").unwrap();
            let mut entry = None;
            if let Some(c) = doi_re.captures(&text) {
                entry = fetch_doi(&c[1]).ok();
            }
            if entry.is_none() {
                if let Some(c) = arxiv_re.captures(&text) {
                    entry = fetch_doi(&format!("10.48550/arXiv.{}", &c[1])).ok().map(|mut e| {
                        e.set("eprint", &c[1]);
                        e.set("archiveprefix", "arXiv");
                        e
                    });
                }
            }
            let entry = entry.unwrap_or_else(|| fallback_entry(&f, &text));
            let _ = tx.send(ShelfMsg::Fetched { entry, pdf: Some(f) });
            ctx.request_repaint();
        }
    });
}

fn fallback_entry(f: &Path, text: &str) -> BibEntry {
    let info = crate::platform::cmd("pdfinfo").arg(f).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let field = |k: &str| {
        info.lines()
            .find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()))
            .filter(|v| !v.is_empty())
    };
    let mut title = field("Title:").unwrap_or_default();
    if title.len() < 4 {
        // first reasonably long line of the first page
        title = text.lines().map(str::trim).find(|l| l.len() > 12 && l.len() < 200).unwrap_or("").to_string();
    }
    if title.is_empty() {
        title = f.file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_default();
    }
    let mut e = BibEntry { kind: "misc".into(), key: String::new(), fields: vec![] };
    e.set("title", &title);
    if let Some(a) = field("Author:") {
        e.set("author", &a.replace([';', ','], " and "));
    }
    let year = field("CreationDate:").and_then(|d| d.split_whitespace().last().map(String::from)).unwrap_or_default();
    if year.len() == 4 {
        e.set("year", &year);
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs network"]
    fn fetch_arxiv_and_doi() {
        let e = fetch_doi("10.48550/arXiv.1706.03762").unwrap();
        assert_eq!(e.suggest_key(), "vaswani2017attention");
        let e = fetch_doi("10.1038/nature14539").unwrap();
        assert_eq!(e.suggest_key(), "lecun2015deep");
        assert!(e.get("keywords").is_none());
        let hits = search_crossref("Deep Residual Learning for Image Recognition").unwrap();
        assert!(!hits.is_empty());
    }

    #[test]
    fn classify_inputs() {
        assert!(matches!(classify("https://doi.org/10.1038/nature14539"), Query::Doi(d) if d == "10.1038/nature14539"));
        assert!(matches!(classify("arXiv:1706.03762v5"), Query::Arxiv(a) if a == "1706.03762"));
        assert!(matches!(classify("attention is all you need"), Query::Search(_)));
    }
}
