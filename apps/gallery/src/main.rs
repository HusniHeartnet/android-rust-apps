//! Desktop runner for trying the gallery on a PC: `gallery-desktop <folder> [folder...]`
//! Set GALLERY_START=albums|photos|viewer|album:<label> to open straight onto a screen.
fn main() -> eframe::Result {
    let roots: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    let options = eframe::NativeOptions {
        // roughly phone-shaped, so layouts can be checked on the PC
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([405.0, 880.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Gallery",
        options,
        Box::new(move |cc| Ok(Box::new(gallery::Gallery::new(cc, roots.clone())))),
    )
}
