//! Omarchy theme integration: reads `colors.toml` (or falls back to `alacritty.toml`)
//! of the active Omarchy theme and derives a full UI palette from it.

use egui::{Color32, CornerRadius, Stroke, Visuals};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct Palette {
    pub name: String,
    pub dark: bool,
    // surfaces
    pub base: Color32,    // editor background
    pub mantle: Color32,  // sidebars, top bar
    pub crust: Color32,   // deepest (stage / pdf backdrop)
    pub surface: Color32, // cards, raised widgets
    pub overlay: Color32, // hover
    pub border: Color32,
    pub selection: Color32,
    // text
    pub text: Color32,
    pub subtext: Color32,
    pub dim: Color32,
    pub bright: Color32,
    // accents
    pub accent: Color32,
    pub on_accent: Color32,
    pub red: Color32,
    pub yellow: Color32,
    pub orange: Color32,
    pub green: Color32,
    pub cyan: Color32,
    pub blue: Color32,
    pub magenta: Color32,
}

fn hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() < 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color32::from_rgb(r, g, b))
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn luminance(c: Color32) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

pub fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Raw colors as found in a theme file.
#[derive(Default, Debug)]
struct Raw {
    mode: Option<String>,
    background: Option<Color32>,
    dark_background: Option<Color32>,
    foreground: Option<Color32>,
    bright_foreground: Option<Color32>,
    accent: Option<Color32>,
    selection: Option<Color32>,
    red: Option<Color32>,
    yellow: Option<Color32>,
    orange: Option<Color32>,
    green: Option<Color32>,
    cyan: Option<Color32>,
    blue: Option<Color32>,
    magenta: Option<Color32>,
}

impl Palette {
    pub fn default_dark() -> Self {
        Self::from_raw(
            "nEdit Ink",
            Raw {
                mode: Some("dark".into()),
                background: hex("#16171d"),
                foreground: hex("#d4d6de"),
                accent: hex("#e2a65b"),
                red: hex("#e06c75"),
                yellow: hex("#e5c07b"),
                orange: hex("#e2a65b"),
                green: hex("#8fc1a0"),
                cyan: hex("#7fbfc4"),
                blue: hex("#82aaff"),
                magenta: hex("#c099e0"),
                ..Default::default()
            },
        )
    }

    fn from_raw(name: &str, r: Raw) -> Self {
        let bg = r.background.unwrap_or(Color32::from_rgb(22, 23, 29));
        let fg = r.foreground.unwrap_or(Color32::from_rgb(212, 214, 222));
        let dark = match r.mode.as_deref() {
            Some("light") => false,
            Some("dark") => true,
            _ => luminance(bg) < 0.4,
        };
        let black = Color32::BLACK;
        let accent = r.accent.or(r.blue).unwrap_or(Color32::from_rgb(226, 166, 91));
        let (mantle, crust) = if dark {
            (r.dark_background.filter(|c| *c != bg).unwrap_or(mix(bg, black, 0.22)), mix(bg, black, 0.42))
        } else {
            (mix(bg, fg, 0.035), mix(bg, fg, 0.09))
        };
        let surface = mix(bg, fg, if dark { 0.065 } else { 0.05 });
        let overlay = mix(bg, fg, if dark { 0.12 } else { 0.10 });
        let border = mix(bg, fg, if dark { 0.14 } else { 0.16 });
        let selection = r
            .selection
            .filter(|s| (luminance(*s) - luminance(bg)).abs() > 0.01)
            .unwrap_or(mix(bg, accent, 0.28));
        let on_accent = if luminance(accent) > 0.38 { mix(black, bg, 0.3) } else { Color32::WHITE };
        let blue = r.blue.unwrap_or(accent);
        Palette {
            name: name.to_string(),
            dark,
            base: bg,
            mantle,
            crust,
            surface,
            overlay,
            border,
            selection,
            text: fg,
            subtext: mix(fg, bg, 0.28),
            dim: mix(fg, bg, 0.52),
            bright: r.bright_foreground.unwrap_or(if dark { mix(fg, Color32::WHITE, 0.3) } else { mix(fg, black, 0.3) }),
            accent,
            on_accent,
            red: r.red.unwrap_or(Color32::from_rgb(224, 108, 117)),
            yellow: r.yellow.unwrap_or(Color32::from_rgb(229, 192, 123)),
            orange: r.orange.or(r.yellow).unwrap_or(Color32::from_rgb(226, 166, 91)),
            green: r.green.unwrap_or(Color32::from_rgb(143, 193, 160)),
            cyan: r.cyan.unwrap_or(Color32::from_rgb(127, 191, 196)),
            blue,
            magenta: r.magenta.unwrap_or(Color32::from_rgb(192, 153, 224)),
        }
    }

