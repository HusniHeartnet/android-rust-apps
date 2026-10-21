//! The Heartnet Apps look: dark surfaces with the logo orange (#FC5700) as the accent.

use egui::Color32;

pub const ORANGE: Color32 = Color32::from_rgb(0xFC, 0x57, 0x00);
pub const BG: Color32 = Color32::from_rgb(18, 18, 20);
pub const CARD: Color32 = Color32::from_rgb(30, 30, 34);
pub const TEXT: Color32 = Color32::from_gray(235);
pub const MUTED: Color32 = Color32::from_gray(150);
pub const DANGER: Color32 = Color32::from_rgb(230, 70, 60);

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.hyperlink_color = ORANGE;
    v.selection.bg_fill = ORANGE.gamma_multiply(0.45);
    v.selection.stroke = egui::Stroke::new(1.0, ORANGE);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ORANGE);
    v.widgets.active.bg_fill = ORANGE;
    ctx.set_visuals(v);
}
