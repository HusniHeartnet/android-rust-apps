//! A small photo and video gallery: Photos (everything) and Albums (pinned + folders) modes,
//! full-screen viewer with pinch-zoom and swipe, in-app video player, favourites, and a trash
//! with undo/restore.

mod media;
#[cfg(target_os = "android")]
mod perm;
mod albumops;
mod diag;
mod exif;
mod gridlayout;
mod hidden;
mod intents;
mod settings;
mod space;
mod thumbcache;
mod trash;
mod viewer;

use eframe::egui::{
    self, Color32, ColorImage, Context, Rect, Sense, TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use image::{DynamicImage, ImageDecoder, ImageReader};
use intents::{Pick, PickKind};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const THUMB_PX: u32 = 320;
const FULL_PX: u32 = 4096;
const MAX_THUMB_TEXTURES: usize = 900;
const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];
/// Keys of the built-in albums. They start with a control character so they can never equal a real
/// folder path.
const ALL_KEY: &str = "\u{1}all";
const VIDEOS_KEY: &str = "\u{1}videos";
const FAVS_KEY: &str = "\u{1}favs";
const TRASH_KEY: &str = "\u{1}trash";
const HIDDEN_KEY: &str = "\u{1}hidden";
/// Files opened from other apps that are not in the library live under this pseudo-album.
const EXT_KEY: &str = "\u{1}ext";
/// Trashed items are erased for good after this many days.
const TRASH_DAYS: u64 = 30;
const UNDO_SECS: f64 = 7.0;
const BG: Color32 = Color32::BLACK;
const CARD: Color32 = Color32::from_gray(28);

// ---------- library scan ----------

#[derive(Clone)]
struct Item {
    path: PathBuf,
    mtime: SystemTime,
    size: u64,
    /// Full path of the folder it lives in (the album key), or `TRASH_KEY` / `HIDDEN_KEY`.
    album: String,
    video: bool,
    trashed: bool,
    /// Moved into the hidden vault (shown only after unlocking).
    hidden: bool,
    /// Where a trashed or hidden item came from (for Restore / Unhide).
    orig: Option<PathBuf>,
    /// Permanently deleted this session; kept only so indices stay stable.
    gone: bool,
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
}

fn scan(roots: &[PathBuf], trash_dir: &Path, hidden_dir: &Path) -> Vec<Item> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    stack.push(trash_dir.to_path_buf());
    stack.push(hidden_dir.to_path_buf());
    while let Some(dir) = stack.pop() {
        let is_trash_dir = dir == trash_dir;
        let is_vault = dir == hidden_dir;
        let special = is_trash_dir || is_vault;
        // like Android's media scanner: honour .nomedia
        if !special && dir.join(".nomedia").exists() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let album = dir.to_string_lossy().into_owned();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !special {
                continue; // .thumbnails, .trashed-*, hidden folders
            }
            let path = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                if name == "Android" && roots.contains(&dir) {
                    // Android/data and Android/obb are app-private, but Android/media is where apps
                    // such as WhatsApp keep the photos and videos the user receives
                    let media = path.join("media");
                    if !special && media.is_dir() {
                        stack.push(media);
                    }
                } else if !special && path != trash_dir && path != hidden_dir {
                    stack.push(path);
                }
            } else if special {
                // trashed and hidden files carry their original name inside ours, so check that extension
                let orig = trash::original_of(&path);
                if let Some(o) = orig.as_deref().filter(|o| is_image(o) || media::is_video(o)) {
                    let video = media::is_video(o);
                    out.push(Item {
                        path,
                        mtime: meta.modified().unwrap_or(UNIX_EPOCH),
                        size: meta.len(),
                        album: if is_vault { HIDDEN_KEY } else { TRASH_KEY }.into(),
                        video,
                        trashed: is_trash_dir,
                        hidden: is_vault,
                        orig,
                        gone: false,
                    });
                }
            } else if is_image(&path) || media::is_video(&path) {
                let video = media::is_video(&path);
                out.push(Item {
                    path,
                    mtime: meta.modified().unwrap_or(UNIX_EPOCH),
                    size: meta.len(),
                    album: album.clone(),
                    video,
                    trashed: false,
                    hidden: false,
                    orig: None,
                    gone: false,
                });
            }
        }
    }
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    out
}

// ---------- albums, pins, favourites ----------

#[derive(Clone)]
struct Album {
    key: String,
    label: String,
    count: usize,
    /// Newest item, used as the cover picture.
    cover: Option<usize>,
}

fn key_to_line(key: &str) -> String {
    key.replace('\u{1}', "@")
}

fn line_to_key(line: &str) -> String {
    match line.strip_prefix('@') {
        Some(rest) => format!("\u{1}{rest}"),
        None => line.to_string(),
    }
}

/// What a fresh install pins, like the Xiaomi gallery: everything, camera, screenshots, videos, favourites.
fn default_pins(real: &[Album]) -> Vec<String> {
    let pick = |name: &str| real.iter().filter(|a| a.label == name).max_by_key(|a| a.count).map(|a| a.key.clone());
    let mut pins = vec![ALL_KEY.to_string()];
    pins.extend(pick("Camera"));
    pins.extend(pick("Screenshots"));
    pins.push(VIDEOS_KEY.to_string());
    pins.push(FAVS_KEY.to_string());
    pins
}

const COLUMNS_MIN: usize = 3;
const COLUMNS_MAX: usize = 10;
/// The old gallery shows five pictures per row.
const COLUMNS_DEFAULT: usize = 5;

/// Read `columns=N` from the settings lines, kept inside the allowed range.
fn parse_columns(lines: &[String]) -> Option<usize> {
    lines
        .iter()
        .find_map(|l| l.strip_prefix("columns=")?.trim().parse::<usize>().ok())
        .map(|n| n.clamp(COLUMNS_MIN, COLUMNS_MAX))
}

/// Read a `key=1` / `key=0` switch from the settings lines.
fn parse_flag(lines: &[String], key: &str) -> Option<bool> {
    lines.iter().find_map(|l| l.strip_prefix(key)).map(|v| v.trim() == "1")
}

/// `resume.txt`: one line per video, `milliseconds<TAB>path`.
fn load_resume(dir: &Path) -> HashMap<PathBuf, i32> {
    read_lines(dir, "resume.txt")
        .unwrap_or_default()
        .iter()
        .filter_map(|l| {
            let (ms, path) = l.split_once('\t')?;
            Some((PathBuf::from(path), ms.trim().parse().ok()?))
        })
        .collect()
}

/// Read a whole number such as `skipsecs=10` from the settings lines.
fn parse_int(lines: &[String], key: &str) -> Option<i32> {
    lines.iter().find_map(|l| l.strip_prefix(key)?.trim().parse().ok())
}

/// The lengths offered for the double-tap skip, in seconds.
const SKIP_CHOICES: [i32; 4] = [5, 10, 15, 30];

fn load_columns(dir: &Path) -> usize {
    read_lines(dir, "settings.txt").and_then(|l| parse_columns(&l)).unwrap_or(COLUMNS_DEFAULT)
}

fn data_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        PathBuf::from("/sdcard/Android/data/dev.heartnetapps.gallery/files")
    } else {
        std::env::temp_dir().join("gallery-data")
    }
}

fn read_lines(dir: &Path, name: &str) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(dir.join(name)).ok()?;
    Some(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

fn write_lines(dir: &Path, name: &str, lines: &[String]) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(name), lines.join("\n"));
}

// ---------- background decoding ----------

enum Job {
    Thumb { id: usize, pos: usize, path: PathBuf, video: bool, key: Option<u64> },
    Full { id: usize, path: PathBuf },
}

enum Done {
    Thumb(usize, ColorImage, Option<u64>),
    Full(usize, ColorImage, [u32; 2]),
    /// Dropped because the cell scrolled out of view; may be requested again later.
    Skipped(usize),
    /// Cannot be decoded; do not retry.
    ThumbFailed(usize),
    FullFailed(usize),
}

struct Shared {
    jobs: Mutex<Vec<Job>>, // used as a stack: newest request is decoded first
    cv: Condvar,
    vis_lo: AtomicUsize,
    vis_hi: AtomicUsize,
    /// Photos the viewer wants decoded at full size (current and its neighbours).
    wanted: Mutex<HashSet<usize>>,
    /// Bumped whenever item numbers change (a rescan); results from an older epoch are dropped.
    epoch: AtomicU32,
    cache_dir: PathBuf,
}

fn decode(path: &Path, max: u32, fast: bool) -> Option<(ColorImage, [u32; 2])> {
    // the format is sniffed from the bytes: trashed files have no usable extension
    let mut dec = ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_decoder().ok()?;
    let orientation = dec.orientation().ok();
    let mut img = DynamicImage::from_decoder(dec).ok()?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    let dims = [img.width(), img.height()];
    if img.width().max(img.height()) > max {
        img = if fast {
            img.thumbnail(max, max)
        } else {
            img.resize(max, max, image::imageops::FilterType::Triangle)
        };
    }
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some((ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), dims))
}

fn spawn_workers(shared: Arc<Shared>, tx: Sender<(u32, Done)>, ctx: Context) {
    let n = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
    for _ in 0..n {
        let (shared, tx, ctx) = (shared.clone(), tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            loop {
                let (job, epoch) = {
                    let mut q = shared.jobs.lock().unwrap();
                    while q.is_empty() {
                        q = shared.cv.wait(q).unwrap();
                    }
                    (q.pop().unwrap(), shared.epoch.load(Ordering::SeqCst))
                };
                let done = match job {
                    Job::Thumb { id, pos, path, video, key } => {
                        let (lo, hi) = (shared.vis_lo.load(Ordering::Relaxed), shared.vis_hi.load(Ordering::Relaxed));
                        if pos + 24 < lo || pos > hi.saturating_add(24) {
                            Done::Skipped(id) // scrolled away while queued
                        } else if let Some((img, duration)) = key.and_then(|k| thumbcache::load(&shared.cache_dir, k)) {
                            Done::Thumb(id, img, duration)
                        } else if video {
                            match media::thumbnail(&path, THUMB_PX) {
                                Some(t) => {
                                    if let Some(k) = key {
                                        thumbcache::store(&shared.cache_dir, k, &t.image, Some(t.duration_ms));
                                    }
                                    Done::Thumb(id, t.image, Some(t.duration_ms))
                                }
                                None => Done::ThumbFailed(id),
                            }
                        } else {
                            match decode(&path, THUMB_PX, true) {
                                Some((img, _)) => {
                                    if let Some(k) = key {
                                        thumbcache::store(&shared.cache_dir, k, &img, None);
                                    }
                                    Done::Thumb(id, img, None)
                                }
                                None => Done::ThumbFailed(id),
                            }
                        }
                    }
                    Job::Full { id, path } => {
                        if !shared.wanted.lock().unwrap().contains(&id) {
                            continue; // user already moved on
                        }
                        match decode(&path, FULL_PX, false) {
                            Some((img, dims)) => Done::Full(id, img, dims),
                            None => Done::FullFailed(id),
                        }
                    }
                };
                let _ = tx.send((epoch, done));
                ctx.request_repaint();
            }
        });
    }
}

// ---------- app ----------

struct Viewer {
    pos: usize, // position in the filtered view
    id: usize,
    /// The video could not be opened.
    failed: bool,
    zoom: f32,
    offset: Vec2,
    /// Horizontal drag offset of the current page, in points (the neighbours follow it).
    swipe: f32,
    /// Settle animation after letting go: (from, to, start time).
    anim: Option<(f32, f32, f64)>,
    /// Slider position while the user is dragging the video seek bar.
    scrub: Option<f32>,
    /// Whether the video was playing when the finger went down on the seek bar (it resumes on release).
    scrub_was_playing: bool,
    /// Egui time of the last seek issued while dragging (they are rate-limited).
    scrub_last_seek: f64,
    /// What a drag on a video is doing: 0 not decided yet, 1 swiping between items, 2 changing
    /// brightness (left half) or volume (right half).
    gesture: u8,
    drag_origin: Option<egui::Pos2>,
    /// Double-tap skipping: seconds skipped so far in this burst of taps (negative = back), when
    /// the last one happened, which side (-1 left, 1 right), and where the video was when it began.
    skip_acc: i32,
    /// The finger that is down right now (when, where), and the last tap in an edge strip (when, which side).
    press: Option<(f64, egui::Pos2)>,
    last_tap: Option<(f64, i8)>,
    skip_last: f64,
    skip_side: i8,
    skip_base: i32,
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Close,
    Trash,
    Restore,
    DeleteForever,
    ToggleFavourite,
    Float,
    Hide,
    Unhide,
    Wallpaper,
    MoveTo,
    CopyTo,
}

/// What the album chooser does with the chosen items.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum AlbumOp {
    Move,
    Copy,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Photos,
    Albums,
}

