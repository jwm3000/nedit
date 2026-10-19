//! System check: which TeX components and tools are missing, and how to install them on
//! this system (package manager detected: pacman, apt, dnf, zypper …; Homebrew on macOS,
//! winget / MiKTeX / TeX Live on Windows).

use crate::platform::{has_tool, kpsewhich};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pm {
    Pacman,
    Apt,
    Dnf,
    Zypper,
    Xbps,
    Apk,
    Brew,
    MacNoBrew,
    WinMiktex,
    WinTexlive,
    Winget,
    Unknown,
}

impl Pm {
    pub fn detect() -> Pm {
        if cfg!(target_os = "macos") {
            return if has_tool("brew") { Pm::Brew } else { Pm::MacNoBrew };
        }
        if cfg!(windows) {
            if has_tool("miktex") || has_tool("initexmf") {
                return Pm::WinMiktex;
            }
            if has_tool("tlmgr") {
                return Pm::WinTexlive;
            }
            return Pm::Winget;
        }
        for (tool, pm) in [("pacman", Pm::Pacman), ("apt-get", Pm::Apt), ("dnf", Pm::Dnf), ("zypper", Pm::Zypper), ("xbps-install", Pm::Xbps), ("apk", Pm::Apk)] {
            if has_tool(tool) {
                return pm;
            }
        }
        Pm::Unknown
    }

    pub fn label(self) -> &'static str {
        match self {
            Pm::Pacman => "pacman",
            Pm::Apt => "apt",
            Pm::Dnf => "dnf",
            Pm::Zypper => "zypper",
            Pm::Xbps => "xbps",
            Pm::Apk => "apk",
            Pm::Brew => "Homebrew",
            Pm::MacNoBrew => "macOS",
            Pm::WinMiktex => "MiKTeX",
            Pm::WinTexlive => "TeX Live",
            Pm::Winget => "winget",
            Pm::Unknown => "",
        }
    }

    pub(crate) fn install_cmd(self, pkgs: &[&str]) -> String {
        let p = pkgs.join(" ");
        match self {
            Pm::Pacman => format!("sudo pacman -S --needed {p}"),
            Pm::Apt => format!("sudo apt install {p}"),
            Pm::Dnf => format!("sudo dnf install {p}"),
            Pm::Zypper => format!("sudo zypper install {p}"),
            Pm::Xbps => format!("sudo xbps-install {p}"),
            Pm::Apk => format!("sudo apk add {p}"),
            Pm::Brew => format!("brew install {p}"),
            Pm::WinMiktex => format!("miktex packages install {p}"),
            Pm::WinTexlive | Pm::MacNoBrew => format!("tlmgr install {p}"),
            Pm::Winget => format!("winget install {p}"),
            Pm::Unknown => p,
        }
    }
}

/// Name of the Linux distribution (from /etc/os-release), or the OS.
pub fn os_name() -> String {
    if cfg!(target_os = "macos") {
        return "macOS".into();
    }
    if cfg!(windows) {
        return "Windows".into();
    }
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string())))
        .unwrap_or_else(|| "Linux".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Latexmk,
    Pdflatex,
    Xelatex,
    Lualatex,
    Biber,
    Poppler,
    Git,
    German,
    Koma,
    Biblatex,
    Beamer,
    Pgfplots,
    OcgP,
}

#[derive(Clone, Debug)]
pub struct Component {
    pub key: Key,
    pub name: &'static str,
    pub purpose: &'static str,
    pub required: bool,
    pub ok: bool,
    /// Command for this system (copyable), if there is one.
    pub command: Option<String>,
    /// Download / docs page.
    pub link: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub os: String,
    pub pm: Pm,
    pub items: Vec<Component>,
}

impl Report {
    pub fn missing_required(&self) -> usize {
        self.items.iter().filter(|c| c.required && !c.ok).count()
    }
    pub fn missing(&self) -> usize {
        self.items.iter().filter(|c| !c.ok).count()
    }
    /// One command that installs everything that's missing (where possible).
    pub fn install_all(&self) -> Option<String> {
        let pkgs: Vec<&str> = self.items.iter().filter(|c| !c.ok).flat_map(|c| packages(self.pm, c.key)).collect();
        let mut uniq: Vec<&str> = vec![];
        for p in pkgs {
            if !uniq.contains(&p) {
                uniq.push(p);
            }
        }
        (!uniq.is_empty() && !matches!(self.pm, Pm::Unknown | Pm::Winget | Pm::MacNoBrew)).then(|| self.pm.install_cmd(&uniq))
    }
}

