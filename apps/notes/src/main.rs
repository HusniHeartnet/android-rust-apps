//! Desktop preview of Notes (phone-shaped window). Set NOTES_DIR to keep the notes somewhere specific.
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([405.0, 880.0]),
        ..Default::default()
    };
    eframe::run_native("Notes", options, Box::new(|cc| Ok(Box::new(notes::Notes::new(cc)))))
}
