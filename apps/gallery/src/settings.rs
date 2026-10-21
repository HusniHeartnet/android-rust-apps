//! The settings page: grid columns, video and viewer behaviour, library and trash housekeeping.

use super::*;

/// A switch with a line of explanation under it. Returns true when it was changed.
fn switch(ui: &mut egui::Ui, value: &mut bool, title: &str, help: &str) -> bool {
    let changed = ui.add(egui::Checkbox::new(value, egui::RichText::new(title).size(16.0))).changed();
    ui.label(egui::RichText::new(help).small().weak());
    ui.add_space(10.0);
    changed
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).size(20.0).strong());
    ui.add_space(6.0);
}

impl Gallery {
    /// Open settings; from the picture viewer this remembers where to come back to.
    pub(crate) fn open_settings(&mut self) {
        self.settings_return = self.viewer.as_ref().map(|v| v.pos);
        self.close_viewer();
        self.settings_open = true;
    }

    pub(crate) fn close_settings(&mut self) {
        self.settings_open = false;
        self.confirm_empty = false;
        if let Some(pos) = self.settings_return.take() {
            if pos < self.view.len() {
                self.open_viewer(pos);
            }
        }
    }

    pub(crate) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("‹ Back").clicked() {
                    self.close_settings();
                }
                ui.heading("Settings");
            });
        });

        let live: Vec<&Item> = self.items.iter().filter(|i| !i.gone).collect();
        let photos = live.iter().filter(|i| !i.trashed && !i.video).count();
        let videos = live.iter().filter(|i| !i.trashed && i.video).count();
        let trashed = live.iter().filter(|i| i.trashed).count();

        let mut changed = false;
        let mut rescan = false;
        let mut open_trash = false;
        let mut open_hidden = false;
        let mut open_space = false;
        let mut lock_now = false;
        let mut empty_trash = false;
        egui::ScrollArea::vertical().id_salt("settings").auto_shrink([false; 2]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 0)).show(ui, |ui| {
                section(ui, "Grid");
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Pictures per row").size(16.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(self.cols < COLUMNS_MAX, egui::Button::new("+").min_size(vec2(44.0, 34.0))).clicked() {
                            self.set_columns(self.cols + 1);
                        }
                        ui.label(egui::RichText::new(self.cols.to_string()).size(18.0).strong());
                        if ui.add_enabled(self.cols > COLUMNS_MIN, egui::Button::new("-").min_size(vec2(44.0, 34.0))).clicked() {
                            self.set_columns(self.cols - 1);
                        }
                    });
                });
                ui.label(egui::RichText::new("Pinching the grid changes this too.").small().weak());
                ui.add_space(10.0);

                section(ui, "Videos");
                changed |= switch(ui, &mut self.loop_video, "Loop videos", "Start again from the beginning when a video ends.");
                changed |= switch(ui, &mut self.autoplay, "Play when opened", "Off: a video opens paused, with its first picture showing.");
                changed |= switch(ui, &mut self.double_tap_skip, "Skip strips on videos", "Narrow strips along the left and right edges of a video: double-tap one to skip back or forward, then keep tapping to skip more. Taps in a strip never show or hide the bars.");
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Skip length").size(16.0));
                    for secs in SKIP_CHOICES {
                        if ui.selectable_label(self.skip_secs == secs, egui::RichText::new(format!("{secs} s")).size(16.0)).clicked() {
                            self.skip_secs = secs;
                            changed = true;
                        }
                    }
                });
                ui.add_space(10.0);

                section(ui, "Viewer");
                changed |= switch(
                    ui,
                    &mut self.tap_hide,
                    "Tap to hide the bars",
                    "Tap a picture to hide or show the bars. On a video, tap its middle to play or pause, and anywhere else to hide the bars. Off: bars always show, and a tap on a video plays or pauses.",
                );
                changed |= switch(ui, &mut self.details_strip, "Name and date on photos", "A small line with the file name and date at the bottom of a photo.");

                section(ui, "Display");
                changed |= switch(
                    ui,
                    &mut self.request_refresh,
                    "Ask for the fastest refresh rate",
                    "Off by default: HyperOS keeps this app at 60 Hz either way, and turning this on moves it to \"Based on app settings\" in the phone's refresh-rate list.",
                );

                section(ui, "Hidden album");
                changed |= switch(ui, &mut self.show_hidden_tile, "Show Hidden in the Albums list", "Off: the card is not shown; open the hidden album from here instead.");
                ui.horizontal(|ui| {
                    if ui.add_sized([150.0, 38.0], egui::Button::new("Open hidden album")).clicked() {
                        open_hidden = true;
                    }
                    if self.hidden_unlocked && ui.add_sized([100.0, 38.0], egui::Button::new("Lock now")).clicked() {
                        lock_now = true;
                    }
                });
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Hidden photos and videos are moved to a private folder, taken out of Android's media list so other gallery apps and pickers cannot see them, and shown here only after your fingerprint or screen lock. They are not encrypted: a file manager that shows hidden folders could still list them.").small().weak());
                ui.add_space(10.0);

                section(ui, "Library");
                ui.label(format!("{photos} photos and {videos} videos found."));
                ui.label(egui::RichText::new("Folders that start with a dot or contain a .nomedia file are skipped, like Android's own gallery.").small().weak());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([150.0, 38.0], egui::Button::new("Rescan library")).clicked() {
                        rescan = true;
                    }
                    if ui.add_sized([150.0, 38.0], egui::Button::new("Free up space")).clicked() {
                        open_space = true;
                    }
                });
                ui.add_space(10.0);

                section(ui, "Trash");
                ui.label(if trashed == 0 { "The trash is empty.".to_string() } else { format!("{trashed} items in the trash.") });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(trashed > 0, egui::Button::new("Open trash").min_size(vec2(130.0, 38.0))).clicked() {
                        open_trash = true;
                    }
                    let label = if self.confirm_empty { format!("Tap again: delete {trashed} forever") } else { "Empty trash".to_string() };
                    let red = Color32::from_rgb(255, 120, 110);
                    if ui.add_enabled(trashed > 0, egui::Button::new(egui::RichText::new(label).color(red)).min_size(vec2(130.0, 38.0))).clicked() {
                        if self.confirm_empty {
                            empty_trash = true;
                        } else {
                            self.confirm_empty = true;
                        }
                    }
                });
                ui.add_space(70.0);
            });
        });

        if changed {
            self.save_settings();
        }
        if rescan {
            self.rescan(&ctx);
            self.say(&ctx, "Scanning…", None);
        }
        if open_space {
            self.open_space(&ctx);
        }
        if open_hidden {
            self.settings_return = None;
            self.settings_open = false;
            self.request_unlock(&ctx);
        }
        if lock_now {
            self.lock_hidden(&ctx);
        }
        if open_trash {
            self.settings_open = false;
            self.settings_return = None;
            self.open_album(TRASH_KEY);
        }
        if empty_trash {
            self.confirm_empty = false;
            self.empty_trash(&ctx);
        }
    }
}
