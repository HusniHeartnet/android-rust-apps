//! Notes: plain text notes kept as files, with search, pinning and a 30 day trash.

mod store;

use eframe::egui::{self, Color32, Context, RichText, vec2};
use heartnet_kit::settings::Settings;
use heartnet_kit::theme::{BG, CARD, DANGER, MUTED, ORANGE};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use store::{Note, Store};

/// How long after the last keystroke the note is written to disk.
const SAVE_DELAY: Duration = Duration::from_millis(700);
const TEXT_SIZES: [(&str, f32); 3] = [("Small", 16.0), ("Medium", 19.0), ("Large", 23.0)];

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    List,
    Edit(u64),
    Trash,
    Settings,
}

struct Toast {
    text: String,
    /// The deleted note, when the toast offers to bring it back.
    undo: Option<u64>,
    until: Instant,
}

pub struct Notes {
    store: Store,
    settings: Settings,
    screen: Screen,
    query: String,
    /// The text of the note being edited (written to the store after a short pause).
    buffer: String,
    dirty_since: Option<Instant>,
    focus_editor: bool,
    toast: Option<Toast>,
    confirm_empty_trash: bool,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    ctx: Context,
    #[cfg(target_os = "android")]
    android: Option<android_activity::AndroidApp>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn data_dir() -> PathBuf {
    std::env::var_os("NOTES_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("notes-data"))
}

impl Notes {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_dir(cc, data_dir())
    }

    fn with_dir(cc: &eframe::CreationContext<'_>, dir: PathBuf) -> Self {
        heartnet_kit::theme::apply(&cc.egui_ctx);
        heartnet_kit::diag::init(dir.join("logs"));
        heartnet_kit::diag::log("notes start");
        let store = Store::open(&dir.join("notes"), now_secs());
        let settings = Settings::load(&dir, "settings.txt");
        Self {
            store,
            settings,
            screen: Screen::List,
            query: String::new(),
            buffer: String::new(),
            dirty_since: None,
            focus_editor: false,
            toast: None,
            confirm_empty_trash: false,
            ctx: cc.egui_ctx.clone(),
            #[cfg(target_os = "android")]
            android: None,
        }
    }

    #[cfg(target_os = "android")]
    pub fn with_android(mut self, app: android_activity::AndroidApp) -> Self {
        heartnet_kit::android::install_lifecycle(&app, &self.ctx);
        self.android = Some(app);
        self
    }

    fn text_size(&self) -> f32 {
        TEXT_SIZES[(self.settings.int("textsize", 1).clamp(0, 2)) as usize].1
    }

    fn insets(&self, _ctx: &Context) -> (f32, f32) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android {
            return heartnet_kit::android::insets(app, _ctx);
        }
        (0.0, 0.0)
    }

    fn open_note(&mut self, id: u64) {
        self.buffer = self.store.get(id).map(|n| n.text.clone()).unwrap_or_default();
        self.dirty_since = None;
        self.focus_editor = self.buffer.is_empty();
        self.screen = Screen::Edit(id);
    }

    fn new_note(&mut self) {
        let id = self.store.create(now_secs());
        self.open_note(id);
    }

    /// Write the text being edited to the store now.
    fn flush(&mut self) {
        if let (Screen::Edit(id), Some(_)) = (self.screen, self.dirty_since) {
            if !self.store.save(id, &self.buffer, now_secs()) {
                heartnet_kit::diag::log("save failed");
                self.toast = Some(Toast { text: "Could not save this note".into(), undo: None, until: Instant::now() + Duration::from_secs(4) });
            }
            self.dirty_since = None;
        }
    }

    fn close_editor(&mut self) {
        self.flush();
        // an untouched new note must not stay in the list
        if let Screen::Edit(id) = self.screen {
            if self.buffer.trim().is_empty() {
                self.store.save(id, "", now_secs());
            }
        }
        self.screen = Screen::List;
    }

