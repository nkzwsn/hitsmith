#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod export;
mod hsimport;
mod icons;
mod library;
mod logo_paths;
mod osu;
mod project;
mod theme;
mod titlebar;

pub const LOGO_PNG: &[u8] = include_bytes!("../assets/icon-256.png");

fn main() -> eframe::Result {
    let mut viewport = eframe::egui::ViewportBuilder::default().with_title("hitsmith").with_inner_size([1280.0, 760.0]).with_min_inner_size([900.0, 520.0]);
    if let Ok(img) = image::load_from_memory(LOGO_PNG) {
        let img = img.to_rgba8();
        let (width, height) = img.dimensions();
        viewport = viewport.with_icon(std::sync::Arc::new(eframe::egui::IconData { rgba: img.into_raw(), width, height }));
    }
    // eframe's default vsync makes frames follow the monitor's refresh rate (60, 144, 240 Hz...).
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("hitsmith", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
