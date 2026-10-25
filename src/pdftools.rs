//! Reading PDFs: page sizes, rendering a page, text and metadata.
//!
//! On Linux and Windows this uses Poppler (`pdfinfo`, `pdftoppm`, `pdftotext`). On macOS the
//! system does it itself – CoreGraphics renders, PDFKit extracts text – so nothing has to be
//! installed there (Poppler is only used as a fallback if it happens to be on PATH).

use std::path::Path;

/// What the bibliography import needs from a PDF's metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub title: String,
    pub author: String,
    pub year: String,
}

/// Can PDFs be shown on this system?
pub fn available() -> bool {
    cfg!(target_os = "macos") || (crate::platform::has_tool("pdftoppm") && crate::platform::has_tool("pdfinfo"))
}

/// Size of every page in pt as displayed (width/height swapped for pages rotated by 90°).
pub fn page_sizes(path: &Path) -> Option<Vec<[f32; 2]>> {
    #[cfg(target_os = "macos")]
    if let Some(s) = mac::page_sizes(path) {
        return Some(s);
    }
    poppler::page_sizes(path)
}

/// Render page `page` (0-based) at `dpi`, upright, on white.
pub fn render(path: &Path, page: usize, dpi: u32) -> Option<image::RgbaImage> {
    #[cfg(target_os = "macos")]
    if let Some(i) = mac::render(path, page, dpi) {
        return Some(i);
    }
    poppler::render(path, page, dpi)
}

/// Plain text of the first `pages` pages.
pub fn text(path: &Path, pages: usize) -> String {
    #[cfg(target_os = "macos")]
    if let Some(t) = mac::text(path, pages) {
        return t;
    }
    poppler::text(path, pages).unwrap_or_default()
}

/// Title, author and year from the document information.
pub fn info(path: &Path) -> Info {
    #[cfg(target_os = "macos")]
    if let Some(i) = mac::info(path) {
        return i;
    }
    poppler::info(path).unwrap_or_default()
}

mod poppler {
    use super::Info;
    use std::path::Path;

    pub fn page_sizes(path: &Path) -> Option<Vec<[f32; 2]>> {
        let out = crate::platform::cmd("pdfinfo").args(["-f", "1", "-l", "100000"]).arg(path).output().ok()?;
        parse_pdfinfo(&String::from_utf8_lossy(&out.stdout))
    }

    /// `Page 3 size: 595 x 842 pts` and `Page 3 rot: 90` lines (or only the totals).
    pub fn parse_pdfinfo(s: &str) -> Option<Vec<[f32; 2]>> {
        let size = regex::Regex::new(r"^Page\s+(\d+) size:\s+([\d.]+) x ([\d.]+)").unwrap();
        let rot = regex::Regex::new(r"^Page\s+(\d+) rot:\s+(\d+)").unwrap();
        let mut pages: Vec<[f32; 2]> = vec![];
        for l in s.lines() {
            if let Some(c) = size.captures(l) {
                pages.push([c[2].parse().unwrap_or(595.0), c[3].parse().unwrap_or(842.0)]);
            } else if let Some(c) = rot.captures(l) {
                let i = c[1].parse::<usize>().unwrap_or(0);
                if c[2].parse::<u32>().unwrap_or(0) % 180 == 90 {
                    if let Some(p) = i.checked_sub(1).and_then(|i| pages.get_mut(i)) {
                        p.swap(0, 1);
                    }
                }
            }
        }
        if pages.is_empty() {
            let n = s.lines().find_map(|l| l.strip_prefix("Pages:").and_then(|v| v.trim().parse::<usize>().ok()))?;
            pages = vec![[595.0, 842.0]; n];
        }
        Some(pages)
    }

    pub fn render(path: &Path, page: usize, dpi: u32) -> Option<image::RgbaImage> {
        // a unique temp prefix per call: several render workers run at the same time
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let prefix = std::env::temp_dir().join(format!("nedit-pdf-{}-{n}", std::process::id()));
        let p = (page + 1).to_string();
        let ok = crate::platform::cmd("pdftoppm")
            .args(["-png", "-singlefile", "-aa", "yes", "-aaVector", "yes", "-r", &dpi.to_string(), "-f", &p, "-l", &p])
            .arg(path)
            .arg(&prefix)
            .status()
            .is_ok_and(|s| s.success());
        let png = prefix.with_extension("png");
        let img = if ok { image::open(&png).ok().map(|i| i.to_rgba8()) } else { None };
        let _ = std::fs::remove_file(&png);
        img
    }

    pub fn text(path: &Path, pages: usize) -> Option<String> {
        let out = crate::platform::cmd("pdftotext").args(["-l", &pages.to_string(), "-q"]).arg(path).arg("-").output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    }

