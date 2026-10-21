//! Desktop runner for trying Files on a PC: `files-desktop [storage-folder]`.
//! Set FILES_START=folder:<name>|category:image|recent|large|trash|settings|select:<name> to open onto a screen.
fn main() -> eframe::Result {
    let root: Option<std::path::PathBuf> = std::env::args().nth(1).map(Into::into);
    let options = eframe::NativeOptions {
        // roughly phone-shaped, so layouts can be checked on the PC
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([405.0, 880.0]),
        ..Default::default()
    };
    eframe::run_native("Files", options, Box::new(move |cc| Ok(Box::new(files::Files::new(cc, root.clone())))))
}