pub struct Gallery {
    items: Vec<Item>,
    real_albums: Vec<Album>,
    virt_albums: Vec<Album>,
    pins: Vec<String>,
    pins_loaded: bool,
    favs: HashSet<PathBuf>,
    tab: Tab,
    /// Showing one album's photos (Albums mode).
    album_open: bool,
    album_menu: Option<String>,
    filter: Option<String>,
    view: Vec<usize>,
    /// The day (yyyymmdd, local time) of each item in `view`, for the date headings.
    view_days: Vec<i32>,
    /// Scroll bookkeeping for the fast-scroll handle.
    grid_last_offset: f32,
    grid_last_move: f64,
    thumbs: HashMap<usize, TextureHandle>,
    durations: HashMap<usize, u64>,
    failed: HashSet<usize>,
    requested: HashSet<usize>,
    shared: Arc<Shared>,
    done_rx: Receiver<(u32, Done)>,
    scan_rx: Receiver<Vec<Item>>,
    scanned: bool,
    roots: Vec<PathBuf>,
    trash_dir: PathBuf,
    /// The hidden vault: a dot-folder with a .nomedia file, so no gallery or media picker lists it.
    hidden_dir: PathBuf,
    data_dir: PathBuf,
    /// The album chooser for a move or copy, and a question about one album (rename / delete).
    album_picker: Option<albumops::AlbumPicker>,
    album_dialog: Option<albumops::AlbumDialog>,
    /// The "Free up space" page.
    space: Option<space::Space>,
    /// The More menu above the selection bar is open.
    select_menu: bool,
    /// The vault has been unlocked (fingerprint / screen lock) and is showing.
    hidden_unlocked: bool,
    /// The system lock prompt is up.
    auth_pending: bool,
    /// Show a "Hidden" card in the Albums list (otherwise it is opened from Settings).
    show_hidden_tile: bool,
    /// Another app asked for a photo or video: choosing one answers it.
    pick: Option<Pick>,
    /// The viewer was opened by another app, so leaving it goes back there.
    external_viewer: bool,
    last_intent_poll: Instant,
    last_scan: Instant,
    /// A scan that came back empty or far too small is ignored and tried again: how many times so far, and when.
    scan_retries: u8,
    rescan_at: Option<Instant>,
    started: Instant,
    first_frame_logged: bool,
    frames_since_focus: u32,
    last_screen: (u32, u32),
    was_focused: bool,
    left_at: Option<Instant>,
    viewer: Option<Viewer>,
    /// The playing video and the item it belongs to.
    player: Option<(usize, media::Player)>,
    /// A video that is still opening in the background.
    pending_player: Option<(usize, Receiver<Option<media::Player>>)>,
    /// Full-size textures for the open photo and its neighbours.
    full: HashMap<usize, (TextureHandle, [u32; 2])>,
    full_requested: HashSet<usize>,
    full_failed: HashSet<usize>,
    ctx: Context,
    /// Space (top, bottom) covered by the status and navigation bars, in points.
    insets: (f32, f32),
    last_rate_request: Option<Instant>,
    /// `refresh=request` in settings.txt: ask the system for the fastest refresh rate.
    request_refresh: bool,
    /// Pictures per row in the grids; a saved setting (COLUMNS_MIN..=COLUMNS_MAX).
    cols: usize,
    /// Start a video again when it ends (`loop=1` in settings.txt).
    loop_video: bool,
    /// Videos start playing as soon as they open (`autoplay=0` turns it off).
    autoplay: bool,
    /// A tap on the picture hides or shows the bars (`taphide=0` keeps them always on).
    tap_hide: bool,
    /// A small name-and-date strip on photos (`strip=1`).
    details_strip: bool,
    /// Double-tap the left or right side of a video to skip back or forward (`dtskip=0` turns it off).
    double_tap_skip: bool,
    skip_secs: i32,
    /// The settings page is showing; `settings_return` is the viewer position to come back to.
    settings_open: bool,
    settings_return: Option<usize>,
    /// Speed, mute and screen orientation of the video player (kept while the app runs).
    video_speed: f32,
    video_muted: bool,
    landscape: bool,
    /// Where each long video was left: path to milliseconds (`resume.txt`).
    resume: HashMap<PathBuf, i32>,
    /// Brightness and volume as last set by a swipe, and the little readout shown for it.
    brightness: Option<f32>,
    volume: Option<f32>,
    hud: Option<(String, f32, f64)>,
    /// EXIF of the photo whose Details are open: (item, what was read).
    details_exif: Option<(usize, Option<exif::Exif>)>,
    /// The viewer's top and bottom bars are showing.
    bars_visible: bool,
    viewer_menu: bool,
    details_open: bool,
    /// Multi-select mode: the chosen items. `None` when not selecting.
    select: Option<HashSet<usize>>,
    /// Painting a selection with the finger: (adding or removing, last finger position).
    paint: Option<(bool, Option<egui::Pos2>)>,
    /// Ignore the tap that ends a long-press or a paint swipe.
    swallow_click: bool,
    /// Scroll position to jump to on the next frame (edge auto-scroll while painting).
    grid_scroll_to: Option<f32>,
    /// Top edge of the selection action bar, so swipes that start on it are not read as painting.
    select_bar_top: f32,
    /// The number of items the next "delete forever" tap would delete, once armed.
    confirm_batch_forever: Option<usize>,
    /// Running scale of the current pinch on the grid.
    pinch: f32,
    // permissions (always "granted" off Android)
    media_ok: bool,
    video_ok: bool,
    asked_video: bool,
    manage_ok: bool,
    manage_prompt: bool,
    last_perm_check: Option<Instant>,
    // transient UI state
    toast: Option<(String, Vec<usize>, f64)>, // message, items an Undo would restore, deadline (egui time)
    confirm_forever: Option<usize>,
    confirm_empty: bool,
    /// Desktop preview only: GALLERY_START=albums|photos|album:<label>|viewer
    start: Option<String>,
    #[cfg(target_os = "android")]
    android: Option<android_activity::AndroidApp>,
}

/// The window loop's context, so the destroy hook (called from Java on the main thread) can end it.
#[cfg(target_os = "android")]
static CTX: Mutex<Option<Context>> = Mutex::new(None);

/// Called by `GalleryActivity.onDestroy` just before Android waits for the app thread to stop.
/// The windowing library ignores Android's destroy notice, so ask the window to close here:
/// that ends the event loop, `android_main` returns, and Android can finish destroying the activity.
#[cfg(target_os = "android")]
extern "system" fn native_destroying(_env: jni::JNIEnv, _class: jni::objects::JClass) {
    diag::log("activity is being destroyed: closing the window");
    if let Some(ctx) = CTX.lock().ok().and_then(|g| g.clone()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }
}

/// Used when Android reports no system-bar insets but the window is drawn edge to edge.
#[cfg(target_os = "android")]
const FALLBACK_INSETS: (f32, f32) = (30.0, 36.0);

impl Gallery {
    #[cfg(target_os = "android")]
    pub fn with_android(mut self, app: android_activity::AndroidApp) -> Self {
        *CTX.lock().unwrap() = Some(self.ctx.clone());
        let hooked = perm::register_destroy_hook(&app, native_destroying);
        diag::log(&format!("destroy hook registered: {hooked}"));
        media::init(&app);
        self.media_ok = perm::has_permission(&app, perm::MEDIA_IMAGES);
        self.video_ok = perm::has_permission(&app, perm::MEDIA_VIDEO);
        self.manage_ok = perm::has_all_files_access(&app);
        self.android = Some(app);
        self
    }