/// Packages that provide a component with the given package manager.
fn packages(pm: Pm, k: Key) -> Vec<&'static str> {
    use Key::*;
    match pm {
        Pm::Pacman => match k {
            Latexmk => vec!["texlive-binextra"],
            Pdflatex => vec!["texlive-basic", "texlive-latex"],
            Xelatex => vec!["texlive-xetex"],
            Lualatex => vec!["texlive-luatex"],
            Biber => vec!["biber"],
            Poppler => vec!["poppler"],
            Git => vec!["git"],
            German => vec!["texlive-langgerman"],
            Koma | Beamer => vec!["texlive-latexrecommended"],
            Biblatex => vec!["texlive-bibtexextra"],
            Pgfplots => vec!["texlive-pictures"],
            OcgP => vec!["texlive-latexextra"],
        },
        Pm::Apt => match k {
            Latexmk => vec!["latexmk"],
            Pdflatex => vec!["texlive-latex-base"],
            Xelatex => vec!["texlive-xetex"],
            Lualatex => vec!["texlive-luatex"],
            Biber => vec!["biber"],
            Poppler => vec!["poppler-utils"],
            Git => vec!["git"],
            German => vec!["texlive-lang-german"],
            Koma | Beamer => vec!["texlive-latex-recommended"],
            Biblatex => vec!["texlive-bibtex-extra"],
            Pgfplots => vec!["texlive-pictures"],
            OcgP => vec!["texlive-latex-extra"],
        },
        // Fedora and openSUSE packages provide tex(<file>) – exact and robust
        Pm::Dnf | Pm::Zypper => match k {
            Latexmk => vec![if pm == Pm::Dnf { "latexmk" } else { "texlive-latexmk" }],
            Pdflatex => vec!["'tex(latex)'", "'tex(pdftex)'"],
            Xelatex => vec!["texlive-xetex"],
            Lualatex => vec!["texlive-luatex"],
            Biber => vec![if pm == Pm::Dnf { "biber" } else { "texlive-biber" }],
            Poppler => vec![if pm == Pm::Dnf { "poppler-utils" } else { "poppler-tools" }],
            Git => vec!["git"],
            German => vec!["'tex(ngerman.ldf)'"],
            Koma => vec!["'tex(scrreprt.cls)'"],
            Beamer => vec!["'tex(beamer.cls)'"],
            Biblatex => vec!["'tex(biblatex.sty)'"],
            Pgfplots => vec!["'tex(pgfplots.sty)'"],
            OcgP => vec!["'tex(ocg-p.sty)'"],
        },
        Pm::Xbps | Pm::Apk => match k {
            Poppler => vec!["poppler-utils"],
            Git => vec!["git"],
            Biber => vec!["biber"],
            _ => vec!["texlive-full"],
        },
        Pm::Brew => match k {
            Poppler => vec!["poppler"],
            Git => vec!["git"],
            _ => vec![], // TeX parts come from MacTeX (see command)
        },
        // TeX Live (tlmgr) / MiKTeX package names
        Pm::WinMiktex | Pm::WinTexlive | Pm::MacNoBrew => match k {
            Latexmk => vec!["latexmk"],
            Biber => vec!["biber"],
            German => vec!["babel-german", "hyphen-german"],
            Koma => vec!["koma-script"],
            Beamer => vec!["beamer"],
            Biblatex => vec!["biblatex"],
            Pgfplots => vec!["pgfplots"],
            OcgP => vec!["ocg-p"],
            _ => vec![],
        },
        Pm::Winget => match k {
            Git => vec!["Git.Git"],
            _ => vec![],
        },
        Pm::Unknown => vec![],
    }
}

fn command_for(pm: Pm, k: Key) -> (Option<String>, Option<&'static str>) {
    use Key::*;
    let tex_link = match pm {
        Pm::Brew | Pm::MacNoBrew => "https://tug.org/mactex/",
        Pm::WinMiktex | Pm::Winget => "https://miktex.org/download",
        Pm::WinTexlive => "https://tug.org/texlive/windows.html",
        _ => "https://tug.org/texlive/quickinstall.html",
    };
    let is_tex = !matches!(k, Poppler | Git);
    match (pm, k) {
        // macOS: the whole TeX side comes with MacTeX
        (Pm::Brew, _) if is_tex => (Some("brew install --cask mactex-no-gui".into()), Some(tex_link)),
        (Pm::MacNoBrew, Poppler) => (None, Some("https://brew.sh")),
        (Pm::MacNoBrew, Git) => (Some("xcode-select --install".into()), None),
        (Pm::MacNoBrew, Pdflatex | Xelatex | Lualatex) => (None, Some(tex_link)),
        // Windows
        (Pm::Winget, Pdflatex | Xelatex | Lualatex | Latexmk | Biber | Koma | Beamer | Biblatex | Pgfplots | OcgP | German) => {
            (Some("winget install MiKTeX.MiKTeX".into()), Some(tex_link))
        }
        (Pm::WinMiktex | Pm::WinTexlive | Pm::Winget, Poppler) => (None, Some("https://github.com/oschwartz10612/poppler-windows/releases")),
        (Pm::WinMiktex | Pm::WinTexlive, Git) => (Some("winget install Git.Git".into()), Some("https://git-scm.com/download/win")),
        (Pm::WinMiktex, Latexmk) => (Some("miktex packages install latexmk  &  winget install StrawberryPerl.StrawberryPerl".into()), None),
        (Pm::WinMiktex | Pm::WinTexlive, Pdflatex | Xelatex | Lualatex) => (None, Some(tex_link)),
        (Pm::Unknown, _) => (None, Some(if is_tex { tex_link } else if k == Git { "https://git-scm.com/downloads" } else { "https://poppler.freedesktop.org" })),
        _ => {
            let pkgs = packages(pm, k);
            if pkgs.is_empty() {
                (None, Some(tex_link))
            } else {
                (Some(pm.install_cmd(&pkgs)), None)
            }
        }
    }
}

