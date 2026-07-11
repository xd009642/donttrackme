mod app;
mod audio;
mod model;
mod piano_roll;
mod project_io;

use app::DawApp;

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "Don't Track Me",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 760.0])
                .with_min_inner_size([900.0, 560.0]),
            ..Default::default()
        },
        Box::new(|creation_context| Ok(Box::new(DawApp::new(creation_context)))),
    )
}
