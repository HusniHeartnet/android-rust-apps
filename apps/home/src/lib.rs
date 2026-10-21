//! Home: a small, fast launcher. A clock, a search box, every app in one grid (no pages, no folders to
//! dig through), and a dock of your favourites. Touch and hold an app to pin it, hide it or open its
//! settings page.

mod sys;

use eframe::egui::{self, Align2, Color32, Context, FontId, Rect, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sys::{App, Handle};

const BRAND: Color32 = Color32::from_rgb(252, 87, 0);
const BG: Color32 = Color32::from_rgb(12, 12, 14);
const ICON_PX: usize = 96;
const DOCK_MAX: usize = 5;

enum Loaded {
    Apps(Vec<App>),
    Icon(String, egui::ColorImage),
}

pub struct Home {
    ctx: Context,
    handle: Handle,
    data_dir: PathBuf,
    apps: Vec<App>,
    icons: HashMap<String, TextureHandle>,
    icon_requested: HashSet<String>,
    rx: Receiver<Loaded>,
    tx: Sender<Loaded>,
    search: String,
    pins: Vec<String>,
    hidden: HashSet<String>,
    show_hidden: bool,
    menu: Option<String>,
    was_focused: bool,
    scroll_top: bool,
    insets: (f32, f32),
    last_refresh: Instant,
    started: Instant,
    first_frame_logged: bool,
    #[cfg(target_os = "android")]
    android: Option<android_activity::AndroidApp>,
}

fn data_dir() -> PathBuf {
    if cfg!(target_os = "android") { PathBuf::from("/sdcard/Android/data/dev.heartnetapps.home/files") } else { std::env::temp_dir().join("home-data") }
}

fn read_list(dir: &Path, name: &str) -> Vec<String> {
    std::fs::read_to_string(dir.join(name)).map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()).unwrap_or_default()
}

fn write_list(dir: &Path, name: &str, items: &[String]) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(name), items.join("\n"));
}

/// (hour, minute, "Wed 21 Oct") in local time.
#[cfg(unix)]
fn clock_now() -> (u32, u32, String) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // SAFETY: localtime_r only writes into the zeroed tm we hand it.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let tt = secs as libc::time_t;
        libc::localtime_r(&tt, &mut tm);
        (tm.tm_hour as u32, tm.tm_min as u32, date_text(tm.tm_wday as usize, tm.tm_mday as u32, tm.tm_mon as usize))
    }
}

#[cfg(not(unix))]
fn clock_now() -> (u32, u32, String) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let wday = ((days + 4).rem_euclid(7)) as usize; // 1970-01-01 was a Thursday
    ((rem / 3600) as u32, (rem % 3600 / 60) as u32, date_text(wday, d as u32, (m - 1) as usize))
}

fn date_text(wday: usize, day: u32, month0: usize) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {day} {}", DAYS[wday % 7], MONTHS[month0 % 12])
}

/// The apps to show for a search text: a name that contains it (case-insensitive), apps whose name
/// starts with it first. Hidden apps are left out unless asked for.
fn visible_apps<'a>(apps: &'a [App], search: &str, hidden: &HashSet<String>, show_hidden: bool) -> Vec<&'a App> {
    let q = search.trim().to_lowercase();
    let mut out: Vec<&App> = apps.iter().filter(|a| (show_hidden || !hidden.contains(&a.key())) && (q.is_empty() || a.label.to_lowercase().contains(&q))).collect();
    if !q.is_empty() {
        out.sort_by_key(|a| !a.label.to_lowercase().starts_with(&q)); // stable: keeps the alphabetical order inside each group
    }
    out
}

impl Home {
    pub fn new(cc: &eframe::CreationContext<'_>, handle: Handle) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(Color32::from_gray(235));
        visuals.selection.bg_fill = BRAND;
        cc.egui_ctx.set_visuals(visuals);
        let data_dir = data_dir();
        let (tx, rx) = channel();
        let me = Self {
            ctx: cc.egui_ctx.clone(),
            handle,
            pins: read_list(&data_dir, "dock.txt"),
            hidden: read_list(&data_dir, "hidden.txt").into_iter().collect(),
            data_dir,
            apps: Vec::new(),
            icons: HashMap::new(),
            icon_requested: HashSet::new(),
            rx,
            tx,
            search: String::new(),
            show_hidden: false,
            menu: None,
            was_focused: true,
            scroll_top: false,
            insets: (0.0, 0.0),
            last_refresh: Instant::now(),
            started: Instant::now(),
            first_frame_logged: false,
            #[cfg(target_os = "android")]
            android: None,
        };
        me.refresh_apps();
        me
    }

