//! Font setup. Every family is looked up in this order:
//! system fonts (fontconfig on Linux) → fonts shipped with TeX Live/MiKTeX (found via
//! `kpsewhich`, so they exist wherever LaTeX is installed) → common OS fonts → egui defaults.
//! The icon font (Nerd Font symbols, MIT) is embedded, so icons work everywhere.

use egui::{FontData, FontDefinitions, FontFamily};
use std::path::Path;
use std::sync::Arc;

const ICONS: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFont-Regular.ttf");

/// macOS has none of the fonts nEdit uses on Linux, and the TeX it sets up (TinyTeX) brings
/// no Libertinus – so the Mac build carries them (all SIL Open Font License, see
/// assets/fonts/LICENSE-*): the same look as on Linux.
#[cfg(target_os = "macos")]
mod bundled {
    pub const UI: &[u8] = include_bytes!("../assets/fonts/AdwaitaSans-Regular.ttf");
    pub const UI_BOLD: &[u8] = include_bytes!("../assets/fonts/NotoSans-SemiBold.ttf");
    pub const MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
    pub const MONO_BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");
    pub const DISPLAY: &[u8] = include_bytes!("../assets/fonts/LibertinusSerif-Semibold.otf");
    pub const SERIF: &[u8] = include_bytes!("../assets/fonts/LibertinusSerif-Regular.otf");
    pub const SERIF_BOLD: &[u8] = include_bytes!("../assets/fonts/LibertinusSerif-Bold.otf");
    pub const SERIF_ITALIC: &[u8] = include_bytes!("../assets/fonts/LibertinusSerif-Italic.otf");
    pub const SERIF_BOLDITALIC: &[u8] = include_bytes!("../assets/fonts/LibertinusSerif-BoldItalic.otf");
}

/// A font that comes with nEdit on this system (macOS), by role.
#[allow(unused_variables)]
fn shipped(role: &str) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        let b = match role {
            "ui" => bundled::UI,
            "ui-bold" => bundled::UI_BOLD,
            "mono" => bundled::MONO,
            "mono-bold" => bundled::MONO_BOLD,
            "display" => bundled::DISPLAY,
            "serif" => bundled::SERIF,
            "serif-bold" => bundled::SERIF_BOLD,
            "serif-italic" => bundled::SERIF_ITALIC,
            "serif-bolditalic" => bundled::SERIF_BOLDITALIC,
            _ => return None,
        };
        return Some(b.to_vec());
    }
    #[allow(unreachable_code)]
    None
}