    /// (top, bottom) space in points that the status and navigation bars cover.
    fn insets(&self, _ctx: &Context) -> (f32, f32) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android {
            let r = app.content_rect();
            let ppp = _ctx.pixels_per_point();
            let win_h = _ctx.viewport_rect().height() * ppp;
            let top = r.top.max(0) as f32 / ppp;
            let bottom = (win_h - r.bottom as f32).max(0.0) / ppp;
            return if top + bottom > 0.5 { (top, bottom) } else { FALLBACK_INSETS };
        }
        (0.0, 0.0)
    }

    pub fn new(cc: &eframe::CreationContext<'_>, roots: Vec<PathBuf>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(Color32::from_gray(235));
        cc.egui_ctx.set_visuals(visuals);
        let shared = Arc::new(Shared {
            jobs: Mutex::new(Vec::new()),
            cv: Condvar::new(),
            vis_lo: AtomicUsize::new(0),
            vis_hi: AtomicUsize::new(0),
            wanted: Mutex::new(HashSet::new()),
            epoch: AtomicU32::new(0),
            cache_dir: data_dir().join("thumbs"),
        });
        let (done_tx, done_rx) = channel();
        spawn_workers(shared.clone(), done_tx, cc.egui_ctx.clone());

        let roots = if roots.is_empty() { default_roots() } else { roots };
        let trash_dir = trash::default_trash_dir();
        let hidden_dir = trash::default_hidden_dir();
        let data_dir = data_dir();
        let scan_rx = start_scan(roots.clone(), trash_dir.clone(), hidden_dir.clone(), cc.egui_ctx.clone());
        let favs = read_lines(&data_dir, "favourites.txt").unwrap_or_default().into_iter().map(PathBuf::from).collect();
        let cols = load_columns(&data_dir);
        let settings = read_lines(&data_dir, "settings.txt").unwrap_or_default();
        let request_refresh = settings.iter().any(|s| s == "refresh=request");
        let loop_video = parse_flag(&settings, "loop=").unwrap_or(false);
        let autoplay = parse_flag(&settings, "autoplay=").unwrap_or(true);
        let tap_hide = parse_flag(&settings, "taphide=").unwrap_or(true);
        let details_strip = parse_flag(&settings, "strip=").unwrap_or(false);
        diag::init(data_dir.clone());
        diag::log(&format!("---- start, version {}", env!("CARGO_PKG_VERSION")));
        let resume = load_resume(&data_dir);
        let double_tap_skip = parse_flag(&settings, "dtskip=").unwrap_or(true);
        let skip_secs = parse_int(&settings, "skipsecs=").filter(|s| SKIP_CHOICES.contains(s)).unwrap_or(10);
        let show_hidden_tile = parse_flag(&settings, "hiddentile=").unwrap_or(true);
        Self {
            items: Vec::new(),
            real_albums: Vec::new(),
            virt_albums: Vec::new(),
            pins: Vec::new(),
            pins_loaded: false,
            favs,
            tab: Tab::Albums,
            album_open: false,
            album_menu: None,
            filter: None,
            view: Vec::new(),
            view_days: Vec::new(),
            grid_last_offset: 0.0,
            grid_last_move: f64::MIN,
            thumbs: HashMap::new(),
            durations: HashMap::new(),
            failed: HashSet::new(),
            requested: HashSet::new(),
            shared,
            done_rx,
            scan_rx,
            scanned: false,
            roots,
            trash_dir,
            hidden_dir,
            data_dir,
            album_picker: None,
            album_dialog: None,
            space: None,
            select_menu: false,
            hidden_unlocked: false,
            auth_pending: false,
            show_hidden_tile,
            pick: None,
            external_viewer: false,
            last_intent_poll: Instant::now(),
            last_scan: Instant::now(),
            scan_retries: 0,
            rescan_at: None,
            started: Instant::now(),
            first_frame_logged: false,
            frames_since_focus: 0,
            last_screen: (0, 0),
            was_focused: true,
            left_at: None,
            viewer: None,
            player: None,
            pending_player: None,
            full: HashMap::new(),
            full_requested: HashSet::new(),
            full_failed: HashSet::new(),
            ctx: cc.egui_ctx.clone(),
            insets: (0.0, 0.0),
            last_rate_request: None,
            request_refresh,
            cols,
            loop_video,
            autoplay,
            tap_hide,
            details_strip,
            double_tap_skip,
            skip_secs,
            settings_open: false,
            settings_return: None,
            video_speed: 1.0,
            video_muted: false,
            landscape: false,
            resume,
            brightness: None,
            volume: None,
            hud: None,
            details_exif: None,
            bars_visible: true,
            viewer_menu: false,
            details_open: false,
            select: None,
            paint: None,
            swallow_click: false,
            grid_scroll_to: None,
            select_bar_top: f32::MAX,
            confirm_batch_forever: None,
            pinch: 1.0,
            media_ok: true,
            video_ok: true,
            asked_video: false,
            manage_ok: true,
            manage_prompt: false,
            last_perm_check: None,
            toast: None,
            confirm_forever: None,
            confirm_empty: false,
            start: std::env::var("GALLERY_START").ok(),
            #[cfg(target_os = "android")]
            android: None,
        }
    }

    /// When the app comes back to the front, look for new photos; leaving the vault open while
    /// the app was away for a minute is not safe, so lock it.
    fn track_focus(&mut self, ctx: &Context) {
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused != self.was_focused {
            diag::log(if focused { "window focused" } else { "window lost focus" });
            self.frames_since_focus = 0;
            ctx.request_repaint();
        }
        if focused && !self.was_focused {
            self.last_intent_poll = Instant::now() - Duration::from_secs(1);
            if self.scanned && self.last_scan.elapsed() > Duration::from_secs(3) {
                self.rescan(ctx);
            }
            if self.left_at.take().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
                self.lock_hidden(ctx);
            }
        } else if !focused && self.was_focused {
            self.left_at = Some(Instant::now());
        }
        self.was_focused = focused;
    }

    fn in_trash_view(&self) -> bool {
        self.filter.as_deref() == Some(TRASH_KEY)
    }

    fn is_fav(&self, id: usize) -> bool {
        self.favs.contains(&self.items[id].path)
    }

    fn album(&self, key: &str) -> Option<Album> {
        self.virt_albums.iter().chain(self.real_albums.iter()).find(|a| a.key == key).cloned()
    }

    fn rebuild_view(&mut self) {
        let filter = self.filter.clone();
        let unlocked = self.hidden_unlocked;
        let kind = self.pick.map(|p| p.kind);
        self.view = (0..self.items.len())
            .filter(|&i| {
                let it = &self.items[i];
                if it.gone {
                    return false;
                }
                // an item in the library proper: not trashed, not in the vault, not a one-off from another app
                let kind_ok = match kind {
                    Some(PickKind::Image) => !it.video,
                    Some(PickKind::Video) => it.video,
                    _ => true,
                };
                let normal = !it.trashed && !it.hidden && it.album != EXT_KEY && kind_ok;
                match filter.as_deref() {
                    Some(TRASH_KEY) => it.trashed,
                    Some(HIDDEN_KEY) => it.hidden && unlocked,
                    Some(EXT_KEY) => it.album == EXT_KEY,
                    Some(VIDEOS_KEY) => normal && it.video,
                    Some(FAVS_KEY) => normal && self.favs.contains(&it.path),
                    Some(ALL_KEY) | None => normal,
                    Some(dir) => normal && it.album == dir,
                }
            })
            .collect();
        self.view_days = self.view.iter().map(|&i| local_day(self.items[i].mtime)).collect();

        // Albums: items are sorted newest first, so the first item seen per folder is its cover.
        let mut order: Vec<String> = Vec::new();
        let mut by_dir: HashMap<&str, (usize, usize)> = HashMap::new();
        let (mut all, mut videos, mut favs, mut trash, mut hidden) = ((0, None), (0, None), (0, None), (0, None), (0, None));
        let bump = |slot: &mut (usize, Option<usize>), i: usize| {
            slot.0 += 1;
            slot.1.get_or_insert(i);
        };
        for (i, it) in self.items.iter().enumerate().filter(|(_, it)| !it.gone) {
            if it.trashed {
                bump(&mut trash, i);
                continue;
            }
            if it.hidden {
                bump(&mut hidden, i);
                continue;
            }
            if it.album == EXT_KEY {
                continue;
            }
            bump(&mut all, i);
            if it.video {
                bump(&mut videos, i);
            }
            if self.favs.contains(&it.path) {
                bump(&mut favs, i);
            }
            let e = by_dir.entry(it.album.as_str()).or_insert_with(|| {
                order.push(it.album.clone());
                (0, i)
            });
            e.0 += 1;
        }
        let name_of = |key: &str| Path::new(key).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| key.to_string());
        let mut name_count: HashMap<String, usize> = HashMap::new();
        for k in &order {
            *name_count.entry(name_of(k)).or_default() += 1;
        }
        self.real_albums = order
            .iter()
            .map(|k| {
                let (count, cover) = by_dir[k.as_str()];
                let name = name_of(k);
                let label = if name_count[&name] > 1 {
                    let parent = Path::new(k).parent().map(|p| name_of(&p.to_string_lossy())).unwrap_or_default();
                    format!("{name} · {parent}")
                } else {
                    name
                };
                Album { key: k.clone(), label, count, cover: Some(cover) }
            })
            .collect();
        let mk = |key: &str, label: &str, (count, cover): (usize, Option<usize>)| Album { key: key.into(), label: label.into(), count, cover };
        self.virt_albums = vec![
            mk(ALL_KEY, "All photos", all),
            mk(VIDEOS_KEY, "Videos", videos),
            mk(FAVS_KEY, "Favourites", favs),
            mk(TRASH_KEY, "Trash", trash),
            // no cover: nothing from the vault may be drawn outside the vault
            mk(HIDDEN_KEY, "Hidden", (hidden.0, None)),
        ];

        if self.scanned && !self.pins_loaded {
            self.pins_loaded = true;
            self.pins = match read_lines(&self.data_dir, "pins.txt") {
                Some(lines) => lines.iter().map(|l| line_to_key(l)).collect(),
                None => {
                    let pins = default_pins(&self.real_albums);
                    self.save_pins(&pins);
                    pins
                }
            };
        }
    }

    fn save_pins(&self, pins: &[String]) {
        let lines: Vec<String> = pins.iter().map(|k| key_to_line(k)).collect();
        write_lines(&self.data_dir, "pins.txt", &lines);
    }

    fn save_favs(&self) {
        let mut lines: Vec<String> = self.favs.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        lines.sort();
        write_lines(&self.data_dir, "favourites.txt", &lines);
    }

    fn toggle_pin(&mut self, key: &str) {
        if let Some(i) = self.pins.iter().position(|k| k == key) {
            self.pins.remove(i);
        } else {
            self.pins.push(key.to_string());
        }
        let pins = self.pins.clone();
        self.save_pins(&pins);
    }

    fn toggle_favourite(&mut self, id: usize) {
        let path = self.items[id].path.clone();
        if !self.favs.remove(&path) {
            self.favs.insert(path);
        }
        self.save_favs();
        self.rebuild_view();
    }

    fn open_album(&mut self, key: &str) {
        self.tab = Tab::Albums;
        self.album_open = true;
        self.filter = Some(key.to_string());
        self.rebuild_view();
    }

    fn go_to_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.album_open = false;
        self.filter = None;
        self.close_viewer();
        self.rebuild_view();
    }

    /// Look at the storage again in the background. What is on screen stays put: when the new
    /// list arrives, everything open is re-pointed at the same files (see `apply_scan`).
    fn rescan(&mut self, ctx: &Context) {
        self.last_scan = Instant::now();
        self.scan_rx = start_scan(self.roots.clone(), self.trash_dir.clone(), self.hidden_dir.clone(), ctx.clone());
    }

    /// Change how many pictures fit in a row, and remember it.
    fn set_columns(&mut self, n: usize) {
        let n = n.clamp(COLUMNS_MIN, COLUMNS_MAX);
        if n != self.cols {
            self.cols = n;
            self.save_settings();
        }
    }

    fn save_settings(&self) {
        let mut lines = vec![
            format!("columns={}", self.cols),
            format!("loop={}", u8::from(self.loop_video)),
            format!("autoplay={}", u8::from(self.autoplay)),
            format!("taphide={}", u8::from(self.tap_hide)),
            format!("strip={}", u8::from(self.details_strip)),
            format!("hiddentile={}", u8::from(self.show_hidden_tile)),
            format!("dtskip={}", u8::from(self.double_tap_skip)),
            format!("skipsecs={}", self.skip_secs),
        ];
        if self.request_refresh {
            lines.push("refresh=request".to_string());
        }
        write_lines(&self.data_dir, "settings.txt", &lines);
    }

    /// Ask Android for the display's fastest refresh rate. Apps that draw through their own GL
    /// surface (like this one) are otherwise held at 60 Hz even when the screen is set to 120 Hz.
    /// Repeated every couple of seconds so it also covers a recreated window.
    ///
    /// Off by default: on HyperOS an app that makes these requests is moved to "Based on app
    /// settings" in Settings > High refresh rate, which has no switch, and the system still holds
    /// it at 60 Hz. Without the requests the app gets a switch there instead. Opt in by adding the
    /// line `refresh=request` to settings.txt.
    fn request_high_refresh_rate(&mut self) {
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.as_ref().filter(|_| self.request_refresh) {
            if self.last_rate_request.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
                return;
            }
            self.last_rate_request = Some(Instant::now());
            if let Some(window) = app.native_window() {
                let _ = window.set_frame_rate(120.0, ndk::native_window::FrameRateCompatibility::Default);
            }
            // HyperOS's "based on app settings" mode follows the window-level request
            perm::request_window_refresh(app);
        }
    }

    /// Quit. On Android this closes the window loop while the activity is still on screen; when
    /// `android_main` returns, the glue finishes the activity by itself. Asking Android to finish
    /// first does not work: the activity is paused by the time it is destroyed, the paused window
    /// loop never sees the close request, and the process is left stuck.
    fn exit(&self, ctx: &Context) {
        diag::log("quitting");
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }

    /// Re-read permission state about once a second (the user can change it in a system screen).
    fn refresh_permissions(&mut self, _ctx: &Context) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android {
            if self.last_perm_check.is_some_and(|t| t.elapsed() < Duration::from_millis(800)) {
                return;
            }
            self.last_perm_check = Some(Instant::now());
            let media = perm::has_permission(app, perm::MEDIA_IMAGES);
            self.video_ok = perm::has_permission(app, perm::MEDIA_VIDEO);
            self.manage_ok = perm::has_all_files_access(app);
            let newly_granted = media && !self.media_ok;
            self.media_ok = media;
            // photo access was granted by an earlier version: ask for video access once as well
            if media && !self.video_ok && !self.asked_video {
                self.asked_video = true;
                perm::request_permissions(app, &[perm::MEDIA_VIDEO]);
            }
            if newly_granted {
                self.rescan(_ctx);
            }
            if !media || (self.manage_prompt && !self.manage_ok) {
                _ctx.request_repaint_after(Duration::from_millis(800));
            }
        }
    }

    /// Take a fresh scan result. Items are numbered by position, so every number kept anywhere
    /// (thumbnails, the open picture, the selection) is translated to the same file's new number.
    fn apply_scan(&mut self, ctx: &Context, mut items: Vec<Item>) {
        let (old_live, new_live) = (self.items.iter().filter(|i| !i.gone && i.album != EXT_KEY).count(), items.iter().filter(|i| !i.gone).count());
        diag::log(&format!("scan result: {new_live} items (had {old_live}), took {} ms", self.last_scan.elapsed().as_millis()));
        // A scan that finds nothing (or far less than before) usually means storage was briefly
        // unreadable, not that the library is gone. Keep what is on screen and look again shortly.
        let suspicious = old_live > 0 && (new_live == 0 || (old_live >= 40 && new_live * 2 < old_live));
        if suspicious && self.scan_retries < 3 {
            self.scan_retries += 1;
            diag::log(&format!("scan ignored (retry {})", self.scan_retries));
            self.rescan_at = Some(Instant::now() + Duration::from_millis(2000));
            return;
        }
        self.scan_retries = 0;
        // files opened from other apps stay available across rescans
        for e in self.items.iter().filter(|i| i.album == EXT_KEY && !i.gone) {
            if !items.iter().any(|i| i.path == e.path) {
                items.push(e.clone());
            }
        }
        // trash empties itself after TRASH_DAYS
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        for it in items.iter_mut().filter(|i| i.trashed) {
            let old = trash::trashed_at(&it.path).is_some_and(|t| now.saturating_sub(t) > TRASH_DAYS * 86_400);
            if old && trash::delete_forever(&it.path, &self.trash_dir).is_ok() {
                it.gone = true;
            }
        }
        // anything still queued, or being decoded, belongs to the old numbering
        {
            let mut q = self.shared.jobs.lock().unwrap();
            q.clear();
            self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        }
        let old = std::mem::replace(&mut self.items, items);
        let new_ids: HashMap<PathBuf, usize> = self.items.iter().enumerate().filter(|(_, i)| !i.gone).map(|(n, i)| (i.path.clone(), n)).collect();
        let remap: Vec<Option<usize>> = old.iter().map(|it| if it.gone { None } else { new_ids.get(&it.path).copied() }).collect();
        drop(old);
        drop(new_ids);

        self.thumbs = remap_map(std::mem::take(&mut self.thumbs), &remap);
        self.durations = remap_map(std::mem::take(&mut self.durations), &remap);
        self.full = remap_map(std::mem::take(&mut self.full), &remap);
        self.failed = remap_set(std::mem::take(&mut self.failed), &remap);
        self.requested.clear();
        self.full_requested.clear();
        self.full_failed.clear();
        self.select = self.select.take().map(|s| remap_set(s, &remap));
        self.paint = None;
        self.confirm_forever = None;
        self.pending_player = None;
        self.player = self.player.take().and_then(|(pid, p)| remap.get(pid).copied().flatten().map(|n| (n, p)));
        let viewer_id = self.viewer.as_ref().map(|v| remap.get(v.id).copied().flatten());

        self.scanned = true;
        self.rebuild_view();
        let still_there = |f: &str| matches!(f, ALL_KEY | VIDEOS_KEY | FAVS_KEY | TRASH_KEY | HIDDEN_KEY | EXT_KEY) || self.real_albums.iter().any(|a| a.key == f);
        if self.filter.as_deref().is_some_and(|f| !still_there(f)) {
            self.filter = None;
            self.album_open = false;
            self.rebuild_view();
        }

        // a file opened from another app may have been found in the library by this scan
        if let (Some(Some(nid)), Some(EXT_KEY)) = (viewer_id, self.filter.as_deref()) {
            if self.items[nid].album != EXT_KEY {
                self.filter = None;
                self.rebuild_view();
            }
        }
        match viewer_id {
            Some(Some(nid)) => match self.view.iter().position(|&i| i == nid) {
                Some(pos) => {
                    if let Some(v) = self.viewer.as_mut() {
                        v.id = nid;
                        v.pos = pos;
                    }
                    self.prefetch(pos);
                }
                None => self.close_viewer(),
            },
            Some(None) => self.close_viewer(),
            None => self.shared.wanted.lock().unwrap().clear(),
        }

        let keep: HashSet<u64> = self.items.iter().filter(|i| !i.hidden && !i.gone).map(|i| thumbcache::key(&i.path, i.mtime, i.size)).collect();
        thumbcache::prune(&self.shared.cache_dir, &keep);
        self.apply_start();
        ctx.request_repaint();
    }

    fn drain(&mut self, ctx: &Context) {
        if let Ok(items) = self.scan_rx.try_recv() {
            self.apply_scan(ctx, items);
        }
        while let Ok((epoch, done)) = self.done_rx.try_recv() {
            if epoch != self.shared.epoch.load(Ordering::SeqCst) {
                continue; // decoded for a numbering that no longer exists
            }
            match done {
                Done::Thumb(id, img, duration) => {
                    self.requested.remove(&id);
                    let tex = ctx.load_texture(format!("t{id}"), img, TextureOptions::LINEAR);
                    self.thumbs.insert(id, tex);
                    if let Some(d) = duration {
                        self.durations.insert(id, d);
                    }
                }
                Done::Skipped(id) => {
                    self.requested.remove(&id);
                }
                Done::ThumbFailed(id) => {
                    self.requested.remove(&id);
                    self.failed.insert(id);
                }
                Done::Full(id, img, dims) => {
                    self.full_requested.remove(&id);
                    // only keep it if the viewer still wants it (the user may have swiped far away)
                    if self.shared.wanted.lock().unwrap().contains(&id) {
                        let tex = ctx.load_texture(format!("f{id}"), img, TextureOptions::LINEAR);
                        self.full.insert(id, (tex, dims));
                    }
                }
                Done::FullFailed(id) => {
                    self.full_requested.remove(&id);
                    self.full_failed.insert(id);
                }
            }
        }
    }

    /// Desktop preview helper: jump straight to a screen so it can be screenshotted.
    fn apply_start(&mut self) {
        let Some(start) = self.start.take() else { return };
        match start.as_str() {
            "photos" => self.go_to_tab(Tab::Photos),
            "albums" => self.go_to_tab(Tab::Albums),
            "settings" => self.open_settings(),
            "space" => {
                let ctx = self.ctx.clone();
                self.open_space(&ctx);
            }
            "hide" => {
                // desktop test of the vault: hide three photos, unlock, and show the vault
                self.go_to_tab(Tab::Photos);
                let ids: Vec<usize> = self.view.iter().copied().skip(6).take(3).collect();
                let ctx = self.ctx.clone();
                self.hide_items(&ctx, &ids);
                self.request_unlock(&ctx);
            }
            "select" => {
                self.go_to_tab(Tab::Photos);
                self.select = Some(self.view.iter().copied().skip(2).take(7).collect());
            }
            "viewer" => {
                self.go_to_tab(Tab::Photos);
                if let Some(pos) = self.view.iter().position(|&i| !self.items[i].video) {
                    self.open_viewer(pos);
                }
            }
            s => {
                let find = |label: &str| self.virt_albums.iter().chain(self.real_albums.iter()).find(|a| a.label == label).cloned();
                if let Some(a) = s.strip_prefix("album:").and_then(find) {
                    self.open_album(&a.key);
                } else if let Some(a) = s.strip_prefix("menu:").and_then(find) {
                    self.go_to_tab(Tab::Albums);
                    self.album_menu = Some(a.key);
                }
            }
        }
    }

    fn request_thumb(&mut self, id: usize, pos: usize) {
        if self.thumbs.contains_key(&id) || self.failed.contains(&id) || !self.requested.insert(id) {
            return;
        }
        let it = &self.items[id];
        // nothing from the vault is ever written to the thumbnail cache
        let key = (!it.hidden).then(|| thumbcache::key(&it.path, it.mtime, it.size));
        let job = Job::Thumb { id, pos, path: it.path.clone(), video: it.video, key };
        self.shared.jobs.lock().unwrap().push(job);
        self.shared.cv.notify_one();
    }

    fn trim_textures(&mut self) {
        if self.thumbs.len() <= MAX_THUMB_TEXTURES {
            return;
        }
        let (lo, hi) = (self.shared.vis_lo.load(Ordering::Relaxed), self.shared.vis_hi.load(Ordering::Relaxed));
        let keep: HashSet<usize> = self
            .view
            .iter()
            .enumerate()
            .filter(|(p, _)| *p + 150 >= lo && *p <= hi.saturating_add(150))
            .map(|(_, &id)| id)
            .collect();
        let covers: HashSet<usize> = self.virt_albums.iter().chain(self.real_albums.iter()).filter_map(|a| a.cover).collect();
        self.thumbs.retain(|id, _| keep.contains(id) || covers.contains(id));
    }

    // ----- trash actions -----

    fn say(&mut self, ctx: &Context, msg: impl Into<String>, undo: Option<usize>) {
        self.say_many(ctx, msg, undo.into_iter().collect());
    }

    fn say_many(&mut self, ctx: &Context, msg: impl Into<String>, undo: Vec<usize>) {
        let deadline = ctx.input(|i| i.time) + UNDO_SECS;
        self.toast = Some((msg.into(), undo, deadline));
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    // ----- batch actions (multi-select) -----

    /// The chosen items that are in the current view, in the order shown.
    fn selected_ids(&self) -> Vec<usize> {
        match &self.select {
            Some(sel) => self.view.iter().copied().filter(|id| sel.contains(id)).collect(),
            None => Vec::new(),
        }
    }

    fn exit_select(&mut self) {
        self.select_menu = false;
        self.select = None;
        self.paint = None;
        self.confirm_batch_forever = None;
    }

    fn trash_items(&mut self, ctx: &Context, ids: &[usize]) -> usize {
        if !self.manage_ok {
            self.manage_prompt = true;
            return 0;
        }
        self.player = None;
        let (mut done, mut failed) = (Vec::new(), 0);
        let mut moved_paths: Vec<String> = Vec::new();
        for &id in ids {
            if self.items[id].trashed || self.items[id].gone || self.items[id].hidden {
                continue;
            }
            match trash::move_to_trash(&self.items[id].path, &self.trash_dir) {
                Ok(dest) => {
                    let it = &mut self.items[id];
                    it.orig = Some(it.path.clone());
                    it.path = dest;
                    it.trashed = true;
                    it.album = TRASH_KEY.into();
                    moved_paths.push(it.orig.clone().unwrap_or_default().to_string_lossy().into_owned());
                    done.push(id);
                }
                Err(_) => failed += 1,
            }
        }
        self.rebuild_view();
        self.notify_media(&moved_paths);
        let n = done.len();
        let msg = if failed == 0 { format!("Moved {n} to trash") } else { format!("Moved {n} to trash, {failed} failed") };
        self.say_many(ctx, msg, done);
        n
    }

    fn restore_items(&mut self, ctx: &Context, ids: &[usize]) -> usize {
        let mut n = 0;
        let mut restored: Vec<String> = Vec::new();
        for &id in ids {
            if !self.items[id].trashed || self.items[id].gone {
                continue;
            }
            if let Ok(dest) = trash::restore(&self.items[id].path, &self.trash_dir) {
                let it = &mut self.items[id];
                it.album = dest.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                restored.push(dest.to_string_lossy().into_owned());
                it.path = dest;
                it.trashed = false;
                it.orig = None;
                n += 1;
            }
        }
        self.notify_media(&restored);
        self.rebuild_view();
        self.say(ctx, if n == 1 { "Restored".to_string() } else { format!("Restored {n}") }, None);
        n
    }

    fn delete_forever_items(&mut self, ctx: &Context, ids: &[usize]) -> usize {
        self.player = None;
        let mut n = 0;
        for &id in ids {
            if self.items[id].gone {
                continue;
            }
            let dir = self.vault_of(id);
            if trash::delete_forever(&self.items[id].path, &dir).is_ok() {
                self.items[id].gone = true;
                self.thumbs.remove(&id);
                n += 1;
            }
        }
        self.rebuild_view();
        self.say(ctx, format!("Deleted {n} permanently"), None);
        n
    }

    fn set_favourites(&mut self, ids: &[usize], on: bool) {
        for &id in ids {
            let path = self.items[id].path.clone();
            if on {
                self.favs.insert(path);
            } else {
                self.favs.remove(&path);
            }
        }
        self.save_favs();
        self.rebuild_view();
    }

    fn share_items(&mut self, ctx: &Context, ids: &[usize]) {
        const MAX: usize = 400;
        let paths: Vec<String> = ids.iter().filter(|&&i| !self.items[i].trashed).take(MAX).map(|&i| self.items[i].path.to_string_lossy().into_owned()).collect();
        if paths.is_empty() {
            return;
        }
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            if perm::share_files(&app, &paths) {
                if ids.len() > MAX {
                    self.say(ctx, format!("Sharing the first {MAX} of {} items", ids.len()), None);
                }
            } else {
                self.say(ctx, "Could not open the share sheet.", None);
            }
            return;
        }
        self.say(ctx, "Sharing is available on the phone.", None);
    }

    fn trash_item(&mut self, ctx: &Context, id: usize) -> bool {
        if !self.manage_ok {
            self.manage_prompt = true;
            return false;
        }
        self.player = None; // do not hold the file open while moving it
        match trash::move_to_trash(&self.items[id].path, &self.trash_dir) {
            Ok(dest) => {
                let it = &mut self.items[id];
                it.orig = Some(it.path.clone());
                it.path = dest;
                it.trashed = true;
                it.album = TRASH_KEY.into();
                let (from, to) = (self.items[id].orig.clone().unwrap_or_default(), self.items[id].path.clone());
                self.notify_media(&[from.to_string_lossy().into_owned(), to.to_string_lossy().into_owned()]);
                self.rebuild_view();
                self.say(ctx, "Moved to trash", Some(id));
                true
            }
            Err(e) => {
                self.say(ctx, format!("Could not delete: {e}"), None);
                false
            }
        }
    }

    fn restore_item(&mut self, ctx: &Context, id: usize) -> bool {
        match trash::restore(&self.items[id].path, &self.trash_dir) {
            Ok(dest) => {
                let it = &mut self.items[id];
                it.album = dest.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                it.path = dest;
                it.trashed = false;
                it.orig = None;
                let back = self.items[id].path.to_string_lossy().into_owned();
                self.notify_media(&[back]);
                self.rebuild_view();
                self.say(ctx, "Restored", None);
                true
            }
            Err(e) => {
                self.say(ctx, format!("Could not restore: {e}"), None);
                false
            }
        }
    }

    /// The folder an item sits in when it is trashed or hidden (the only places files may be erased).
    fn vault_of(&self, id: usize) -> PathBuf {
        if self.items[id].hidden { self.hidden_dir.clone() } else { self.trash_dir.clone() }
    }

    fn delete_forever_item(&mut self, ctx: &Context, id: usize) -> bool {
        self.player = None;
        let dir = self.vault_of(id);
        match trash::delete_forever(&self.items[id].path, &dir) {
            Ok(()) => {
                self.items[id].gone = true;
                self.thumbs.remove(&id);
                self.rebuild_view();
                self.say(ctx, "Deleted permanently", None);
                true
            }
            Err(e) => {
                self.say(ctx, format!("Could not delete: {e}"), None);
                false
            }
        }
    }

    fn empty_trash(&mut self, ctx: &Context) {
        let ids: Vec<usize> = (0..self.items.len()).filter(|&i| self.items[i].trashed && !self.items[i].gone).collect();
        let mut n = 0;
        for id in ids {
            if trash::delete_forever(&self.items[id].path, &self.trash_dir).is_ok() {
                self.items[id].gone = true;
                self.thumbs.remove(&id);
                n += 1;
            }
        }
        self.rebuild_view();
        self.say(ctx, format!("Deleted {n} items permanently"), None);
    }

    // ----- albums home -----

    /// Draw `cover` (or a grey placeholder) centre-cropped into `rect` with rounded corners.
    fn cover_ui(&mut self, ui: &egui::Ui, rect: Rect, cover: Option<usize>, radius: f32) {
        let tex = cover.and_then(|id| self.thumbs.get(&id).cloned());
        match tex {
            Some(tex) => {
                let [tw, th] = tex.size();
                let (tw, th) = (tw as f32, th as f32);
                let uv = if tw > th {
                    let m = (1.0 - th / tw) / 2.0;
                    Rect::from_min_max(pos2(m, 0.0), pos2(1.0 - m, 1.0))
                } else {
                    let m = (1.0 - tw / th) / 2.0;
                    Rect::from_min_max(pos2(0.0, m), pos2(1.0, 1.0 - m))
                };
                egui::Image::new((tex.id(), rect.size())).uv(uv).corner_radius(radius).paint_at(ui, rect);
            }
            None => {
                ui.painter().rect_filled(rect, radius, Color32::from_gray(40));
                if let Some(id) = cover {
                    if !self.failed.contains(&id) {
                        self.request_thumb(id, 0);
                    }
                }
            }
        }
    }

    fn albums_ui(&mut self, ui: &mut egui::Ui) {
        // album covers are always wanted, wherever they sit
        self.shared.vis_lo.store(0, Ordering::Relaxed);
        self.shared.vis_hi.store(usize::MAX / 4, Ordering::Relaxed);

        let pinned: Vec<Album> = self
            .pins
            .iter()
            .filter_map(|k| self.album(k))
            .filter(|a| a.count > 0 || matches!(a.key.as_str(), ALL_KEY | VIDEOS_KEY | FAVS_KEY))
            .collect();
        let pinned_keys: HashSet<String> = pinned.iter().map(|a| a.key.clone()).collect();
        let mut pinned = pinned;
        if self.show_hidden_tile && self.pick.is_none() {
            // the vault's card shows no count and no cover: nothing from inside may be drawn out here
            pinned.push(Album { key: HIDDEN_KEY.into(), label: "Hidden".into(), count: 0, cover: None });
        }
        let mut others: Vec<Album> = self.real_albums.iter().filter(|a| !pinned_keys.contains(&a.key)).cloned().collect();
        if let Some(t) = self.album(TRASH_KEY).filter(|t| t.count > 0 && !pinned_keys.contains(TRASH_KEY)) {
            others.push(t);
        }

        let mut open: Option<String> = None;
        let mut menu: Option<String> = None;
        egui::ScrollArea::vertical().id_salt("albums-home").auto_shrink([false; 2]).show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if self.pick.is_some() && ui.button("‹ Cancel").clicked() {
                    self.cancel_pick();
                }
                ui.heading("Albums");
                if self.pick.is_none() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if ui.add(egui::Button::new("Settings").min_size(vec2(40.0, 30.0))).clicked() {
                            self.open_settings();
                        }
                    });
                }
            });
            ui.add_space(10.0);

            let margin = 10.0;
            let gap = 8.0;
            let width = ui.available_width() - 2.0 * margin;

            // --- pinned: two columns of cards ---
            if !pinned.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(margin);
                    ui.label(egui::RichText::new("Pinned").size(20.0).strong());
                });
                ui.add_space(6.0);
                let tile_w = (width - gap) / 2.0;
                let tile_h = 68.0;
                let rows = pinned.len().div_ceil(2);
                let (_, area) = ui.allocate_space(vec2(width + 2.0 * margin, rows as f32 * (tile_h + gap)));
                for (i, a) in pinned.iter().enumerate() {
                    let (r, c) = (i / 2, i % 2);
                    let rect = Rect::from_min_size(
                        area.min + vec2(margin + c as f32 * (tile_w + gap), r as f32 * (tile_h + gap)),
                        vec2(tile_w, tile_h),
                    );
                    let resp = ui.interact(rect, ui.make_persistent_id(("pin", &a.key)), Sense::click());
                    ui.painter().rect_filled(rect, 14.0, if resp.is_pointer_button_down_on() { Color32::from_gray(44) } else { CARD });
                    let cover = Rect::from_min_size(rect.min + vec2(7.0, 7.0), vec2(tile_h - 14.0, tile_h - 14.0));
                    self.cover_ui(ui, cover, a.cover, 10.0);
                    let text_x = cover.max.x + 10.0;
                    let font = egui::FontId::proportional(15.0);
                    let clipped = ui.painter().with_clip_rect(rect.shrink(4.0));
                    clipped.text(pos2(text_x, rect.min.y + 16.0), egui::Align2::LEFT_TOP, &a.label, font, Color32::WHITE);
                    let sub = if a.key == HIDDEN_KEY { "Locked".to_string() } else { a.count.to_string() };
                    clipped.text(pos2(text_x, rect.min.y + 38.0), egui::Align2::LEFT_TOP, sub, egui::FontId::proportional(13.0), Color32::GRAY);
                    if a.key == HIDDEN_KEY {
                        draw_lock(ui.painter(), cover.center(), 11.0);
                    }
                    if resp.clicked() {
                        open = Some(a.key.clone());
                    }
                    if (resp.long_touched() || resp.secondary_clicked()) && a.key != HIDDEN_KEY {
                        menu = Some(a.key.clone());
                    }
                }
            }

            // --- all other albums: three columns of covers ---
            if !others.is_empty() {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space(margin);
                    ui.label(egui::RichText::new("Albums").size(20.0).strong());
                });
                ui.add_space(6.0);
                let cols = 3;
                let tile_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
                let tile_h = tile_w + 46.0;
                let rows = others.len().div_ceil(cols);
                let (_, area) = ui.allocate_space(vec2(width + 2.0 * margin, rows as f32 * (tile_h + gap) + 90.0));
                for (i, a) in others.iter().enumerate() {
                    let (r, c) = (i / cols, i % cols);
                    let origin = area.min + vec2(margin + c as f32 * (tile_w + gap), r as f32 * (tile_h + gap));
                    let rect = Rect::from_min_size(origin, vec2(tile_w, tile_h));
                    let resp = ui.interact(rect, ui.make_persistent_id(("album", &a.key)), Sense::click());
                    self.cover_ui(ui, Rect::from_min_size(origin, vec2(tile_w, tile_w)), a.cover, 14.0);
                    let clipped = ui.painter().with_clip_rect(Rect::from_min_size(origin + vec2(0.0, tile_w), vec2(tile_w, 46.0)));
                    clipped.text(origin + vec2(2.0, tile_w + 6.0), egui::Align2::LEFT_TOP, &a.label, egui::FontId::proportional(14.0), Color32::WHITE);
                    clipped.text(origin + vec2(2.0, tile_w + 26.0), egui::Align2::LEFT_TOP, a.count.to_string(), egui::FontId::proportional(12.0), Color32::GRAY);
                    if resp.clicked() {
                        open = Some(a.key.clone());
                    }
                    if resp.long_touched() || resp.secondary_clicked() {
                        menu = Some(a.key.clone());
                    }
                }
            }
            if pinned.is_empty() && others.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(if self.scanned { "No photos or videos found." } else { "Scanning…" });
                    ui.add_space(10.0);
                    if self.scanned && ui.add_sized([160.0, 40.0], egui::Button::new("Rescan")).clicked() {
                        let ctx = ui.ctx().clone();
                        self.rescan(&ctx);
                    }
                    #[cfg(target_os = "android")]
                    if self.scanned && !self.manage_ok {
                        ui.add_space(6.0);
                        ui.label("Gallery needs access to all files to see your photos.");
                        if let Some(app) = &self.android {
                            if ui.add_sized([200.0, 40.0], egui::Button::new("Allow file access")).clicked() {
                                perm::open_all_files_settings(app);
                            }
                        }
                    }
                });
            }
        });
        if let Some(k) = open {
            if k == HIDDEN_KEY {
                let ctx = ui.ctx().clone();
                self.request_unlock(&ctx);
            } else {
                self.open_album(&k);
            }
        }
        if menu.is_some() {
            self.album_menu = menu;
        }
    }

    // ----- grid -----

    fn grid_ui(&mut self, ui: &mut egui::Ui) {
        if self.pick.is_some_and(|p| p.multiple) && self.select.is_none() {
            self.select = Some(HashSet::new());
        }
        let album_mode = self.tab == Tab::Albums && self.album_open;
        egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.pick.is_some_and(|p| !p.multiple) {
                    if ui.button("‹ Cancel").clicked() {
                        self.cancel_pick();
                    }
                    let what = match self.pick.map(|p| p.kind) {
                        Some(PickKind::Image) => "Choose a photo",
                        Some(PickKind::Video) => "Choose a video",
                        _ => "Choose a photo or video",
                    };
                    ui.label(egui::RichText::new(what).size(18.0).strong());
                    return;
                }
                if let Some(n) = self.select.as_ref().map(|s| s.len()) {
                    if ui.button("‹ Cancel").clicked() {
                        if self.pick.is_some() {
                            self.cancel_pick();
                        } else {
                            self.exit_select();
                        }
                    }
                    ui.label(egui::RichText::new(format!("{n} selected")).size(18.0).strong());
                    let all = n >= self.view.len();
                    if ui.button(if all { "Select none" } else { "Select all" }).clicked() {
                        self.select = Some(if all { HashSet::new() } else { self.view.iter().copied().collect() });
                    }
                    return;
                }
                if album_mode {
                    if ui.button("‹ Albums").clicked() {
                        self.album_open = false;
                        self.filter = None;
                        self.rebuild_view();
                    }
                    let label = self.filter.as_deref().and_then(|k| self.album(k)).map(|a| a.label).unwrap_or_default();
                    ui.heading(label);
                } else {
                    ui.heading("Photos");
                }
                ui.label(egui::RichText::new(format!("{} items", self.view.len())).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("Settings").min_size(vec2(40.0, 30.0))).clicked() {
                        self.open_settings();
                    }
                    if !self.view.is_empty() && ui.button("Select").clicked() {
                        self.select = Some(HashSet::new());
                    }
                });
            });
        });

        if self.view.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                if self.in_trash_view() {
                    ui.label("Trash is empty.");
                } else if self.scanned {
                    ui.label("Nothing here.");
                    ui.add_space(10.0);
                    if ui.add_sized([160.0, 40.0], egui::Button::new("Rescan")).clicked() {
                        let ctx = ui.ctx().clone();
                        self.rescan(&ctx);
                    }
                } else {
                    ui.label("Scanning…");
                }
            });
            return;
        }

        let gap = 2.0;
        let avail = ui.available_width();
        // pinch the grid: spread = bigger pictures (fewer per row), pinch in = smaller (more per row)
        if let Some(mt) = ui.ctx().multi_touch() {
            self.pinch *= mt.zoom_delta;
            if self.pinch > 1.3 {
                self.set_columns(self.cols - 1);
                self.pinch = 1.0;
            } else if self.pinch < 0.77 {
                self.set_columns(self.cols + 1);
                self.pinch = 1.0;
            }
        } else {
            self.pinch = 1.0;
        }
        let cell = (avail - gap * (self.cols as f32 - 1.0)) / self.cols as f32;
        let now_t = SystemTime::now();
        let (today, yesterday) = (local_day(now_t), local_day(now_t - Duration::from_secs(86_400)));
        let layout = gridlayout::build(&self.view_days, |d| gridlayout::day_label(d, today, yesterday), self.cols, cell, gap);
        let total_h = layout.height + 90.0; // extra room at the end so the bottom bar never hides the last row

        let scroll_id = ("grid", self.filter.clone().unwrap_or_default());
        let mut area = egui::ScrollArea::vertical()
            .id_salt(scroll_id)
            .auto_shrink([false; 2])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        if let Some(y) = self.grid_scroll_to.take() {
            area = area.vertical_scroll_offset(y);
        }
        if self.paint.is_some() {
            area = area.scroll_source(egui::containers::scroll_area::ScrollSource { drag: egui::containers::scroll_area::DragScroll::Never, ..Default::default() }); // the finger is painting, not scrolling
        }
        let out = area.show_viewport(ui, |ui, viewport| {
            let (_, content) = ui.allocate_space(vec2(avail, total_h));
            let (first_row, last_row) = layout.visible(viewport.min.y, viewport.max.y);
            let (mut lo, mut hi) = (usize::MAX, 0);
            for row in &layout.rows[first_row..last_row] {
                if let gridlayout::RowKind::Cells { start, n } = row.kind {
                    lo = lo.min(start);
                    hi = hi.max(start + n);
                }
            }
            if lo == usize::MAX {
                lo = 0;
            }
            self.shared.vis_lo.store(lo, Ordering::Relaxed);
            self.shared.vis_hi.store(hi, Ordering::Relaxed);

            // ---- swipe-to-select ----
            let ctx = ui.ctx().clone();
            let down = ctx.input(|i| i.pointer.primary_down());
            let origin = content.min;
            let tile_at = |p: egui::Pos2| -> Option<usize> {
                let rel = p - origin;
                if rel.x < 0.0 || rel.y < 0.0 {
                    return None;
                }
                match layout.rows[layout.row_at(rel.y)?].kind {
                    gridlayout::RowKind::Cells { start, n } => {
                        let c = (rel.x / (cell + gap)) as usize;
                        (c < n).then_some(start + c)
                    }
                    _ => None,
                }
            };
            // a sideways swipe that starts on a tile begins painting (an up/down swipe still scrolls)
            if self.select.is_some() && self.paint.is_none() && down {
                let (press, now_p) = ctx.input(|i| (i.pointer.press_origin(), i.pointer.interact_pos()));
                if let (Some(o), Some(p)) = (press, now_p) {
                    let d = p - o;
                    if d.x.abs() > 14.0 && d.x.abs() > d.y.abs() * 1.5 && ui.clip_rect().contains(o) && o.y < self.select_bar_top {
                        if let Some(pos) = tile_at(o) {
                            let add = !self.select.as_ref().is_some_and(|s| s.contains(&self.view[pos]));
                            self.paint = Some((add, Some(o)));
                            self.swallow_click = true;
                        }
                    }
                }
            }
            if let Some((add, last)) = self.paint {
                if !down {
                    self.paint = None;
                } else if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                    // every tile the finger crossed since the last frame, not just where it is now
                    let from = last.unwrap_or(p);
                    let steps = ((p - from).length() / (cell / 3.0).max(4.0)).ceil().max(1.0) as usize;
                    for k in 0..=steps {
                        let q = from + (p - from) * (k as f32 / steps as f32);
                        if let (Some(pos), Some(sel)) = (tile_at(q), self.select.as_mut()) {
                            let id = self.view[pos];
                            if add {
                                sel.insert(id);
                            } else {
                                sel.remove(&id);
                            }
                        }
                    }
                    self.paint = Some((add, Some(p)));
                    ctx.request_repaint();
                }
            }

            let mut open = None;
            let mut picked = None;
            for row in &layout.rows[first_row..last_row] {
                let (start, n) = match &row.kind {
                    gridlayout::RowKind::Header { label, start, end } => {
                        let rect = Rect::from_min_size(content.min + vec2(0.0, row.y), vec2(avail, row.h));
                        ui.painter().text(rect.left_center() + vec2(10.0, 4.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(16.0), Color32::from_gray(225));
                        if let Some(sel) = self.select.as_ref() {
                            // in select mode the heading takes a whole day at once
                            let ids: Vec<usize> = self.view[*start..*end].to_vec();
                            let all = ids.iter().all(|id| sel.contains(id));
                            let c = rect.right_center() + vec2(-22.0, 4.0);
                            let blue = Color32::from_rgb(70, 140, 255);
                            if all {
                                ui.painter().circle_filled(c, 11.0, blue);
                                let tick = egui::Stroke::new(2.0, Color32::WHITE);
                                ui.painter().line_segment([c + vec2(-5.0, 0.0), c + vec2(-1.5, 4.0)], tick);
                                ui.painter().line_segment([c + vec2(-1.5, 4.0), c + vec2(5.5, -4.0)], tick);
                            } else {
                                ui.painter().circle_stroke(c, 11.0, egui::Stroke::new(1.5, Color32::from_gray(170)));
                            }
                            let resp = ui.interact(rect, ui.make_persistent_id(("day", *start)), Sense::click());
                            if resp.clicked() && !self.swallow_click {
                                if let Some(sel) = self.select.as_mut() {
                                    for id in ids {
                                        if all {
                                            sel.remove(&id);
                                        } else {
                                            sel.insert(id);
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    gridlayout::RowKind::Cells { start, n } => (*start, *n),
                };
                for pos in start..start + n {
                    let id = self.view[pos];
                    let rect = Rect::from_min_size(content.min + vec2((pos - start) as f32 * (cell + gap), row.y), vec2(cell, cell));
                    let resp = ui.interact(rect, ui.make_persistent_id(("cell", pos)), Sense::click());
                    match self.thumbs.get(&id) {
                        Some(tex) => {
                            let [tw, th] = tex.size();
                            let (tw, th) = (tw as f32, th as f32);
                            // centre-crop to a square
                            let uv = if tw > th {
                                let m = (1.0 - th / tw) / 2.0;
                                Rect::from_min_max(pos2(m, 0.0), pos2(1.0 - m, 1.0))
                            } else {
                                let m = (1.0 - tw / th) / 2.0;
                                Rect::from_min_max(pos2(0.0, m), pos2(1.0, 1.0 - m))
                            };
                            ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
                        }
                        None => {
                            ui.painter().rect_filled(rect, 0.0, Color32::from_gray(32));
                            if self.failed.contains(&id) {
                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(22.0), Color32::GRAY);
                            } else {
                                self.request_thumb(id, pos);
                            }
                        }
                    }
                    if self.items[id].video && cell < 72.0 {
                        // tiles too small for a text badge: a small play triangle on a dark disc
                        let c = rect.right_bottom() - vec2(10.0, 10.0);
                        ui.painter().circle_filled(c, 8.0, Color32::from_black_alpha(170));
                        ui.painter().add(egui::Shape::convex_polygon(
                            vec![c + vec2(-2.5, -4.5), c + vec2(-2.5, 4.5), c + vec2(4.5, 0.0)],
                            Color32::WHITE,
                            egui::Stroke::NONE,
                        ));
                    } else if self.items[id].video {
                        let label = self.durations.get(&id).map_or("VIDEO".to_string(), |d| media::format_duration(*d));
                        let font = egui::FontId::proportional(12.0);
                        let galley = ui.painter().layout_no_wrap(label, font, Color32::WHITE);
                        let badge = Rect::from_min_size(rect.right_bottom() - galley.size() - vec2(12.0, 8.0), galley.size() + vec2(8.0, 4.0));
                        ui.painter().rect_filled(badge, 4.0, Color32::from_black_alpha(170));
                        ui.painter().galley(badge.min + vec2(4.0, 2.0), galley, Color32::WHITE);
                    }
                    if let Some(sel) = &self.select {
                        let on = sel.contains(&id);
                        let painter = ui.painter();
                        if on {
                            painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(40, 120, 255, 70));
                            painter.rect_stroke(rect.shrink(1.5), 0.0, egui::Stroke::new(3.0, Color32::from_rgb(70, 140, 255)), egui::StrokeKind::Inside);
                        }
                        let r = (cell * 0.16).clamp(6.0, 12.0);
                        let c = rect.left_top() + vec2(r + 4.0, r + 4.0);
                        if on {
                            painter.circle_filled(c, r, Color32::from_rgb(70, 140, 255));
                            let k = r * 0.5;
                            let tick = egui::Stroke::new((r * 0.22).max(1.5), Color32::WHITE);
                            painter.line_segment([c + vec2(-k * 0.8, 0.0), c + vec2(-k * 0.2, k * 0.65)], tick);
                            painter.line_segment([c + vec2(-k * 0.2, k * 0.65), c + vec2(k * 0.9, -k * 0.6)], tick);
                        } else {
                            painter.circle_filled(c, r, Color32::from_black_alpha(90));
                            painter.circle_stroke(c, r, egui::Stroke::new(1.5, Color32::from_white_alpha(200)));
                        }
                    }
                    if resp.long_touched() && self.paint.is_none() && !self.pick.is_some_and(|p| !p.multiple) {
                        // hold on a tile: start selecting, and keep painting if the finger keeps moving
                        let sel = self.select.get_or_insert_with(HashSet::new);
                        let add = !sel.contains(&id);
                        if add {
                            sel.insert(id);
                        } else {
                            sel.remove(&id);
                        }
                        self.paint = Some((add, ctx.input(|i| i.pointer.interact_pos())));
                        self.swallow_click = true;
                    }
                    if resp.clicked() && !self.swallow_click {
                        if let Some(sel) = self.select.as_mut() {
                            if !sel.remove(&id) {
                                sel.insert(id);
                            }
                        } else if self.pick.is_some() {
                            picked = Some(id);
                        } else {
                            open = Some(pos);
                        }
                    }
                }
                let _ = n;
            }
            if !down {
                self.swallow_click = false;
            }
            if let Some(pos) = open {
                self.open_viewer(pos);
            }
            if let Some(id) = picked {
                self.finish_pick(&ctx, &[id]);
            }
        });
        let offset_y = out.state.clamped_offset().y;
        let vis = out.inner_rect;

        // while painting, hovering near the top or bottom edge scrolls the grid
        if let (Some(_), Some(p)) = (self.paint, ui.ctx().input(|i| i.pointer.interact_pos())) {
            let dt = ui.ctx().input(|i| i.unstable_dt).min(0.05);
            let bottom_edge = self.select_bar_top.min(vis.max.y);
            let speed = if p.y < vis.min.y + 90.0 {
                -(1.0 - ((p.y - vis.min.y) / 90.0).clamp(0.0, 1.0))
            } else if p.y > bottom_edge - 90.0 {
                ((p.y - (bottom_edge - 90.0)) / 90.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if speed != 0.0 {
                self.grid_scroll_to = Some((offset_y + speed * 1400.0 * dt).max(0.0));
                ui.ctx().request_repaint();
            }
        }

        // ---- fast-scroll handle: drag it to run through the whole library, with the date shown ----
        let max_scroll = (total_h - vis.height()).max(0.0);
        let now = ui.ctx().input(|i| i.time);
        if (offset_y - self.grid_last_offset).abs() > 0.5 {
            self.grid_last_offset = offset_y;
            self.grid_last_move = now;
        }
        if max_scroll > vis.height() * 1.2 && self.paint.is_none() {
            const HANDLE_H: f32 = 56.0;
            let track = (vis.height() - HANDLE_H).max(1.0);
            let frac = (offset_y / max_scroll).clamp(0.0, 1.0);
            let handle = Rect::from_min_size(pos2(vis.max.x - 44.0, vis.min.y + frac * track), vec2(44.0, HANDLE_H));
            let dragging = ui.ctx().dragged_id() == Some(ui.make_persistent_id("fast-scroll"));
            let recently = now - self.grid_last_move < 1.4;
            if dragging || recently {
                let resp = ui.interact(handle, ui.make_persistent_id("fast-scroll"), Sense::drag());
                if resp.dragged() {
                    if let Some(p) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                        let f = ((p.y - vis.min.y - HANDLE_H / 2.0) / track).clamp(0.0, 1.0);
                        self.grid_scroll_to = Some(f * max_scroll);
                        self.grid_last_move = now;
                    }
                    ui.ctx().request_repaint();
                }
                let held = resp.dragged();
                let grip = Rect::from_min_size(handle.min + vec2(if held { 20.0 } else { 28.0 }, 4.0), vec2(if held { 14.0 } else { 6.0 }, HANDLE_H - 8.0));
                let painter = ui.painter();
                painter.rect_filled(grip, 7.0, if held { Color32::from_rgb(70, 140, 255) } else { Color32::from_white_alpha(130) });
                if held {
                    if let Some(label) = layout.label_at(offset_y + vis.height() * 0.5) {
                        let galley = painter.layout_no_wrap(label.to_string(), egui::FontId::proportional(17.0), Color32::WHITE);
                        let size = galley.size() + vec2(24.0, 14.0);
                        let bubble = Rect::from_center_size(pos2(handle.min.x - size.x / 2.0 - 8.0, handle.center().y), size);
                        painter.rect_filled(bubble, 14.0, Color32::from_black_alpha(215));
                        painter.galley(bubble.center() - galley.size() / 2.0, galley, Color32::WHITE);
                    }
                }
                if recently && !held {
                    ui.ctx().request_repaint_after(Duration::from_millis(300)); // so the handle fades away
                }
            }
        }
        self.trim_textures();
    }

    // ----- permission screen, nav bar, toast, prompts -----

    fn permission_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            ui.heading("Gallery needs access to your photos and videos");
            ui.add_space(8.0);
            ui.label("Everything stays on your phone. Nothing is uploaded.");
            ui.add_space(16.0);
            #[cfg(target_os = "android")]
            if let Some(app) = &self.android {
                if ui.button("Allow access").clicked() {
                    perm::request_permissions(app, &[perm::MEDIA_IMAGES, perm::MEDIA_VIDEO]);
                }
            }
        });
    }

    fn overlays(&mut self, ctx: &Context, bottom_inset: f32) {
        // bottom pill: switch between the two modes
        if self.viewer.is_none() && self.media_ok && !self.album_open && self.select.is_none() && !self.settings_open && self.space.is_none() && self.pick.is_none() {
            let mut go = None;
            egui::Area::new(egui::Id::new("nav"))
                .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -(bottom_inset + 10.0)))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::new().fill(Color32::from_gray(38)).corner_radius(26.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (tab, label) in [(Tab::Photos, "Photos"), (Tab::Albums, "Albums")] {
                                let on = self.tab == tab;
                                let text = egui::RichText::new(label).size(15.0).color(if on { Color32::WHITE } else { Color32::GRAY });
                                let b = egui::Button::new(text).fill(if on { Color32::from_gray(70) } else { Color32::TRANSPARENT }).corner_radius(20.0).min_size(vec2(96.0, 34.0));
                                if ui.add(b).clicked() && !on {
                                    go = Some(tab);
                                }
                            }
                        });
                    });
                });
            if let Some(tab) = go {
                self.go_to_tab(tab);
            }
        }

        // action bar while selecting
        if self.select.is_some() && self.viewer.is_none() && self.media_ok && !(self.tab == Tab::Albums && !self.album_open) {
            #[derive(PartialEq)]
            enum Batch {
                Share,
                Favourite(bool),
                Delete,
                Restore,
                Forever,
                Done,
                Hide,
                Unhide,
                MoveTo,
                CopyTo,
            }
            let ids = self.selected_ids();
            let n = ids.len();
            let in_trash = self.in_trash_view();
            let in_hidden = self.filter.as_deref() == Some(HIDDEN_KEY);
            let all_fav = n > 0 && ids.iter().all(|&i| self.is_fav(i));
            let armed = self.confirm_batch_forever == Some(n);
            let red = Color32::from_rgb(255, 120, 110);
            let mut act: Option<Batch> = None;
            let mut menu_open = self.select_menu;
            let bar = egui::Area::new(egui::Id::new("select-bar"))
                .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -(bottom_inset + 10.0)))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::new().fill(Color32::from_gray(38)).corner_radius(26.0).inner_margin(egui::Margin::symmetric(8, 8)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let btn = |text: egui::RichText| egui::Button::new(text).min_size(vec2(72.0, 38.0)).corner_radius(18.0);
                            let forever_label = if armed { format!("Tap again: delete {n} forever") } else { "Delete forever".to_string() };
                            if self.pick.is_some() {
                                if ui.add_enabled(n > 0, btn(format!("Done ({n})").into())).clicked() {
                                    act = Some(Batch::Done);
                                }
                            } else if in_trash {
                                if ui.add_enabled(n > 0, btn("Restore".into())).clicked() {
                                    act = Some(Batch::Restore);
                                }
                                if ui.add_enabled(n > 0, btn(egui::RichText::new(forever_label).color(red))).clicked() {
                                    act = Some(Batch::Forever);
                                }
                            } else if in_hidden {
                                if ui.add_enabled(n > 0, btn("Share".into())).clicked() {
                                    act = Some(Batch::Share);
                                }
                                if ui.add_enabled(n > 0, btn("Unhide".into())).clicked() {
                                    act = Some(Batch::Unhide);
                                }
                                if ui.add_enabled(n > 0, btn(egui::RichText::new(forever_label).color(red))).clicked() {
                                    act = Some(Batch::Forever);
                                }
                            } else {
                                if ui.add_enabled(n > 0, btn("Share".into())).clicked() {
                                    act = Some(Batch::Share);
                                }
                                if ui.add_enabled(n > 0, btn(if all_fav { "Unfav" } else { "Favourite" }.into())).clicked() {
                                    act = Some(Batch::Favourite(!all_fav));
                                }
                                if ui.add_enabled(n > 0, btn(egui::RichText::new("Delete").color(red))).clicked() {
                                    act = Some(Batch::Delete);
                                }
                                if ui.add_enabled(n > 0, btn("More".into())).clicked() {
                                    menu_open = !menu_open;
                                }
                            }
                        });
                    });
                });
            let bar_rect = bar.response.rect;
            self.select_bar_top = bar_rect.min.y;
            if menu_open && n > 0 && !in_trash && !in_hidden && self.pick.is_none() {
                egui::Area::new(egui::Id::new("select-menu"))
                    .pivot(egui::Align2::CENTER_BOTTOM)
                    .fixed_pos(pos2(bar_rect.center().x, bar_rect.min.y - 8.0))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_min_width(230.0);
                            let entry = |ui: &mut egui::Ui, text: &str| ui.add_sized([230.0, 38.0], egui::Button::new(text)).clicked();
                            if entry(ui, "Hide in private album") {
                                act = Some(Batch::Hide);
                            }
                            if entry(ui, "Move to album…") {
                                act = Some(Batch::MoveTo);
                            }
                            if entry(ui, "Copy to album…") {
                                act = Some(Batch::CopyTo);
                            }
                        });
                    });
            } else if n == 0 {
                menu_open = false;
            }
            if act.is_some() {
                menu_open = false;
            }
            self.select_menu = menu_open;
            if act != Some(Batch::Forever) && act.is_some() {
                self.confirm_batch_forever = None;
            }
            match act {
                Some(Batch::Done) => self.finish_pick(ctx, &ids),
                Some(Batch::Share) => self.share_items(ctx, &ids),
                Some(Batch::Favourite(on)) => {
                    self.set_favourites(&ids, on);
                    self.say(ctx, if on { format!("Added {n} to favourites") } else { format!("Removed {n} from favourites") }, None);
                    self.exit_select();
                }
                Some(Batch::Delete) => {
                    if self.trash_items(ctx, &ids) > 0 {
                        self.exit_select();
                    }
                }
                Some(Batch::Hide) => {
                    if self.hide_items(ctx, &ids) > 0 {
                        self.exit_select();
                    }
                }
                Some(Batch::Unhide) => {
                    self.unhide_items(ctx, &ids);
                    self.exit_select();
                }
                Some(Batch::MoveTo) => self.open_album_picker(AlbumOp::Move, ids.clone()),
                Some(Batch::CopyTo) => self.open_album_picker(AlbumOp::Copy, ids.clone()),
                Some(Batch::Restore) => {
                    self.restore_items(ctx, &ids);
                    self.exit_select();
                }
                Some(Batch::Forever) => {
                    if armed {
                        self.delete_forever_items(ctx, &ids);
                        self.exit_select();
                    } else {
                        self.confirm_batch_forever = Some(n);
                    }
                }
                None => {}
            }
        } else {
            self.select_bar_top = f32::MAX;
        }

        // long-press menu on an album
        if let Some(key) = self.album_menu.clone() {
            let Some(album) = self.album(&key) else {
                self.album_menu = None;
                return;
            };
            let pinned = self.pins.contains(&key);
            let real = self.real_albums.iter().any(|a| a.key == key);
            let (mut rename, mut delete) = (false, false);
            let mut close = false;
            let mut toggle = false;
            egui::Area::new(egui::Id::new("album-menu"))
                .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(220.0);
                        ui.label(egui::RichText::new(&album.label).strong());
                        ui.add_space(6.0);
                        if ui.add_sized([220.0, 36.0], egui::Button::new(if pinned { "Unpin from top" } else { "Pin to top" })).clicked() {
                            toggle = true;
                        }
                        if real {
                            if ui.add_sized([220.0, 36.0], egui::Button::new("Rename…")).clicked() {
                                rename = true;
                            }
                            if ui.add_sized([220.0, 36.0], egui::Button::new(egui::RichText::new("Delete album…").color(Color32::from_rgb(255, 120, 110)))).clicked() {
                                delete = true;
                            }
                        }
                        if ui.add_sized([220.0, 36.0], egui::Button::new("Cancel")).clicked() {
                            close = true;
                        }
                    });
                });
            if rename {
                let folder = Path::new(&key).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.album_dialog = Some(albumops::AlbumDialog::Rename { key: key.clone(), text: folder });
                close = true;
            }
            if delete {
                self.album_dialog = Some(albumops::AlbumDialog::Delete { key: key.clone() });
                close = true;
            }
            if toggle {
                self.toggle_pin(&key);
            }
            if toggle || close {
                self.album_menu = None;
            }
        }

        let now = ctx.input(|i| i.time);
        if self.toast.as_ref().is_some_and(|t| now > t.2) {
            self.toast = None;
        }
        let mut undo: Option<Vec<usize>> = None;
        if let Some((msg, undo_ids, _)) = self.toast.clone() {
            // lift the toast above the video controls and the bottom bar
            let lift = if self.player.is_some() { 270.0 } else if self.viewer.is_none() { 70.0 } else { 80.0 };
            egui::Area::new(egui::Id::new("toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -(bottom_inset + lift)))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(msg);
                            if !undo_ids.is_empty() && ui.button("Undo").clicked() {
                                undo = Some(undo_ids.clone());
                            }
                        });
                    });
                });
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if let Some(ids) = undo {
            self.toast = None;
            self.restore_items(ctx, &ids);
        }

        self.album_picker_ui(ctx);
        self.album_dialog_ui(ctx);

        // Deleting needs "All files access"; offer its settings screen after the first Delete tap.
        if self.manage_ok {
            self.manage_prompt = false;
        }
        #[cfg(target_os = "android")]
        if self.manage_prompt {
            if let Some(app) = self.android.clone() {
                egui::Area::new(egui::Id::new("manage"))
                    .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label("To delete photos, allow file management for this app.");
                            ui.label(egui::RichText::new("Deleted items go to the trash first, so you can undo.").small().weak());
                            ui.horizontal(|ui| {
                                if ui.button("Open settings").clicked() {
                                    perm::open_all_files_settings(&app);
                                }
                                if ui.button("Cancel").clicked() {
                                    self.manage_prompt = false;
                                }
                            });
                        });
                    });
            }
        }
    }
}