    fn delete_note(&mut self, id: u64) {
        let had_text = self.store.get(id).is_some_and(|n| !n.text.trim().is_empty());
        self.dirty_since = None;
        self.store.delete(id, now_secs());
        self.screen = Screen::List;
        if had_text {
            self.toast = Some(Toast { text: "Note deleted".into(), undo: Some(id), until: Instant::now() + Duration::from_secs(6) });
        }
    }

    fn go_back(&mut self, ctx: &Context) {
        match self.screen {
            Screen::Edit(_) => self.close_editor(),
            Screen::Trash | Screen::Settings => {
                self.confirm_empty_trash = false;
                self.screen = Screen::List;
            }
            Screen::List if !self.query.is_empty() => self.query.clear(),
            Screen::List => heartnet_kit::quit(ctx),
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, title: &str, back: bool) -> bool {
        let mut went_back = false;
        ui.horizontal(|ui| {
            if back && ui.add(egui::Button::new(RichText::new("‹").size(26.0)).frame(false).min_size(vec2(40.0, 40.0))).clicked() {
                went_back = true;
            }
            ui.label(RichText::new(title).size(24.0).strong());
        });
        went_back
    }

    fn list_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Notes").size(28.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("Settings").corner_radius(18.0).min_size(vec2(0.0, 34.0))).clicked() {
                    self.screen = Screen::Settings;
                }
                let label = if self.store.trashed.is_empty() { "Trash".to_string() } else { format!("Trash {}", self.store.trashed.len()) };
                if ui.add(egui::Button::new(label).corner_radius(18.0).min_size(vec2(0.0, 34.0))).clicked() {
                    self.screen = Screen::Trash;
                }
            });
        });
        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search notes").desired_width(f32::INFINITY).margin(egui::Margin::symmetric(12, 10)));
        ui.add_space(8.0);

        let view = self.store.view(&self.query);
        if view.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                let msg = if self.query.trim().is_empty() { "No notes yet.\nTap + to write one." } else { "Nothing matches your search." };
                ui.label(RichText::new(msg).size(18.0).color(MUTED));
            });
            return;
        }

        let mut open = None;
        let mut pin_header_done = false;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for &i in &view {
                let n = &self.store.notes[i];
                if n.pinned && !pin_header_done {
                    ui.label(RichText::new("Pinned").small().color(ORANGE));
                    pin_header_done = true;
                }
                if !n.pinned && pin_header_done {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Others").small().color(MUTED));
                    pin_header_done = false;
                }
                let resp = note_card(ui, n);
                if resp.clicked() {
                    open = Some(n.id);
                }
                ui.add_space(8.0);
            }
            ui.add_space(90.0);
        });
        if let Some(id) = open {
            self.open_note(id);
        }
    }

    fn fab(&mut self, ui: &mut egui::Ui, bottom: f32) {
        let screen = ui.max_rect();
        let size = 60.0;
        let rect = egui::Rect::from_min_size(egui::pos2(screen.right() - size - 20.0, screen.bottom() - size - 20.0 - bottom), vec2(size, size));
        let resp = ui.interact(rect, ui.make_persistent_id("fab"), egui::Sense::click());
        let fill = if resp.is_pointer_button_down_on() { ORANGE.gamma_multiply(0.8) } else { ORANGE };
        ui.painter().circle_filled(rect.center(), size / 2.0, fill);
        let c = rect.center();
        let s = egui::Stroke::new(3.5, Color32::WHITE);
        ui.painter().line_segment([c - vec2(11.0, 0.0), c + vec2(11.0, 0.0)], s);
        ui.painter().line_segment([c - vec2(0.0, 11.0), c + vec2(0.0, 11.0)], s);
        if resp.clicked() {
            self.new_note();
        }
    }

    fn edit_ui(&mut self, ui: &mut egui::Ui, id: u64) {
        let pinned = self.store.get(id).is_some_and(|n| n.pinned);
        let mut back = false;
        let mut delete = false;
        let mut toggle_pin = false;
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new("‹").size(26.0)).frame(false).min_size(vec2(40.0, 40.0))).clicked() {
                back = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Delete").color(DANGER)).corner_radius(18.0).min_size(vec2(0.0, 34.0))).clicked() {
                    delete = true;
                }
                if ui.add(egui::Button::new(if pinned { "Unpin" } else { "Pin" }).corner_radius(18.0).min_size(vec2(0.0, 34.0))).clicked() {
                    toggle_pin = true;
                }
            });
        });
        if let Some(n) = self.store.get(id) {
            if !n.text.is_empty() {
                ui.label(RichText::new(format!("Edited {}", heartnet_kit::timefmt::short(UNIX_EPOCH + Duration::from_secs(n.modified)))).small().color(MUTED));
            }
        }
        ui.add_space(4.0);

        let size = self.text_size();
        let mut changed = false;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            let edit = egui::TextEdit::multiline(&mut self.buffer)
                .font(egui::FontId::proportional(size))
                .hint_text("Start typing")
                .frame(egui::Frame::new())
                .desired_width(f32::INFINITY)
                .desired_rows(18)
                .lock_focus(true);
            let out = ui.add(edit);
            if self.focus_editor {
                out.request_focus();
                self.focus_editor = false;
            }
            changed = out.changed();
        });
        if changed {
            self.dirty_since = Some(Instant::now());
        }

        if toggle_pin {
            self.flush();
            // a note with no text has no file to pin
            if !self.buffer.trim().is_empty() {
                self.store.set_pinned(id, !pinned);
            }
        }
        if delete {
            self.delete_note(id);
        } else if back {
            self.close_editor();
        }
    }

    fn trash_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        if self.top_bar(ui, "Trash", true) {
            self.go_back(&ui.ctx().clone());
            return;
        }
        ui.label(RichText::new("Deleted notes stay here for 30 days.").small().color(MUTED));
        ui.add_space(8.0);
        if self.store.trashed.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("The trash is empty.").size(18.0).color(MUTED)));
            return;
        }
        if self.confirm_empty_trash {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Delete all for good?").color(DANGER));
                if ui.button("Yes, delete").clicked() {
                    self.store.empty_trash();
                    self.confirm_empty_trash = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_empty_trash = false;
                }
            });
        } else if ui.button("Empty trash").clicked() {
            self.confirm_empty_trash = true;
        }
        ui.add_space(8.0);

        let mut restore = None;
        let mut forever = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for n in &self.store.trashed {
                egui::Frame::new().fill(CARD).corner_radius(14.0).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(n.title()).size(17.0).strong());
                    ui.label(RichText::new(format!("Deleted {}", heartnet_kit::timefmt::short(UNIX_EPOCH + Duration::from_secs(n.modified)))).small().color(MUTED));
                    ui.horizontal(|ui| {
                        if ui.button("Restore").clicked() {
                            restore = Some(n.id);
                        }
                        if ui.add(egui::Button::new(RichText::new("Delete for good").color(DANGER))).clicked() {
                            forever = Some(n.id);
                        }
                    });
                });
                ui.add_space(8.0);
            }
        });
        if let Some(id) = restore {
            self.store.restore(id);
        }
        if let Some(id) = forever {
            self.store.delete_forever(id);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        if self.top_bar(ui, "Settings", true) {
            self.go_back(&ui.ctx().clone());
            return;
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Text size").size(18.0).strong());
        ui.add_space(6.0);
        let current = self.settings.int("textsize", 1).clamp(0, 2);
        ui.horizontal(|ui| {
            for (i, (name, _)) in TEXT_SIZES.iter().enumerate() {
                let on = current == i as i64;
                let b = egui::Button::new(*name).fill(if on { ORANGE } else { CARD }).corner_radius(18.0).min_size(vec2(90.0, 36.0));
                if ui.add(b).clicked() {
                    self.settings.set("textsize", i);
                }
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new("The quick brown fox jumps over the lazy dog.").size(self.text_size()));
        ui.add_space(24.0);
        ui.label(RichText::new("Where your notes are").size(18.0).strong());
        ui.label(RichText::new("Each note is a plain text file in the app's own folder. Nothing is sent anywhere.").small().color(MUTED));
        ui.add_space(16.0);
        ui.label(RichText::new(format!("{} notes, {} in the trash", self.store.notes.len(), self.store.trashed.len())).small().color(MUTED));
    }

    fn toast_ui(&mut self, ui: &mut egui::Ui, bottom: f32) {
        let Some(t) = &self.toast else { return };
        if Instant::now() >= t.until {
            self.toast = None;
            return;
        }
        let mut undo = None;
        let mut dismiss = false;
        let area = ui.max_rect();
        egui::Area::new(ui.make_persistent_id("toast")).fixed_pos(egui::pos2(area.left() + 16.0, area.bottom() - 150.0 - bottom)).show(ui.ctx(), |ui| {
            egui::Frame::new().fill(Color32::from_gray(52)).corner_radius(14.0).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.set_width((area.width() - 32.0).max(120.0));
                ui.horizontal(|ui| {
                    ui.label(&t.text);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(id) = t.undo {
                            if ui.add(egui::Button::new(RichText::new("Undo").color(ORANGE).strong()).frame(false)).clicked() {
                                undo = Some(id);
                            }
                        } else if ui.add(egui::Button::new("OK").frame(false)).clicked() {
                            dismiss = true;
                        }
                    });
                });
            });
        });
        if let Some(id) = undo {
            self.store.restore(id);
            self.toast = None;
        } else if dismiss {
            self.toast = None;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
}