fn read(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn first_existing(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().map(Path::new).find(|p| p.exists()).and_then(read)
}

/// fontconfig lookup (Linux, sometimes macOS); `must_contain` guards against fallbacks.
fn fc_match(pattern: &str, must_contain: &str) -> Option<Vec<u8>> {
    let out = crate::platform::cmd("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).to_string();
    if path.is_empty() || (!must_contain.is_empty() && !path.to_lowercase().contains(&must_contain.to_lowercase())) {
        return None;
    }
    read(Path::new(&path))
}

/// A font file from the TeX distribution.
fn tex(file: &str) -> Option<Vec<u8>> {
    const TEXMF: &[&str] = &[
        "/usr/share/texmf-dist/fonts/opentype/",
        "/usr/share/texlive/texmf-dist/fonts/opentype/",
        "/usr/local/texlive/texmf-dist/fonts/opentype/",
    ];
    for root in TEXMF {
        for sub in ["public/libertinus-fonts/", "adobe/sourcesanspro/", "adobe/sourcecodepro/", "public/fira/"] {
            let p = format!("{root}{sub}{file}");
            if Path::new(&p).exists() {
                return read(Path::new(&p));
            }
        }
    }
    crate::platform::kpsewhich(file).and_then(|p| read(&p))
}

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let mut add = |name: &str, data: Option<Vec<u8>>| -> bool {
        match data {
            Some(d) => {
                defs.font_data.insert(name.to_string(), Arc::new(FontData::from_owned(d)));
                true
            }
            None => false,
        }
    };

    let ui = add(
        "ui",
        shipped("ui")
            .or_else(|| fc_match("Adwaita Sans", "adwaita"))
            .or_else(|| fc_match("Inter", "inter"))
            .or_else(|| first_existing(&["C:\\Windows\\Fonts\\segoeui.ttf", "/System/Library/Fonts/Supplemental/Arial.ttf"]))
            .or_else(|| tex("SourceSansPro-Regular.otf"))
            .or_else(|| fc_match("Noto Sans", "noto")),
    );
    let ui_bold = add(
        "ui-bold",
        shipped("ui-bold")
            .or_else(|| first_existing(&["/usr/share/fonts/noto/NotoSans-SemiBold.ttf", "/usr/share/fonts/noto/NotoSans-Bold.ttf"]))
            .or_else(|| fc_match("Inter:semibold", "inter"))
            .or_else(|| first_existing(&["C:\\Windows\\Fonts\\seguisb.ttf", "/System/Library/Fonts/Supplemental/Arial Bold.ttf"]))
            .or_else(|| tex("SourceSansPro-Semibold.otf"))
            .or_else(|| fc_match("Noto Sans:bold", "bold")),
    );
    let mono = add(
        "mono",
        shipped("mono")
            .or_else(|| fc_match("JetBrainsMono Nerd Font", "jetbrains"))
            .or_else(|| fc_match("JetBrains Mono", "jetbrains"))
            .or_else(|| fc_match("CaskaydiaMono Nerd Font", "caskaydia"))
            .or_else(|| first_existing(&["C:\\Windows\\Fonts\\consola.ttf"]))
            .or_else(|| tex("SourceCodePro-Regular.otf"))
            .or_else(|| fc_match("monospace", "")),
    );
    let mono_bold = add(
        "mono-bold",
        shipped("mono-bold")
            .or_else(|| fc_match("JetBrainsMono Nerd Font:bold", "bold"))
            .or_else(|| fc_match("JetBrains Mono:bold", "bold"))
            .or_else(|| first_existing(&["C:\\Windows\\Fonts\\consolab.ttf"]))
            .or_else(|| tex("SourceCodePro-Bold.otf"))
            .or_else(|| fc_match("monospace:bold", "bold")),
    );
    add("icons", Some(ICONS.to_vec()));
    let display = add("display", tex("LibertinusSerif-Semibold.otf").or_else(|| fc_match("Libertinus Serif:semibold", "libertinus")).or_else(|| shipped("display")));
    let serif_files = [
        ("serif", "LibertinusSerif-Regular.otf", "serif"),
        ("serif-bold", "LibertinusSerif-Bold.otf", "serif:bold"),
        ("serif-italic", "LibertinusSerif-Italic.otf", "serif:italic"),
        ("serif-bolditalic", "LibertinusSerif-BoldItalic.otf", "serif:bold:italic"),
    ];
    let serif_ok: Vec<(&str, bool)> = serif_files.iter().map(|(name, file, fc)| (*name, add(name, tex(file).or_else(|| shipped(name)).or_else(|| fc_match(fc, ""))))).collect();

    // base families: preferred font first, then icons, then egui's defaults
    let mut prepend = |family: FontFamily, names: &[(&str, bool)]| {
        let list = defs.families.entry(family).or_default();
        for (i, n) in names.iter().filter(|(_, ok)| *ok).map(|(n, _)| n.to_string()).enumerate() {
            list.insert(i, n);
        }
    };
    prepend(FontFamily::Proportional, &[("ui", ui), ("icons", true)]);
    prepend(FontFamily::Monospace, &[("mono", mono), ("icons", true)]);
    let prop_list = defs.families[&FontFamily::Proportional].clone();
    let mono_list = defs.families[&FontFamily::Monospace].clone();

    let mut named = |name: &str, first: Option<&str>, base: &[String]| {
        let mut v: Vec<String> = first.into_iter().map(String::from).collect();
        v.extend(base.iter().cloned());
        defs.families.insert(FontFamily::Name(name.into()), v);
    };
    named("ui-bold", ui_bold.then_some("ui-bold"), &prop_list);
    named("mono-bold", mono_bold.then_some("mono-bold"), &mono_list);
    named("display", display.then_some("display"), &prop_list);
    for (name, ok) in serif_ok {
        named(name, ok.then_some(name), &prop_list);
    }

    ctx.set_fonts(defs);
}