/// Copyable install command for one component (used in compile error hints).
pub fn install_hint(k: Key) -> String {
    let pm = Pm::detect();
    match command_for(pm, k) {
        (Some(c), _) => c,
        (None, Some(l)) => l.to_string(),
        _ => String::new(),
    }
}

/// Run all checks (calls kpsewhich a few times – run it off the UI thread).
pub fn check() -> Report {
    use Key::*;
    let pm = Pm::detect();
    let kpse = has_tool("kpsewhich");
    let file = |f: &str| kpse && kpsewhich(f).is_some();
    let list: Vec<(Key, &'static str, &'static str, bool, bool)> = vec![
        (Pdflatex, "pdfLaTeX", tr!("Kompiliert Arbeit und Folien" | "Compiles thesis and slides"), true, has_tool("pdflatex")),
        (Latexmk, "latexmk", tr!("Steuert das Kompilieren (mehrere Durchläufe)" | "Runs the compile passes"), true, has_tool("latexmk")),
        (Biber, "biber", tr!("Literaturverzeichnis (biblatex)" | "Bibliography (biblatex)"), true, has_tool("biber")),
        (Poppler, "Poppler", tr!("PDF-Vorschau (pdftoppm, pdfinfo)" | "PDF preview (pdftoppm, pdfinfo)"), true, has_tool("pdftoppm") && has_tool("pdfinfo")),
        (Koma, "KOMA-Script", tr!("Klasse der TU-Graz-Arbeitsvorlage" | "Class of the TU Graz thesis template"), true, file("scrreprt.cls")),
        (Biblatex, "biblatex", tr!("Zitieren in der Vorlage" | "Citations in the template"), true, file("biblatex.sty")),
        (Beamer, "Beamer", tr!("Präsentation (TU-Graz-Theme)" | "Presentation (TU Graz theme)"), true, file("beamer.cls")),
        (OcgP, "ocg-p", tr!("Vom TU-Graz-Beamer-Theme benötigt" | "Needed by the TU Graz beamer theme"), true, file("ocg-p.sty")),
        (Pgfplots, "pgfplots / TikZ", tr!("Diagramme" | "Charts"), false, file("pgfplots.sty")),
        (German, tr!("Deutsche Silbentrennung" | "German hyphenation"), tr!("Deutsche Kurzfassung und Texte" | "German abstract and texts"), false, file("ngerman.ldf")),
        (Xelatex, "XeLaTeX", tr!("Optional – für Systemschriften" | "Optional – for system fonts"), false, has_tool("xelatex")),
        (Lualatex, "LuaLaTeX", tr!("Optional – für Systemschriften" | "Optional – for system fonts"), false, has_tool("lualatex")),
        (Git, "Git", tr!("Versionen und Verlauf" | "Versions and history"), false, has_tool("git")),
    ];
    // screenshot mode can pretend components are missing (NEDIT_FAKE_MISSING=biber,ocg-p)
    let fake = std::env::var("NEDIT_FAKE_MISSING").ok().filter(|_| std::env::var_os("NEDIT_SHOT").is_some()).unwrap_or_default().to_lowercase();
    let items = list
        .into_iter()
        .map(|(key, name, purpose, required, ok)| {
            let ok = ok && !fake.split(',').any(|f| !f.is_empty() && name.to_lowercase().contains(f.trim()));
            let (command, link) = command_for(pm, key);
            Component { key, name, purpose, required, ok, command, link }
        })
        .collect();
    Report { os: os_name(), pm, items }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_per_system() {
        assert_eq!(command_for(Pm::Pacman, Key::Biber).0.unwrap(), "sudo pacman -S --needed biber");
        assert_eq!(command_for(Pm::Apt, Key::German).0.unwrap(), "sudo apt install texlive-lang-german");
        assert_eq!(command_for(Pm::Dnf, Key::Koma).0.unwrap(), "sudo dnf install 'tex(scrreprt.cls)'");
        assert_eq!(command_for(Pm::Brew, Key::Biber).0.unwrap(), "brew install --cask mactex-no-gui");
        assert_eq!(command_for(Pm::Brew, Key::Poppler).0.unwrap(), "brew install poppler");
        assert!(command_for(Pm::Winget, Key::Pdflatex).0.unwrap().contains("MiKTeX"));
        assert!(command_for(Pm::WinMiktex, Key::Poppler).1.is_some());
        let r = Report {
            os: "x".into(),
            pm: Pm::Pacman,
            items: [Key::Koma, Key::Beamer, Key::Biber]
                .iter()
                .map(|k| Component { key: *k, name: "", purpose: "", required: true, ok: false, command: None, link: None })
                .collect(),
        };
        assert_eq!(r.install_all().unwrap(), "sudo pacman -S --needed texlive-latexrecommended biber");
    }
}
