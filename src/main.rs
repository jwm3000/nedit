//! nEdit — a native LaTeX studio for theses and talks.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod app;
mod bib;
mod compile;
mod editor;
mod filetree;
mod fonts;
mod git;
mod help;
mod icons;
mod pdfview;
mod platform;
mod project;
mod quickopen;
mod shelf;
mod shelf_ui;
mod theme;
mod updater;
mod vim;
mod visual;
mod widgets;
mod workspace;

/// Window icon (taskbar / title bar); on Linux the desktop file's icon is used as well.
fn app_icon() -> egui::IconData {
    let png = include_bytes!("../assets/nedit-256.png");
    match image::load_from_memory(png) {
        Ok(img) => {
            let img = img.to_rgba8();
            let (width, height) = img.dimensions();
            egui::IconData { rgba: img.into_raw(), width, height }
        }
        Err(_) => egui::IconData::default(),
    }
}

fn main() -> eframe::Result {
    updater::cleanup(); // also records our executable path before any update
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("nEdit {}", updater::VERSION);
        return Ok(());
    }
    if args.iter().any(|a| a == "--update") {
        match updater::cli_update() {
            Ok(m) => println!("{m}"),
            Err(e) => {
                eprintln!("{}", trf!("Fehler: {e}" | "Error: {e}"));
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", trf!("nEdit {} – LaTeX Studio\n\n  nedit            App starten\n  nedit --update   auf das neueste Release aktualisieren\n  nedit --version  Version anzeigen" | "nEdit {} – LaTeX Studio\n\n  nedit            start the app\n  nedit --update   update to the latest release\n  nedit --version  show version", updater::VERSION));
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("nEdit")
            .with_app_id("nedit")
            .with_inner_size([1500.0, 920.0])
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true)
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native("nEdit", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