fn note_card(ui: &mut egui::Ui, n: &Note) -> egui::Response {
    let width = ui.available_width();
    let preview = n.preview();
    let when = heartnet_kit::timefmt::short(UNIX_EPOCH + Duration::from_secs(n.modified));
    let frame = egui::Frame::new().fill(CARD).corner_radius(14.0).inner_margin(egui::Margin::symmetric(14, 12));
    let out = frame.show(ui, |ui| {
        ui.set_width(width - 28.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(n.title()).size(17.0).strong());
            if n.pinned {
                ui.label(RichText::new("●").size(10.0).color(ORANGE));
            }
        });
        if !preview.is_empty() {
            ui.label(RichText::new(preview).color(Color32::from_gray(190)));
        }
        ui.label(RichText::new(when).small().color(MUTED));
    });
    ui.interact(out.response.rect, out.response.id.with("tap"), egui::Sense::click())
}

impl eframe::App for Notes {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::BrowserBack)) {
            self.go_back(&ctx);
        }
        if self.dirty_since.is_some_and(|t| t.elapsed() >= SAVE_DELAY) {
            self.flush();
        } else if self.dirty_since.is_some() {
            ctx.request_repaint_after(SAVE_DELAY);
        }

        let (top, bottom) = self.insets(&ctx);
        let margin = egui::Margin { left: 14, right: 14, top: top.round() as i8, bottom: bottom.round() as i8 };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(margin)).show(ui, |ui| match self.screen {
            Screen::List => {
                self.list_ui(ui);
                self.fab(ui, bottom);
            }
            Screen::Edit(id) => self.edit_ui(ui, id),
            Screen::Trash => self.trash_ui(ui),
            Screen::Settings => self.settings_ui(ui),
        });
        self.toast_ui(ui, bottom);
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
    let handle = app.clone();
    let dir = heartnet_kit::android::files_dir(&app);
    let options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
    if let Err(e) = eframe::run_native("Notes", options, Box::new(move |cc| Ok(Box::new(Notes::with_dir(cc, dir.clone()).with_android(handle.clone()))))) {
        log::error!("notes exited: {e}");
    }
}
