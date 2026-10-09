//! PDF preview: page metadata via `pdfinfo`, lazy page rendering via `pdftoppm`
//! on a small worker pool, and a continuous / single-page viewer widget.

use crate::theme::{with_alpha, Palette};
use egui::{pos2, vec2, Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct PdfDoc {
    pub path: PathBuf,
    pub stamp: u64,
    pub pages: Vec<[f32; 2]>, // width, height in pt
}

pub fn file_stamp(p: &Path) -> u64 {
    std::fs::metadata(p)
        .map(|m| {
            let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(0);
            t ^ m.len().rotate_left(17)
        })
        .unwrap_or(0)
}

impl PdfDoc {
    pub fn open(path: &Path) -> Option<Self> {
        let out = crate::platform::cmd("pdfinfo").args(["-f", "1", "-l", "100000"]).arg(path).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let re = regex::Regex::new(r"^Page\s+(\d+) size:\s+([\d.]+) x ([\d.]+)").unwrap();
        let mut pages = vec![];
        for l in s.lines() {
            if let Some(c) = re.captures(l) {
                pages.push([c[2].parse().unwrap_or(595.0), c[3].parse().unwrap_or(842.0)]);
            }
        }
        if pages.is_empty() {
            // fallback: single "Page size" line + page count
            let n = s.lines().find_map(|l| l.strip_prefix("Pages:").and_then(|v| v.trim().parse::<usize>().ok()))?;
            pages = vec![[595.0, 842.0]; n];
        }
        Some(PdfDoc { path: path.to_path_buf(), stamp: file_stamp(path), pages })
    }
}

// ───────────────────────────── renderer ─────────────────────────────

struct RenderReq {
    path: PathBuf,
    stamp: u64,
    page: usize,
    dpi: u32,
    reply: Sender<RenderOut>,
    ctx: egui::Context,
}

pub struct RenderOut {
    pub stamp: u64,
    pub page: usize,
    pub dpi: u32,
    pub image: Option<ColorImage>,
}

#[derive(Clone)]
pub struct Renderer {
    tx: Sender<RenderReq>,
}

impl Renderer {
    pub fn new(workers: usize) -> Self {
        let (tx, rx) = channel::<RenderReq>();
        let rx = Arc::new(Mutex::new(rx));
        let tmp = std::env::temp_dir().join(format!("nedit-render-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        for w in 0..workers {
            let rx = rx.clone();
            let tmp = tmp.clone();
            std::thread::spawn(move || loop {
                let req = match rx.lock().unwrap().recv() {
                    Ok(r) => r,
                    Err(_) => return,
                };
                let prefix = tmp.join(format!("w{w}"));
                let ok = crate::platform::cmd("pdftoppm")
                    .args(["-png", "-singlefile", "-aa", "yes", "-aaVector", "yes"])
                    .args(["-r", &req.dpi.to_string()])
                    .args(["-f", &(req.page + 1).to_string(), "-l", &(req.page + 1).to_string()])
                    .arg(&req.path)
                    .arg(&prefix)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                let png = prefix.with_extension("png");
                let image = if ok {
                    image::open(&png).ok().map(|img| {
                        let rgba = img.to_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
                    })
                } else {
                    None
                };
                let _ = std::fs::remove_file(&png);
                let _ = req.reply.send(RenderOut { stamp: req.stamp, page: req.page, dpi: req.dpi, image });
                req.ctx.request_repaint();
            });
        }
        Renderer { tx }
    }
}

struct PageTex {
    tex: TextureHandle,
    dpi: u32,
    stamp: u64,
}

/// Texture cache for the pages of one document at one target resolution.
pub struct PageCache {
    textures: HashMap<usize, PageTex>,
    pending: HashSet<(usize, u32, u64)>,
    tx: Sender<RenderOut>,
    rx: Receiver<RenderOut>,
    name: String,
}

impl PageCache {
    pub fn new(name: &str) -> Self {
        let (tx, rx) = channel();
        PageCache { textures: HashMap::new(), pending: HashSet::new(), tx, rx, name: name.into() }
    }

    pub fn clear(&mut self) {
        self.textures.clear();
        self.pending.clear();
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(out) = self.rx.try_recv() {
            self.pending.remove(&(out.page, out.dpi, out.stamp));
            if let Some(img) = out.image {
                let replace = match self.textures.get(&out.page) {
                    None => true,
                    Some(old) => out.stamp != old.stamp || out.dpi != old.dpi,
                };
                if replace {
                    let tex = ctx.load_texture(format!("{}-{}", self.name, out.page), img, TextureOptions::LINEAR);
                    self.textures.insert(out.page, PageTex { tex, dpi: out.dpi, stamp: out.stamp });
                }
            }
        }
    }

    /// Returns the best available texture (possibly stale) and requests a fresh one if needed.
    pub fn get(&mut self, doc: &PdfDoc, page: usize, dpi: u32, renderer: &Renderer, ctx: &egui::Context) -> Option<egui::TextureId> {
        let fresh = self.textures.get(&page).is_some_and(|t| t.stamp == doc.stamp && t.dpi == dpi);
        if !fresh && !self.pending.contains(&(page, dpi, doc.stamp)) {
            // limit outstanding requests per cache
            if self.pending.len() < 6 {
                self.pending.insert((page, dpi, doc.stamp));
                let _ = renderer.tx.send(RenderReq { path: doc.path.clone(), stamp: doc.stamp, page, dpi, reply: self.tx.clone(), ctx: ctx.clone() });
            }
        }
        self.textures.get(&page).map(|t| t.tex.id())
    }
}

// ───────────────────────────── viewer widget ─────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Zoom {
    FitWidth,
    FitPage,
    Scale(f32),
}

pub struct PdfViewer {
    pub doc: Option<PdfDoc>,
    pub cache: PageCache,
    pub zoom: Zoom,
    pub single_page: bool,
    pub current_page: usize,
    scroll_to: Option<(usize, f32)>, // page, y in pt (top)
    highlight: Option<(usize, Rect, f64)>,
    last_scale: f32,
    page_offsets: Vec<f32>,
    pub dark_pages: bool,
}

pub struct ViewerResponse {
    pub double_click: Option<(usize, f32, f32)>,
}

const GAP: f32 = 18.0;
const PAD: f32 = 22.0;

impl PdfViewer {
    pub fn new(name: &str) -> Self {
        PdfViewer {
            doc: None,
            cache: PageCache::new(name),
            zoom: Zoom::FitWidth,
            single_page: false,
            current_page: 0,
            scroll_to: None,
            highlight: None,
            last_scale: 1.0,
            page_offsets: vec![],
            dark_pages: false,
        }
    }

    pub fn load(&mut self, path: &Path) {
        if let Some(doc) = PdfDoc::open(path) {
            let same_file = self.doc.as_ref().is_some_and(|d| d.path == doc.path);
            if !same_file {
                self.cache.clear();
                self.current_page = 0;
            }
            self.current_page = self.current_page.min(doc.pages.len().saturating_sub(1));
            self.doc = Some(doc);
        }
    }

    pub fn reload_if_changed(&mut self) -> bool {
        let Some(d) = &self.doc else { return false };
        let st = file_stamp(&d.path);
        if st != d.stamp && st != 0 {
            let p = d.path.clone();
            self.load(&p);
            return true;
        }
        false
    }

    pub fn page_count(&self) -> usize {
        self.doc.as_ref().map_or(0, |d| d.pages.len())
    }

    pub fn go_to(&mut self, page: usize, y_pt: f32) {
        let n = self.page_count();
        if n == 0 {
            return;
        }
        self.current_page = page.min(n - 1);
        self.scroll_to = Some((self.current_page, y_pt));
    }

    pub fn flash(&mut self, page: usize, rect_pt: Rect, now: f64) {
        self.highlight = Some((page, rect_pt, now));
    }

    pub fn next_page(&mut self) {
        let p = (self.current_page + 1).min(self.page_count().saturating_sub(1));
        self.go_to(p, 0.0);
    }
    pub fn prev_page(&mut self) {
        let p = self.current_page.saturating_sub(1);
        self.go_to(p, 0.0);
    }

    pub fn scale(&self) -> f32 {
        self.last_scale
    }

    pub fn zoom_by(&mut self, factor: f32) {
        let s = (self.last_scale * factor).clamp(0.2, 6.0);
        self.zoom = Zoom::Scale(s);
    }

    pub fn dpi_for(scale: f32, ppp: f32) -> u32 {
        // quantize to avoid re-render storms while zooming
        let raw = 72.0 * scale * ppp;
        ((raw / 16.0).ceil() * 16.0).clamp(32.0, 600.0) as u32
    }

    pub fn show(&mut self, ui: &mut egui::Ui, renderer: &Renderer, pal: &Palette) -> ViewerResponse {
        let ctx = ui.ctx().clone();
        self.cache.poll(&ctx);
        let mut resp = ViewerResponse { double_click: None };
        let Some(doc) = self.doc.clone() else {
            let r = ui.available_rect_before_wrap();
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, tr!("Noch kein PDF – Strg+Enter zum Kompilieren" | "No PDF yet – Ctrl+Enter to compile"), egui::FontId::proportional(14.0), pal.dim);
            ui.allocate_rect(r, Sense::hover());
            return resp;
        };
        if self.single_page {
            return self.show_single(ui, renderer, pal, &doc);
        }
        let avail = ui.available_size();
        let max_w = doc.pages.iter().map(|p| p[0]).fold(1.0, f32::max);
        let max_h = doc.pages.iter().map(|p| p[1]).fold(1.0, f32::max);
        let scale = match self.zoom {
            Zoom::FitWidth => ((avail.x - 2.0 * PAD - 12.0) / max_w).max(0.1),
            Zoom::FitPage => ((avail.y - 2.0 * PAD) / max_h).min((avail.x - 2.0 * PAD) / max_w).max(0.1),
            Zoom::Scale(s) => s,
        };
        self.last_scale = scale;
        let ppp = ctx.pixels_per_point();
        let dpi = Self::dpi_for(scale, ppp);

        // layout
        self.page_offsets.clear();
        let mut y = PAD;
        for p in &doc.pages {
            self.page_offsets.push(y);
            y += p[1] * scale + GAP;
        }
        let total_h = y - GAP + PAD;
        let total_w = (max_w * scale + 2.0 * PAD).max(avail.x);

        let mut area = egui::ScrollArea::both().auto_shrink([false, false]).id_salt(("pdf", &self.cache.name));
        if let Some((page, ypt)) = self.scroll_to.take() {
            if let Some(off) = self.page_offsets.get(page) {
                let target = (off + ypt * scale - 60.0).max(0.0);
                area = area.vertical_scroll_offset(target);
            }
        }
        let hl = self.highlight;
        let now = ctx.input(|i| i.time);
        area.show_viewport(ui, |ui, viewport| {
            let (rect, response) = ui.allocate_exact_size(vec2(total_w, total_h), Sense::click());
            let painter = ui.painter_at(ui.clip_rect());
            let vis = viewport.translate(rect.min.to_vec2());
            let center_y = viewport.center().y;
            let mut best = (f32::MAX, 0usize);
            for (i, p) in doc.pages.iter().enumerate() {
                let size = vec2(p[0] * scale, p[1] * scale);
                let x = rect.min.x + (total_w - size.x) / 2.0;
                let pr = Rect::from_min_size(pos2(x, rect.min.y + self.page_offsets[i]), size);
                let d = (self.page_offsets[i] + size.y / 2.0 - center_y).abs();
                if self.page_offsets[i] <= center_y && self.page_offsets[i] + size.y + GAP >= center_y {
                    best = (0.0, i);
                } else if d < best.0 {
                    best = (d, i);
                }
                if !pr.expand(size.y * 0.6).intersects(vis) {
                    continue;
                }
                // shadow
                for k in 1..=4 {
                    let k = k as f32;
                    painter.rect_filled(pr.translate(vec2(0.0, k * 1.2)).expand(k * 1.5), 3.0 + k, with_alpha(Color32::BLACK, (22.0 / k) as u8));
                }
                painter.rect_filled(pr, 2.0, Color32::WHITE);
                if let Some(tex) = self.cache.get(&doc, i, dpi, renderer, &ctx) {
                    let tint = if self.dark_pages { Color32::from_gray(225) } else { Color32::WHITE };
                    painter.image(tex, pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
                }
                if let Some((hp, hr, t0)) = hl {
                    if hp == i {
                        let age = (now - t0) as f32;
                        if age < 2.2 {
                            let a = ((1.0 - age / 2.2) * 110.0) as u8;
                            let r = Rect::from_min_size(pr.min + vec2(hr.min.x * scale, hr.min.y * scale), vec2((hr.width() * scale).max(pr.width() * 0.75), hr.height() * scale)).expand(3.0);
                            let r = Rect::from_min_max(pos2(pr.min.x + 12.0, r.min.y), pos2(pr.max.x - 12.0, r.max.y));
                            painter.rect_filled(r, 4.0, with_alpha(pal.accent, a / 2));
                            painter.rect_stroke(r, 4.0, egui::Stroke::new(1.5, with_alpha(pal.accent, a)), egui::StrokeKind::Outside);
                            ui.ctx().request_repaint();
                        }
                    }
                }
            }
            self.current_page = best.1;
            if response.double_clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    for (i, p) in doc.pages.iter().enumerate() {
                        let size = vec2(p[0] * scale, p[1] * scale);
                        let x = rect.min.x + (total_w - size.x) / 2.0;
                        let pr = Rect::from_min_size(pos2(x, rect.min.y + self.page_offsets[i]), size);
                        if pr.contains(pos) {
                            let local = (pos - pr.min) / scale;
                            resp.double_click = Some((i, local.x, local.y));
                        }
                    }
                }
            }
            if response.hovered() {
                let zd = ui.input(|i| i.zoom_delta());
                if (zd - 1.0).abs() > 0.001 {
                    self.zoom = Zoom::Scale((scale * zd).clamp(0.2, 6.0));
                }
            }
        });
        resp
    }

    fn show_single(&mut self, ui: &mut egui::Ui, renderer: &Renderer, _pal: &Palette, doc: &PdfDoc) -> ViewerResponse {
        let ctx = ui.ctx().clone();
        let mut resp = ViewerResponse { double_click: None };
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click());
        let n = doc.pages.len();
        if n == 0 {
            return resp;
        }
        self.current_page = self.current_page.min(n - 1);
        let p = doc.pages[self.current_page];
        let pad = 18.0;
        let scale = ((rect.width() - 2.0 * pad) / p[0]).min((rect.height() - 2.0 * pad) / p[1]).max(0.05);
        let size = vec2(p[0] * scale, p[1] * scale);
        let pr = Rect::from_center_size(rect.center(), size);
        let painter = ui.painter_at(rect);
        for k in 1..=5 {
            let k = k as f32;
            painter.rect_filled(pr.translate(vec2(0.0, k * 2.0)).expand(k * 2.0), 4.0 + k, with_alpha(Color32::BLACK, (26.0 / k) as u8));
        }
        painter.rect_filled(pr, 3.0, Color32::WHITE);
        let dpi = Self::dpi_for(scale, ctx.pixels_per_point());
        if let Some(tex) = self.cache.get(doc, self.current_page, dpi, renderer, &ctx) {
            painter.image(tex, pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        // prefetch neighbours
        if self.current_page + 1 < n {
            self.cache.get(doc, self.current_page + 1, dpi, renderer, &ctx);
        }
        self.last_scale = scale;
        if response.double_clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                if pr.contains(pos) {
                    let local = (pos - pr.min) / scale;
                    resp.double_click = Some((self.current_page, local.x, local.y));
                }
            }
        }
        resp
    }
}

/// Draws one page as a thumbnail into `rect` (fit), returns the response.
pub fn thumbnail(ui: &mut egui::Ui, cache: &mut PageCache, doc: &PdfDoc, page: usize, rect: Rect, renderer: &Renderer) {
    let ctx = ui.ctx().clone();
    let p = doc.pages[page];
    let scale = (rect.width() / p[0]).min(rect.height() / p[1]);
    let size = vec2(p[0] * scale, p[1] * scale);
    let pr = Rect::from_center_size(rect.center(), size);
    let painter = ui.painter();
    painter.rect_filled(pr, 3.0, Color32::WHITE);
    let dpi = PdfViewer::dpi_for(scale, ctx.pixels_per_point());
    if let Some(tex) = cache.get(doc, page, dpi, renderer, &ctx) {
        painter.image(tex, pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}
