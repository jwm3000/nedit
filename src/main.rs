//! nEdit — a native LaTeX studio for theses and talks.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod bib;
mod compile;
mod editor;
mod filetree;
mod fonts;
mod git;
mod icons;
mod pdfview;
mod platform;
mod project;
mod shelf;
mod shelf_ui;
mod theme;
mod visual;
mod widgets;
mod workspace;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("nEdit")
            .with_app_id("nedit")
            .with_inner_size([1500.0, 920.0])
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("nEdit", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
