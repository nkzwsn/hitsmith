#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod export;
mod library;
mod osu;
mod project;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title("hitsmith").with_inner_size([1280.0, 760.0]).with_min_inner_size([900.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native("hitsmith", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