    #[cfg(target_os = "android")]
    pub fn with_android(mut self, app: android_activity::AndroidApp) -> Self {
        *CTX.lock().unwrap() = Some(self.ctx.clone());
        sys::register_destroy_hook(&app, native_destroying);
        self.android = Some(app);
        self
    }

    /// Ask Android for the app list in the background.
    fn refresh_apps(&self) {
        let (handle, tx, ctx) = (self.handle.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let mut apps = sys::list_apps(&handle);
            sys::sort_apps(&mut apps);
            let _ = tx.send(Loaded::Apps(apps));
            ctx.request_repaint();
        });
    }

    fn load_icons(&mut self) {
        let wanted: Vec<App> = self.apps.iter().filter(|a| !self.icons.contains_key(&a.key()) && !self.icon_requested.contains(&a.key())).cloned().collect();
        if wanted.is_empty() {
            return;
        }
        for a in &wanted {
            self.icon_requested.insert(a.key());
        }
        let (handle, tx, ctx) = (self.handle.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            for a in wanted {
                if let Some(img) = sys::icon(&handle, &a, ICON_PX) {
                    let _ = tx.send(Loaded::Icon(a.key(), img));
                    ctx.request_repaint();
                }
            }
        });
    }

    fn drain(&mut self, ctx: &Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Loaded::Apps(apps) => {
                    if apps != self.apps {
                        self.apps = apps;
                    }
                    // a pin whose app is gone is dropped
                    let keys: HashSet<String> = self.apps.iter().map(App::key).collect();
                    if !self.apps.is_empty() && self.pins.iter().any(|p| !keys.contains(p)) {
                        self.pins.retain(|p| keys.contains(p));
                        write_list(&self.data_dir, "dock.txt", &self.pins);
                    }
                    self.load_icons();
                }
                Loaded::Icon(key, img) => {
                    let tex = ctx.load_texture(format!("i{}", self.icons.len()), img, TextureOptions::LINEAR);
                    self.icons.insert(key, tex);
                }
            }
        }
    }

    fn app_by_key(&self, key: &str) -> Option<App> {
        self.apps.iter().find(|a| a.key() == key).cloned()
    }

    fn launch(&mut self, app: &App) {
        sys::launch(&self.handle, app);
    }

    fn icon_ui(&self, ui: &egui::Ui, rect: Rect, app: &App) {
        match self.icons.get(&app.key()) {
            Some(tex) => egui::Image::new((tex.id(), rect.size())).paint_at(ui, rect),
            None => {
                ui.painter().rect_filled(rect, 14.0, Color32::from_gray(34));
            }
        }
    }

    fn ui(&mut self, ctx: &Context, ui: &mut egui::Ui) {
        let (top, bottom) = self.insets;
        let width = ui.available_width();
        let mut tapped: Option<String> = None;
        let mut held: Option<String> = None;

        // ---- clock and search ----
        let (h, m, date) = clock_now();
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(format!("{h:02}:{m:02}")).size(56.0).strong());
                ui.label(egui::RichText::new(date).size(16.0).color(Color32::from_gray(170)));
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search apps").desired_width(width - 32.0 - 76.0));
            let label = if self.show_hidden { "Hide hidden" } else { "Hidden" };
            if !self.hidden.is_empty() && ui.add(egui::Button::new(egui::RichText::new(label).size(12.0)).min_size(vec2(60.0, 28.0))).clicked() {
                self.show_hidden = !self.show_hidden;
            }
        });
        ui.add_space(6.0);

        // ---- the grid ----
        let dock_room = if self.pins.is_empty() { 20.0 } else { 104.0 };
        let apps = self.apps.clone();
        let list = visible_apps(&apps, &self.search, &self.hidden, self.show_hidden);
        let cols = ((width - 16.0) / 78.0).floor().max(3.0) as usize;
        let cell_w = (width - 16.0) / cols as f32;
        let cell_h = 92.0;
        let rows = list.len().div_ceil(cols);
        let mut area = egui::ScrollArea::vertical().id_salt("grid").auto_shrink([false; 2]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        if std::mem::take(&mut self.scroll_top) {
            area = area.vertical_scroll_offset(0.0);
        }
        area.show_viewport(ui, |ui, viewport| {
            let (_, content) = ui.allocate_space(vec2(width, rows as f32 * cell_h + dock_room + bottom));
            let first = (viewport.min.y / cell_h).floor().max(0.0) as usize;
            let last = ((viewport.max.y / cell_h).ceil() as usize).min(rows);
            for idx in (first * cols)..(last * cols).min(list.len()) {
                let app = list[idx];
                let (r, c) = (idx / cols, idx % cols);
                let cell = Rect::from_min_size(content.min + vec2(8.0 + c as f32 * cell_w, r as f32 * cell_h), vec2(cell_w, cell_h));
                let resp = ui.interact(cell, ui.make_persistent_id(("app", idx)), Sense::click());
                if resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(cell.shrink(3.0), 14.0, Color32::from_gray(30));
                }
                self.icon_ui(ui, Rect::from_center_size(pos2(cell.center().x, cell.min.y + 36.0), vec2(52.0, 52.0)), app);
                let clip = ui.painter().with_clip_rect(cell.shrink2(vec2(2.0, 0.0)));
                let faded = self.hidden.contains(&app.key());
                clip.text(pos2(cell.center().x, cell.min.y + 74.0), Align2::CENTER_CENTER, &app.label, FontId::proportional(12.0), if faded { Color32::from_gray(110) } else { Color32::from_gray(225) });
                if resp.long_touched() || resp.secondary_clicked() {
                    held = Some(app.key());
                } else if resp.clicked() {
                    tapped = Some(app.key());
                }
            }
        });

        // ---- the dock ----
        if !self.pins.is_empty() && self.search.is_empty() {
            let pinned: Vec<App> = self.pins.iter().filter_map(|k| self.app_by_key(k)).collect();
            let mut dock_tap = None;
            let mut dock_hold = None;
            egui::Area::new(egui::Id::new("dock")).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -(bottom + 8.0))).order(egui::Order::Foreground).show(ctx, |ui| {
                egui::Frame::new().fill(Color32::from_gray(30)).corner_radius(30.0).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for app in &pinned {
                            let (rect, resp) = ui.allocate_exact_size(vec2(60.0, 60.0), Sense::click());
                            if resp.is_pointer_button_down_on() {
                                ui.painter().rect_filled(rect, 16.0, Color32::from_gray(50));
                            }
                            self.icon_ui(ui, Rect::from_center_size(rect.center(), vec2(52.0, 52.0)), app);
                            if resp.long_touched() || resp.secondary_clicked() {
                                dock_hold = Some(app.key());
                            } else if resp.clicked() {
                                dock_tap = Some(app.key());
                            }
                        }
                    });
                });
            });
            tapped = tapped.or(dock_tap);
            held = held.or(dock_hold);
        }
        let _ = top;

        if let Some(k) = tapped {
            if self.menu.is_some() {
                self.menu = None;
            } else if let Some(app) = self.app_by_key(&k) {
                self.launch(&app);
            }
        }
        if let Some(k) = held {
            self.menu = Some(k);
        }
        self.menu_ui(ctx);
    }

    fn menu_ui(&mut self, ctx: &Context) {
        let Some(key) = self.menu.clone() else { return };
        let Some(app) = self.app_by_key(&key) else {
            self.menu = None;
            return;
        };
        let pinned = self.pins.contains(&key);
        let hidden = self.hidden.contains(&key);
        let (mut pin, mut hide, mut info, mut close) = (false, false, false, false);
        let screen = ctx.viewport_rect();
        egui::Area::new(egui::Id::new("menu")).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width((screen.width() - 80.0).min(300.0));
                ui.label(egui::RichText::new(&app.label).strong().size(18.0));
                ui.add_space(6.0);
                let w = ui.available_width();
                if ui.add_sized([w, 38.0], egui::Button::new(if pinned { "Remove from dock" } else { "Add to dock" })).clicked() {
                    pin = true;
                }
                if ui.add_sized([w, 38.0], egui::Button::new(if hidden { "Show in the list" } else { "Hide from the list" })).clicked() {
                    hide = true;
                }
                if ui.add_sized([w, 38.0], egui::Button::new("App info")).clicked() {
                    info = true;
                }
                if ui.add_sized([w, 38.0], egui::Button::new("Close")).clicked() {
                    close = true;
                }
            });
        });
        if pin {
            if pinned {
                self.pins.retain(|p| *p != key);
            } else if self.pins.len() < DOCK_MAX {
                self.pins.push(key.clone());
            }
            write_list(&self.data_dir, "dock.txt", &self.pins);
        }
        if hide {
            if hidden {
                self.hidden.remove(&key);
            } else {
                self.hidden.insert(key.clone());
            }
            let mut v: Vec<String> = self.hidden.iter().cloned().collect();
            v.sort();
            write_list(&self.data_dir, "hidden.txt", &v);
        }
        if info {
            sys::app_info(&self.handle, &app);
        }
        if pin || hide || info || close {
            self.menu = None;
        }
    }
}