impl eframe::App for Gallery {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.refresh_permissions(&ctx);
        self.request_high_refresh_rate();
        self.drain(&ctx);
        self.track_focus(&ctx);
        if self.rescan_at.is_some_and(|t| Instant::now() >= t) {
            self.rescan_at = None;
            self.rescan(&ctx);
        }
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            diag::log(&format!("first frame after {} ms; media_ok={} manage_ok={}", self.started.elapsed().as_millis(), self.media_ok, self.manage_ok));
        }
        let size = ctx.viewport_rect().size();
        let screen = (size.x as u32, size.y as u32);
        if screen != self.last_screen {
            self.last_screen = screen;
            diag::log(&format!("window {}x{} points, {:.2} px per point", screen.0, screen.1, ctx.pixels_per_point()));
        }
        if self.frames_since_focus < 4 {
            // a few extra frames right after coming back make sure the first picture is really drawn
            self.frames_since_focus += 1;
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        self.poll_intents(&ctx);
        self.poll_auth(&ctx);
        // the vault only stays open while it is on screen
        if self.hidden_unlocked && !self.auth_pending && self.filter.as_deref() != Some(HIDDEN_KEY) {
            self.lock_hidden(&ctx);
        }

        // Back: viewer -> grid -> albums home -> leave the app
        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::BrowserBack)) {
            if self.album_menu.is_some() {
                self.album_menu = None;
            } else if self.album_picker.is_some() || self.album_dialog.is_some() {
                self.album_picker = None;
                self.album_dialog = None;
            } else if self.space.is_some() {
                self.close_space();
            } else if self.settings_open {
                self.close_settings();
            } else if self.viewer.is_some() && (self.viewer_menu || self.details_open) {
                self.viewer_menu = false;
                self.details_open = false;
            } else if self.viewer.is_some() {
                self.leave_viewer(&ctx);
            } else if self.select.is_some() {
                if self.pick.is_some() {
                    self.cancel_pick();
                } else {
                    self.exit_select();
                }
            } else if self.tab == Tab::Albums && self.album_open {
                self.album_open = false;
                self.filter = None;
                self.rebuild_view();
            } else if self.pick.is_some() {
                self.cancel_pick();
            } else {
                self.exit(&ctx);
            }
        }

        // selecting only makes sense on a grid
        if self.select.is_some() && self.pick.is_none() && (self.viewer.is_some() || (self.tab == Tab::Albums && !self.album_open)) {
            self.exit_select();
        }

        let (top, bottom) = self.insets(&ctx);
        self.insets = (top, bottom);
        let margin = egui::Margin { left: 0, right: 0, top: top.round() as i8, bottom: bottom.round() as i8 };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(margin)).show(ui, |ui| {
            if !self.media_ok {
                self.permission_ui(ui);
            } else if self.space.is_some() {
                self.space_ui(ui);
            } else if self.settings_open {
                self.settings_ui(ui);
            } else if self.viewer.is_some() {
                self.viewer_ui(ui);
            } else if self.tab == Tab::Albums && !self.album_open {
                self.albums_ui(ui);
            } else {
                self.grid_ui(ui);
            }
        });
        self.overlays(&ctx, bottom);
        if !self.requested.is_empty() || !self.scanned {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
}