    pub fn info(path: &Path) -> Option<Info> {
        let out = crate::platform::cmd("pdfinfo").arg("-isodates").arg(path).output().ok()?;
        Some(parse_info(&String::from_utf8_lossy(&out.stdout)))
    }

    pub fn parse_info(s: &str) -> Info {
        let field = |k: &str| s.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string())).unwrap_or_default();
        let date = field("CreationDate:");
        Info { title: field("Title:"), author: field("Author:"), year: date.get(..4).filter(|y| y.chars().all(|c| c.is_ascii_digit())).unwrap_or("").to_string() }
    }
}

/// macOS: CoreGraphics (plain C API) for pages and metadata, PDFKit for text.
#[cfg(target_os = "macos")]
mod mac {
    use super::Info;
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    type CFTypeRef = *const c_void;
    type CGPDFDocumentRef = *const c_void;
    type CGPDFPageRef = *const c_void;
    type CGPDFDictionaryRef = *const c_void;
    type CGPDFStringRef = *const c_void;
    type CGContextRef = *mut c_void;
    type CGColorSpaceRef = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    const K_CGPDF_CROP_BOX: c_int = 1;
    /// kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big → bytes R G B A
    const RGBA: u32 = 1 | (4 << 12);
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(alloc: CFTypeRef, buf: *const u8, len: isize, is_dir: u8) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFStringGetLength(s: CFTypeRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(len: isize, enc: u32) -> isize;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, enc: u32) -> u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPDFDocumentCreateWithURL(url: CFTypeRef) -> CGPDFDocumentRef;
        fn CGPDFDocumentRelease(doc: CGPDFDocumentRef);
        fn CGPDFDocumentGetNumberOfPages(doc: CGPDFDocumentRef) -> usize;
        fn CGPDFDocumentGetPage(doc: CGPDFDocumentRef, n: usize) -> CGPDFPageRef;
        fn CGPDFDocumentGetInfo(doc: CGPDFDocumentRef) -> CGPDFDictionaryRef;
        fn CGPDFDictionaryGetString(d: CGPDFDictionaryRef, key: *const c_char, value: *mut CGPDFStringRef) -> bool;
        fn CGPDFStringCopyTextString(s: CGPDFStringRef) -> CFTypeRef;
        fn CGPDFPageGetBoxRect(page: CGPDFPageRef, b: c_int) -> CGRect;
        fn CGPDFPageGetRotationAngle(page: CGPDFPageRef) -> c_int;
        fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
        fn CGColorSpaceRelease(cs: CGColorSpaceRef);
        fn CGBitmapContextCreate(data: *mut c_void, w: usize, h: usize, bpc: usize, bpr: usize, cs: CGColorSpaceRef, info: u32) -> CGContextRef;
        fn CGContextRelease(c: CGContextRef);
        fn CGContextSetRGBFillColor(c: CGContextRef, r: f64, g: f64, b: f64, a: f64);
        fn CGContextFillRect(c: CGContextRef, r: CGRect);
        fn CGContextScaleCTM(c: CGContextRef, sx: f64, sy: f64);
        fn CGContextTranslateCTM(c: CGContextRef, tx: f64, ty: f64);
        fn CGContextRotateCTM(c: CGContextRef, angle: f64);
        fn CGContextSetInterpolationQuality(c: CGContextRef, q: c_int);
        fn CGContextDrawPDFPage(c: CGContextRef, page: CGPDFPageRef);
    }

    #[link(name = "PDFKit", kind = "framework")]
    unsafe extern "C" {}

    /// An open CGPDFDocument (released on drop).
    struct Doc(CGPDFDocumentRef);

    impl Doc {
        fn open(path: &Path) -> Option<Doc> {
            let bytes = path.as_os_str().as_bytes();
            unsafe {
                let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 0);
                if url.is_null() {
                    return None;
                }
                let doc = CGPDFDocumentCreateWithURL(url);
                CFRelease(url);
                (!doc.is_null()).then_some(Doc(doc))
            }
        }

        fn pages(&self) -> usize {
            unsafe { CGPDFDocumentGetNumberOfPages(self.0) }
        }

        /// 0-based page with its crop box and rotation (0, 90, 180, 270).
        fn page(&self, i: usize) -> Option<(CGPDFPageRef, CGRect, i32)> {
            unsafe {
                let p = CGPDFDocumentGetPage(self.0, i + 1);
                if p.is_null() {
                    return None;
                }
                Some((p, CGPDFPageGetBoxRect(p, K_CGPDF_CROP_BOX), CGPDFPageGetRotationAngle(p).rem_euclid(360)))
            }
        }
    }

    impl Drop for Doc {
        fn drop(&mut self) {
            unsafe { CGPDFDocumentRelease(self.0) }
        }
    }

    fn displayed(r: CGRect, rot: i32) -> [f64; 2] {
        if rot % 180 == 90 { [r.size.height, r.size.width] } else { [r.size.width, r.size.height] }
    }

    pub fn page_sizes(path: &Path) -> Option<Vec<[f32; 2]>> {
        let doc = Doc::open(path)?;
        let n = doc.pages();
        if n == 0 {
            return None;
        }
        Some((0..n).map(|i| doc.page(i).map(|(_, r, rot)| displayed(r, rot).map(|v| v as f32)).unwrap_or([595.0, 842.0])).collect())
    }

    pub fn render(path: &Path, page: usize, dpi: u32) -> Option<image::RgbaImage> {
        let doc = Doc::open(path)?;
        let (p, b, rot) = doc.page(page)?;
        let s = dpi as f64 / 72.0;
        let [dw, dh] = displayed(b, rot);
        let (w, h) = ((dw * s).round().max(1.0) as usize, (dh * s).round().max(1.0) as usize);
        if w * h > 16_000 * 16_000 {
            return None;
        }
        let mut buf = vec![0u8; w * h * 4];
        unsafe {
            let cs = CGColorSpaceCreateDeviceRGB();
            let c = CGBitmapContextCreate(buf.as_mut_ptr().cast(), w, h, 8, w * 4, cs, RGBA);
            CGColorSpaceRelease(cs);
            if c.is_null() {
                return None;
            }
            CGContextSetRGBFillColor(c, 1.0, 1.0, 1.0, 1.0);
            CGContextFillRect(c, CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w as f64, height: h as f64 } });
            CGContextSetInterpolationQuality(c, 3); // kCGInterpolationHigh, for embedded images
            CGContextScaleCTM(c, s, s);
            // map the crop box onto the displayed page, turning it like a viewer does
            // (points are transformed by the last call first)
            let (bw, bh) = (b.size.width, b.size.height);
            match rot {
                90 => {
                    CGContextTranslateCTM(c, 0.0, bw);
                    CGContextRotateCTM(c, -std::f64::consts::FRAC_PI_2);
                }
                180 => {
                    CGContextTranslateCTM(c, bw, bh);
                    CGContextRotateCTM(c, std::f64::consts::PI);
                }
                270 => {
                    CGContextTranslateCTM(c, bh, 0.0);
                    CGContextRotateCTM(c, std::f64::consts::FRAC_PI_2);
                }
                _ => {}
            }
            CGContextTranslateCTM(c, -b.origin.x, -b.origin.y);
            CGContextDrawPDFPage(c, p);
            CGContextRelease(c);
        }
        // the bitmap's first row is the top of the page; alpha is 255 everywhere (white background)
        image::RgbaImage::from_raw(w as u32, h as u32, buf)
    }

    fn cfstring(s: CFTypeRef) -> String {
        if s.is_null() {
            return String::new();
        }
        unsafe {
            let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s), K_CF_STRING_ENCODING_UTF8) + 1;
            let mut buf = vec![0 as c_char; size.max(1) as usize];
            let ok = CFStringGetCString(s, buf.as_mut_ptr(), size, K_CF_STRING_ENCODING_UTF8) != 0;
            CFRelease(s);
            if ok { CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned() } else { String::new() }
        }
    }

    pub fn info(path: &Path) -> Option<Info> {
        let doc = Doc::open(path)?;
        let get = |key: &CStr| unsafe {
            let d = CGPDFDocumentGetInfo(doc.0);
            let mut s: CGPDFStringRef = std::ptr::null();
            if d.is_null() || !CGPDFDictionaryGetString(d, key.as_ptr(), &mut s) || s.is_null() {
                return String::new();
            }
            cfstring(CGPDFStringCopyTextString(s)).trim().to_string()
        };
        // "D:20210304120000+01'00'"
        let date = get(c"CreationDate");
        let digits: String = date.trim_start_matches("D:").chars().take(4).collect();
        Some(Info { title: get(c"Title"), author: get(c"Author"), year: if digits.len() == 4 && digits.chars().all(|c| c.is_ascii_digit()) { digits } else { String::new() } })
    }

    /// Text of the first pages via PDFKit (`PDFDocument` / `PDFPage.string`).
    pub fn text(path: &Path, pages: usize) -> Option<String> {
        use objc2::runtime::{AnyClass, AnyObject};
        use objc2::msg_send;
        let bytes = path.as_os_str().as_bytes();
        objc2::rc::autoreleasepool(|_| unsafe {
            let cls = AnyClass::get(c"PDFDocument")?;
            // CFURL is toll-free bridged to NSURL
            let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 0);
            if url.is_null() {
                return None;
            }
            let alloc: *mut AnyObject = msg_send![cls, alloc];
            let doc: *mut AnyObject = msg_send![alloc, initWithURL: url as *mut AnyObject];
            CFRelease(url);
            if doc.is_null() {
                return None;
            }
            let count: usize = msg_send![doc, pageCount];
            let mut out = String::new();
            for i in 0..count.min(pages) {
                let page: *mut AnyObject = msg_send![doc, pageAtIndex: i];
                if page.is_null() {
                    continue;
                }
                let s: *mut AnyObject = msg_send![page, string];
                if s.is_null() {
                    continue;
                }
                let utf8: *const c_char = msg_send![s, UTF8String];
                if !utf8.is_null() {
                    out.push_str(&CStr::from_ptr(utf8).to_string_lossy());
                    out.push('\n');
                }
            }
            let _: () = msg_send![doc, release];
            Some(out)
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A small, valid PDF built by hand: page `w`×`h` pt, optionally rotated, with a black bar
    /// over the top quarter, a line of Helvetica text and document info.
    pub fn sample_pdf(w: u32, h: u32, rotate: u32, text: &str) -> Vec<u8> {
        let content = format!("0 0 0 rg 0 {} {w} {} re f\nBT /F1 12 Tf 10 {} Td ({text}) Tj ET\n", h * 3 / 4, h / 4, h / 2);
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Rotate {rotate} /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"),
            format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            "<< /Title (Testtitel) /Author (Ada Lovelace) /CreationDate (D:20210304120000Z) >>".to_string(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offs = vec![];
        for (i, o) in objs.iter().enumerate() {
            offs.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offs {
            pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        pdf
    }

    fn write(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("nedit-pdftools-{name}-{}.pdf", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn dark(img: &image::RgbaImage, x: u32, y: u32) -> bool {
        img.get_pixel(x, y).0[0] < 100
    }

    #[test]
    fn pdfinfo_parsing() {
        let s = "Pages:          2\nPage    1 size: 595.276 x 841.89 pts (A4)\nPage    1 rot:  0\nPage    2 size: 200 x 100 pts\nPage    2 rot:  90\n";
        assert_eq!(poppler::parse_pdfinfo(s).unwrap(), vec![[595.276, 841.89], [100.0, 200.0]]);
        let i = poppler::parse_info("Title:          Ein Titel\nAuthor:         Ada\nCreationDate:   2021-03-04T12:00:00Z\n");
        assert_eq!(i, Info { title: "Ein Titel".into(), author: "Ada".into(), year: "2021".into() });
    }

    /// The same expectations for Poppler (Linux, Windows) and CoreGraphics/PDFKit (macOS).
    #[test]
    fn sizes_rendering_text_and_info() {
        if !available() {
            eprintln!("no PDF backend – skipped");
            return;
        }
        let up = write("up", &sample_pdf(200, 100, 0, "Hello nEdit 10.1234/abcd"));
        assert_eq!(page_sizes(&up).unwrap(), vec![[200.0, 100.0]]);
        let img = render(&up, 0, 72).unwrap();
        assert_eq!((img.width(), img.height()), (200, 100));
        assert!(dark(&img, 100, 5), "the bar is at the top");
        assert!(!dark(&img, 100, 95), "the bottom stays white");
        let big = render(&up, 0, 144).unwrap();
        assert_eq!((big.width(), big.height()), (400, 200));
        let t = text(&up, 2);
        assert!(t.contains("Hello nEdit") && t.contains("10.1234/abcd"), "{t:?}");
        assert_eq!(info(&up), Info { title: "Testtitel".into(), author: "Ada Lovelace".into(), year: "2021".into() });

        // a page rotated by 90°: shown upright as 100×200, the top bar ends up on the right
        let rot = write("rot", &sample_pdf(200, 100, 90, "x"));
        assert_eq!(page_sizes(&rot).unwrap(), vec![[100.0, 200.0]]);
        let img = render(&rot, 0, 72).unwrap();
        assert_eq!((img.width(), img.height()), (100, 200));
        assert!(dark(&img, 95, 100), "bar on the right");
        assert!(!dark(&img, 5, 100), "left stays white");
        let img = render(&write("r180", &sample_pdf(200, 100, 180, "x")), 0, 72).unwrap();
        assert!(dark(&img, 100, 95) && !dark(&img, 100, 5), "180°: bar at the bottom");
        let img = render(&write("r270", &sample_pdf(200, 100, 270, "x")), 0, 72).unwrap();
        assert!(dark(&img, 5, 100) && !dark(&img, 95, 100), "270°: bar on the left");
        assert!(render(&up, 5, 72).is_none(), "no such page");
    }
}