impl eframe::App for Home {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            log::info!("home: first frame after {} ms", self.started.elapsed().as_millis());
        }
        self.drain(&ctx);
        // back at the front (the Home button): clear the search, go to the top, look for new apps
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused {
            self.search.clear();
            self.menu = None;
            self.show_hidden = false;
            self.scroll_top = true;
            if self.last_refresh.elapsed() > Duration::from_secs(2) {
                self.last_refresh = Instant::now();
                self.refresh_apps();
            }
        }
        self.was_focused = focused;
        // the clock moves on every minute
        ctx.request_repaint_after(Duration::from_secs(20));
        self.insets = insets(self, &ctx);
        let (top, bottom) = self.insets;
        let margin = egui::Margin { left: 0, right: 0, top: top.round() as i8, bottom: bottom.round() as i8 };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(margin)).show(ui, |ui| {
            Home::ui(self, &ctx, ui);
        });
        let _ = Stroke::NONE;
        let _ = Arc::new(Mutex::new(0));
    }
}

#[cfg(target_os = "android")]
const FALLBACK_INSETS: (f32, f32) = (30.0, 36.0);

fn insets(_h: &Home, _ctx: &Context) -> (f32, f32) {
    #[cfg(target_os = "android")]
    if let Some(app) = &_h.android {
        let r = app.content_rect();
        let ppp = _ctx.pixels_per_point();
        let win_h = _ctx.viewport_rect().height() * ppp;
        let top = r.top.max(0) as f32 / ppp;
        let bottom = (win_h - r.bottom as f32).max(0.0) / ppp;
        return if top + bottom > 0.5 { (top, bottom) } else { FALLBACK_INSETS };
    }
    (0.0, 0.0)
}

