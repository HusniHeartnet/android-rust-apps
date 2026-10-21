//! Shared pieces of the Heartnet Apps apps: the exit fix and Android calls, saved settings, the theme
//! and the diagnostic log. Each app depends on this crate instead of keeping its own copy.

pub mod diag;
pub mod settings;
pub mod theme;
pub mod timefmt;

#[cfg(target_os = "android")]
pub mod android;

/// Quit by closing the window loop while the activity is still on screen. Asking Android to
/// finish first leaves the process stuck, and the next open shows a blank screen.
pub fn quit(ctx: &egui::Context) {
    diag::log("quitting");
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    ctx.request_repaint();
}
