//! Desktop runner for trying the launcher on a PC (it shows a made-up list of apps).
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        // roughly phone-shaped, so layouts can be checked on the PC
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([405.0, 880.0]),
        ..Default::default()
    };
    eframe::run_native("Home", options, Box::new(|cc| Ok(Box::new(home::Home::new(cc, home::desktop_handle())))))
}