// ---------- helpers ----------

/// A small padlock, drawn from shapes so it needs no icon font.
fn draw_lock(p: &egui::Painter, c: egui::Pos2, s: f32) {
    let col = Color32::from_gray(215);
    let body = Rect::from_center_size(c + vec2(0.0, s * 0.45), vec2(s * 1.6, s * 1.25));
    p.rect_filled(body, 3.0, col);
    let r = s * 0.5;
    let pts: Vec<egui::Pos2> = (0..=16)
        .map(|k| {
            let a = std::f32::consts::PI * k as f32 / 16.0;
            pos2(c.x + r * a.cos(), body.min.y - r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(pts, egui::Stroke::new(2.5, col)));
}

fn remap_map<V>(m: HashMap<usize, V>, remap: &[Option<usize>]) -> HashMap<usize, V> {
    m.into_iter().filter_map(|(k, v)| remap.get(k).copied().flatten().map(|n| (n, v))).collect()
}

fn remap_set(s: HashSet<usize>, remap: &[Option<usize>]) -> HashSet<usize> {
    s.into_iter().filter_map(|k| remap.get(k).copied().flatten()).collect()
}

fn start_scan(roots: Vec<PathBuf>, trash_dir: PathBuf, hidden_dir: PathBuf, ctx: Context) -> Receiver<Vec<Item>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(scan(&roots, &trash_dir, &hidden_dir));
        ctx.request_repaint();
    });
    rx
}