/// The window loop's context, so the destroy hook (called from Java on the main thread) can end it.
#[cfg(target_os = "android")]
static CTX: Mutex<Option<Context>> = Mutex::new(None);

/// Called by `HomeActivity.onDestroy` just before Android waits for the app thread to stop. The
/// windowing library ignores Android's destroy notice, so ask the window to close here.
#[cfg(target_os = "android")]
extern "system" fn native_destroying(_env: jni::JNIEnv, _class: jni::objects::JClass) {
    if let Some(ctx) = CTX.lock().ok().and_then(|g| g.clone()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
    let handle = app.clone();
    let options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
    if let Err(e) = eframe::run_native("Home", options, Box::new(move |cc| Ok(Box::new(Home::new(cc, handle.clone()).with_android(handle.clone()))))) {
        log::error!("home exited: {e}");
    }
}

#[cfg(not(target_os = "android"))]
pub fn desktop_handle() -> Handle {
    sys::Handle
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(pkg: &str, label: &str) -> App {
        App { pkg: pkg.into(), cls: "Main".into(), label: label.into() }
    }

    #[test]
    fn search_matches_names_and_puts_prefix_matches_first() {
        let apps = vec![app("a", "Camera"), app("b", "Chrome"), app("c", "Scanner"), app("d", "Clock")];
        let none = HashSet::new();
        let all: Vec<_> = visible_apps(&apps, "", &none, false).iter().map(|a| a.pkg.clone()).collect();
        assert_eq!(all, ["a", "b", "c", "d"]);
        let r: Vec<_> = visible_apps(&apps, "c", &none, false).iter().map(|a| a.pkg.clone()).collect();
        assert_eq!(r, ["a", "b", "d", "c"], "names that start with c first, then the ones that only contain it");
        let r: Vec<_> = visible_apps(&apps, "  SCAN ", &none, false).iter().map(|a| a.pkg.clone()).collect();
        assert_eq!(r, ["c"]);
        assert!(visible_apps(&apps, "zzz", &none, false).is_empty());
    }

    #[test]
    fn hidden_apps_are_left_out_unless_asked_for() {
        let apps = vec![app("a", "One"), app("b", "Two")];
        let hidden: HashSet<String> = [apps[1].key()].into_iter().collect();
        assert_eq!(visible_apps(&apps, "", &hidden, false).len(), 1);
        assert_eq!(visible_apps(&apps, "", &hidden, true).len(), 2);
    }

    #[test]
    fn lists_round_trip_through_files() {
        let dir = std::env::temp_dir().join(format!("home-test-{}", std::process::id()));
        write_list(&dir, "dock.txt", &["a/x".to_string(), "b/y".to_string()]);
        assert_eq!(read_list(&dir, "dock.txt"), ["a/x", "b/y"]);
        assert!(read_list(&dir, "missing.txt").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_date_reads_well() {
        assert_eq!(date_text(3, 21, 9), "Wed 21 Oct");
        assert_eq!(date_text(0, 1, 0), "Sun 1 Jan");
        let (h, m, d) = clock_now();
        assert!(h < 24 && m < 60 && d.len() >= 8);
    }
}
