//! Streamium for Windows and Linux.
//!
//! A thin shell over the same crates the Apple app uses: `streamium-core`
//! parses the playlists, Xtream listings and guides, `streamium-mpegts`
//! inspects the streams themselves, and this binary provides the interface,
//! the networking and the hand-off to a video player.

// A release build is a desktop application, not a console program: without
// this Windows opens a command prompt behind the window. Debug builds keep
// the console so `eprintln!` is visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analyse;
mod app;
mod config;
mod load;
mod net;
mod player;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([880.0, 560.0])
            .with_title("Streamium"),
        ..Default::default()
    };
    eframe::run_native(
        "Streamium",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