    pub fn load_dir(dir: &Path, name: &str) -> Option<Self> {
        let colors = dir.join("colors.toml");
        if let Ok(src) = std::fs::read_to_string(&colors) {
            if let Ok(tbl) = src.parse::<toml::Table>() {
                let g = |k: &str| tbl.get(k).and_then(|v| v.as_str()).and_then(hex);
                let raw = Raw {
                    mode: tbl.get("mode").and_then(|v| v.as_str()).map(String::from),
                    background: g("background"),
                    dark_background: g("dark_background"),
                    foreground: g("foreground"),
                    bright_foreground: g("bright_foreground"),
                    accent: g("accent"),
                    selection: g("selection"),
                    red: g("red"),
                    yellow: g("yellow"),
                    orange: g("orange"),
                    green: g("green"),
                    cyan: g("cyan"),
                    blue: g("blue"),
                    magenta: g("magenta"),
                };
                return Some(Self::from_raw(&pretty_name(name), raw));
            }
        }
        // Fallback: alacritty.toml palette
        let src = std::fs::read_to_string(dir.join("alacritty.toml")).ok()?;
        let tbl = src.parse::<toml::Table>().ok()?;
        let colors = tbl.get("colors")?.as_table()?;
        let sect = |s: &str, k: &str| {
            colors.get(s).and_then(|t| t.as_table()).and_then(|t| t.get(k)).and_then(|v| v.as_str()).and_then(hex)
        };
        let raw = Raw {
            mode: None,
            background: sect("primary", "background"),
            foreground: sect("primary", "foreground"),
            accent: sect("normal", "blue"),
            selection: sect("selection", "background"),
            red: sect("normal", "red"),
            yellow: sect("normal", "yellow"),
            green: sect("normal", "green"),
            cyan: sect("normal", "cyan"),
            blue: sect("normal", "blue"),
            magenta: sect("normal", "magenta"),
            ..Default::default()
        };
        raw.background?;
        Some(Self::from_raw(&pretty_name(name), raw))
    }

    pub fn visuals(&self) -> Visuals {
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        v.dark_mode = self.dark;
        v.override_text_color = Some(self.text);
        v.panel_fill = self.mantle;
        v.window_fill = self.surface;
        v.extreme_bg_color = self.base;
        v.faint_bg_color = self.surface;
        v.code_bg_color = self.surface;
        v.hyperlink_color = self.accent;
        v.warn_fg_color = self.yellow;
        v.error_fg_color = self.red;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_shadow = egui::Shadow { offset: [0, 10], blur: 32, spread: 0, color: with_alpha(Color32::BLACK, if self.dark { 110 } else { 40 }) };
        v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: with_alpha(Color32::BLACK, if self.dark { 90 } else { 30 }) };
        v.selection.bg_fill = self.selection;
        v.selection.stroke = Stroke::new(1.0, self.bright);
        v.text_cursor.stroke = Stroke::new(2.0, self.accent);

        let r = CornerRadius::same(7);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.mantle;
        w.noninteractive.weak_bg_fill = self.mantle;
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = self.surface;
        w.inactive.weak_bg_fill = self.surface;
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(1.0, self.text);
        w.inactive.corner_radius = r;

        w.hovered.bg_fill = self.overlay;
        w.hovered.weak_bg_fill = self.overlay;
        w.hovered.bg_stroke = Stroke::new(1.0, mix(self.border, self.accent, 0.35));
        w.hovered.fg_stroke = Stroke::new(1.5, self.bright);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = mix(self.overlay, self.accent, 0.2);
        w.active.weak_bg_fill = mix(self.overlay, self.accent, 0.2);
        w.active.bg_stroke = Stroke::new(1.0, self.accent);
        w.active.fg_stroke = Stroke::new(1.5, self.bright);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open.bg_fill = self.overlay;
        w.open.weak_bg_fill = self.overlay;
        w.open.bg_stroke = Stroke::new(1.0, self.border);
        w.open.fg_stroke = Stroke::new(1.0, self.bright);
        w.open.corner_radius = r;
        v
    }
}

pub fn pretty_name(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn state_dir() -> PathBuf {
    home().join(".local/state/omarchy/current")
}

/// Directories in which Omarchy themes are installed (user themes first).
fn theme_roots() -> Vec<PathBuf> {
    vec![home().join(".config/omarchy/themes"), home().join(".local/share/omarchy/themes")]
}

pub fn list_themes() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for root in theme_roots() {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if !out.iter().any(|(n, _)| *n == name)
                        && (p.join("colors.toml").exists() || p.join("alacritty.toml").exists())
                    {
                        out.push((name, p));
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn current_omarchy_name() -> Option<String> {
    std::fs::read_to_string(state_dir().join("theme.name")).ok().map(|s| s.trim().to_string())
}

/// Load the theme that Omarchy currently has active.
pub fn load_omarchy_current() -> Option<Palette> {
    let name = current_omarchy_name().unwrap_or_else(|| "omarchy".into());
    let dir = state_dir().join("theme");
    Palette::load_dir(&dir, &name).or_else(|| {
        let p = list_themes().into_iter().find(|(n, _)| *n == name)?.1;
        Palette::load_dir(&p, &name)
    })
}

pub fn load_named(name: &str) -> Option<Palette> {
    let p = list_themes().into_iter().find(|(n, _)| n == name)?.1;
    Palette::load_dir(&p, name)
}

/// Cheap change detection for the active Omarchy theme.
pub fn omarchy_stamp() -> Option<SystemTime> {
    let a = std::fs::metadata(state_dir().join("theme.name")).and_then(|m| m.modified()).ok();
    let b = std::fs::metadata(state_dir().join("theme/colors.toml")).and_then(|m| m.modified()).ok();
    a.max(b)
}