/// Where the app's own data lives; `roots.txt` here (one folder per line) overrides the
/// default folders, which is handy for testing with generated files.
fn default_roots() -> Vec<PathBuf> {
    if let Some(lines) = read_lines(&data_dir(), "roots.txt") {
        if !lines.is_empty() {
            return lines.into_iter().map(PathBuf::from).collect();
        }
    }
    // all shared storage, like a MediaStore-based gallery (Android/, hidden and .nomedia folders are skipped)
    vec![PathBuf::from("/sdcard")]
}

/// The day of `t` in local time, as yyyymmdd.
#[cfg(unix)]
fn local_day(t: SystemTime) -> i32 {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // SAFETY: localtime_r only writes into the zeroed tm we hand it.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let tt = secs as libc::time_t;
        libc::localtime_r(&tt, &mut tm);
        gridlayout::day_key(tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32)
    }
}

#[cfg(not(unix))]
fn local_day(t: SystemTime) -> i32 {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    gridlayout::day_key(y as i32, m as u32, d as u32)
}

#[cfg(unix)]
fn local_time(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // SAFETY: localtime_r only writes into the zeroed tm we hand it.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let tt = secs as libc::time_t;
        libc::localtime_r(&tt, &mut tm);
        format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
    }
}

#[cfg(not(unix))]
fn local_time(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // civil-from-days (Howard Hinnant); UTC only on desktop
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
    let handle = app.clone();
    let options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
    if let Err(e) = eframe::run_native(
        "Gallery",
        options,
        Box::new(move |cc| Ok(Box::new(Gallery::new(cc, Vec::new()).with_android(handle.clone())))),
    ) {
        diag::log(&format!("eframe exited with an error: {e}"));
        log::error!("gallery exited: {e}");
    }
    diag::log("android_main returning");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gallery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scan_finds_media_sorts_newest_first_and_skips_hidden_nomedia_and_android() {
        let dir = temp("scan");
        for sub in ["Camera", ".thumbnails", "Private", "Android/data", "Android/media/com.chat/Media"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let img = image::RgbImage::from_pixel(64, 32, image::Rgb([200, 30, 30]));
        img.save(dir.join("Camera/a.jpg")).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        img.save(dir.join("Camera/b.PNG")).unwrap();
        std::fs::write(dir.join("Camera/clip.mp4"), b"not really a video").unwrap();
        img.save(dir.join(".thumbnails/hidden.jpg")).unwrap();
        img.save(dir.join("Private/secret.jpg")).unwrap();
        std::fs::write(dir.join("Private/.nomedia"), b"").unwrap();
        img.save(dir.join("Android/data/cache.jpg")).unwrap();
        img.save(dir.join("Android/media/com.chat/Media/received.jpg")).unwrap();
        std::fs::write(dir.join("Camera/stream.ts"), b"transport stream").unwrap();
        std::fs::write(dir.join("Camera/notes.txt"), "x").unwrap();

        let items = scan(&[dir.clone()], &dir.join(".trash"), &dir.join(".vault"));
        let names: Vec<_> = items.iter().map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(items.len(), 5, "{names:?}");
        assert!(names.iter().all(|n| n != "hidden.jpg" && n != "secret.jpg" && n != "cache.jpg" && n != "notes.txt"));
        assert!(names.contains(&"received.jpg".to_string()), "Android/media is scanned");
        assert!(names.contains(&"stream.ts".to_string()), ".ts is a video");
        assert_eq!(items.iter().filter(|i| i.video).count(), 2);
        assert!(items.windows(2).all(|w| w[0].mtime >= w[1].mtime), "newest first");
        assert!(items.iter().filter(|i| !i.path.ends_with("received.jpg")).all(|i| i.album == dir.join("Camera").to_string_lossy()), "album key is the full folder path");

        let a = items.iter().find(|i| i.path.ends_with("a.jpg")).unwrap();
        let (c, dims) = decode(&a.path, 16, true).unwrap();
        assert_eq!(dims, [64, 32]);
        assert!(c.size[0] <= 16 && c.size[1] <= 16, "thumbnail respects max size");
        assert!(decode(&dir.join("Camera/notes.txt"), 16, true).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trashed_media_shows_up_only_as_trash_and_decodes_without_an_extension() {
        let dir = temp("trash");
        std::fs::create_dir_all(dir.join("Camera")).unwrap();
        let photo = dir.join("Camera/keep.jpg");
        let gone = dir.join("Camera/gone.jpg");
        let clip = dir.join("Camera/clip.mp4");
        let img = image::RgbImage::from_pixel(40, 40, image::Rgb([5, 5, 200]));
        img.save(&photo).unwrap();
        img.save(&gone).unwrap();
        std::fs::write(&clip, b"video bytes").unwrap();
        let trash_dir = dir.join(".trash");
        let t = trash::move_to_trash(&gone, &trash_dir).unwrap();
        trash::move_to_trash(&clip, &trash_dir).unwrap();
        std::fs::write(trash_dir.join("junk.txt"), "not media").unwrap();

        let items = scan(&[dir.clone()], &trash_dir, &dir.join(".vault"));
        assert_eq!(items.len(), 3, "one live photo plus a trashed photo and video, junk ignored");
        let trashed: Vec<_> = items.iter().filter(|i| i.trashed).collect();
        assert_eq!(trashed.len(), 2);
        assert!(trashed.iter().all(|i| i.album == TRASH_KEY));
        assert!(trashed.iter().any(|i| i.video && i.orig.as_deref() == Some(clip.as_path())));
        assert!(trashed.iter().any(|i| !i.video && i.orig.as_deref() == Some(gone.as_path())));
        assert!(items.iter().any(|i| !i.trashed && i.path == photo));
        assert!(decode(&t, 100, false).is_some(), "format is sniffed, not taken from the extension");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_files_are_found_only_as_hidden_and_in_no_other_view() {
        let dir = temp("vault");
        let vault = dir.join(".vault");
        std::fs::create_dir_all(dir.join("Camera")).unwrap();
        let img = image::RgbImage::from_pixel(20, 20, image::Rgb([9, 9, 9]));
        let secret = dir.join("Camera/private.jpg");
        img.save(&secret).unwrap();
        img.save(dir.join("Camera/public.jpg")).unwrap();
        let moved = trash::move_to_trash(&secret, &vault).unwrap();

        let items = scan(&[dir.clone()], &dir.join(".trash"), &vault);
        assert_eq!(items.len(), 2);
        let h: Vec<_> = items.iter().filter(|i| i.hidden).collect();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].path, moved);
        assert_eq!(h[0].album, HIDDEN_KEY);
        assert!(!h[0].trashed && h[0].orig.as_deref() == Some(secret.as_path()));
        assert!(items.iter().filter(|i| !i.hidden).all(|i| i.path.ends_with("public.jpg")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remapping_follows_files_to_their_new_numbers() {
        let remap = vec![Some(2), None, Some(0)];
        let m: HashMap<usize, &str> = [(0, "a"), (1, "b"), (2, "c"), (9, "z")].into_iter().collect();
        let m = remap_map(m, &remap);
        assert_eq!(m.get(&2), Some(&"a"));
        assert_eq!(m.get(&0), Some(&"c"));
        assert_eq!(m.len(), 2, "removed and unknown numbers are dropped");
        let s: HashSet<usize> = [0, 1, 2, 7].into_iter().collect();
        assert_eq!(remap_set(s, &remap), [2, 0].into_iter().collect());
    }

    #[test]
    fn album_keys_cannot_collide_with_folder_paths() {
        for k in [ALL_KEY, VIDEOS_KEY, FAVS_KEY, TRASH_KEY, HIDDEN_KEY, EXT_KEY] {
            assert!(k.starts_with('\u{1}'));
            assert!(!Path::new(k).is_absolute());
        }
        let all = [ALL_KEY, VIDEOS_KEY, FAVS_KEY, TRASH_KEY, HIDDEN_KEY, EXT_KEY];
        assert_eq!(all.iter().collect::<HashSet<_>>().len(), 6);
    }

    #[test]
    fn pins_round_trip_through_the_file_format() {
        let keys = [ALL_KEY.to_string(), "/sdcard/DCIM/Camera".to_string(), VIDEOS_KEY.to_string(), FAVS_KEY.to_string()];
        let lines: Vec<String> = keys.iter().map(|k| key_to_line(k)).collect();
        assert_eq!(lines, ["@all", "/sdcard/DCIM/Camera", "@videos", "@favs"]);
        let back: Vec<String> = lines.iter().map(|l| line_to_key(l)).collect();
        assert_eq!(back, keys);

        let dir = temp("pins");
        write_lines(&dir, "pins.txt", &lines);
        assert_eq!(read_lines(&dir, "pins.txt").unwrap(), lines);
        assert!(read_lines(&dir, "missing.txt").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn columns_setting_is_parsed_and_kept_in_range() {
        let l = |s: &str| vec![s.to_string()];
        assert_eq!(parse_columns(&l("columns=7")), Some(7));
        assert_eq!(parse_columns(&l("columns=1")), Some(COLUMNS_MIN));
        assert_eq!(parse_columns(&l("columns=99")), Some(COLUMNS_MAX));
        assert_eq!(parse_columns(&l("columns=abc")), None);
        assert_eq!(parse_columns(&[]), None);

        let dir = temp("cols");
        assert_eq!(load_columns(&dir), COLUMNS_DEFAULT, "no file yet -> default of five");
        write_lines(&dir, "settings.txt", &["columns=9".to_string()]);
        assert_eq!(load_columns(&dir), 9);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(COLUMNS_DEFAULT, 5);
    }

    #[test]
    fn numbers_in_settings_are_parsed() {
        let l = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_int(&l(&["columns=5", "skipsecs=15"]), "skipsecs="), Some(15));
        assert_eq!(parse_int(&l(&["skipsecs=abc"]), "skipsecs="), None);
        assert_eq!(parse_int(&l(&[]), "skipsecs="), None);
        assert!(SKIP_CHOICES.contains(&10));
    }

    #[test]
    fn switches_are_parsed() {
        let l = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_flag(&l(&["columns=5", "loop=1"]), "loop="), Some(true));
        assert_eq!(parse_flag(&l(&["loop=0"]), "loop="), Some(false));
        assert_eq!(parse_flag(&l(&["columns=5"]), "loop="), None);
    }

    #[test]
    fn batch_trash_and_restore_move_every_item_once() {
        let dir = temp("batch");
        let trash_dir = dir.join(".trash");
        let paths: Vec<PathBuf> = (0..4).map(|i| dir.join(format!("p{i}.jpg"))).collect();
        for p in &paths {
            image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])).save(p).unwrap();
        }
        let moved: Vec<PathBuf> = paths.iter().map(|p| trash::move_to_trash(p, &trash_dir).unwrap()).collect();
        assert!(paths.iter().all(|p| !p.exists()));
        for t in &moved {
            trash::restore(t, &trash_dir).unwrap();
        }
        assert!(paths.iter().all(|p| p.exists()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_pins_pick_camera_and_screenshots_with_most_items() {
        let a = |key: &str, label: &str, count| Album { key: key.into(), label: label.into(), count, cover: None };
        let real = [a("/x/Camera", "Camera", 3), a("/y/Camera", "Camera", 40), a("/z/Screenshots", "Screenshots", 9), a("/w/shoots", "shoots", 200)];
        assert_eq!(default_pins(&real), [ALL_KEY, "/y/Camera", "/z/Screenshots", VIDEOS_KEY, FAVS_KEY]);
        assert_eq!(default_pins(&[a("/w/shoots", "shoots", 5)]), [ALL_KEY, VIDEOS_KEY, FAVS_KEY]);
    }

    /// Insert a minimal EXIF block (orientation only) right after the JPEG SOI marker.
    fn jpeg_with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0\x08\0\0\0"); // little-endian TIFF header, IFD at offset 8
        tiff.extend_from_slice(&1u16.to_le_bytes()); // one entry
        tiff.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_le_bytes()); // count
        tiff.extend_from_slice(&orientation.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]); // value padding
        tiff.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let len = (app1.len() + 2) as u16;
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn exif_orientation_is_applied() {
        let dir = temp("exif");
        let mut plain = Vec::new();
        image::RgbImage::from_pixel(64, 32, image::Rgb([10, 200, 10]))
            .write_to(&mut std::io::Cursor::new(&mut plain), image::ImageFormat::Jpeg)
            .unwrap();
        for (o, expect) in [(1u16, [64u32, 32u32]), (6, [32, 64]), (8, [32, 64]), (3, [64, 32])] {
            let p = dir.join(format!("o{o}.jpg"));
            std::fs::write(&p, jpeg_with_orientation(&plain, o)).unwrap();
            let (c, dims) = decode(&p, 1000, false).expect("decodes");
            assert_eq!(dims, expect, "orientation {o}");
            assert_eq!([c.size[0] as u32, c.size[1] as u32], expect, "orientation {o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_time_formats() {
        assert!(local_time(UNIX_EPOCH + Duration::from_secs(86_400 * 365)).starts_with("19"));
    }
}
