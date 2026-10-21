//! Files: a small, fast file explorer. Home (storage, quick folders, categories), folder browsing,
//! search, largest and recent files, copy / move / delete (to a trash first) / rename / share, and
//! a trash that empties itself after 30 days.

mod diag;
mod fsops;
mod media;
#[cfg(target_os = "android")]
mod perm;
mod thumbcache;
mod timefmt;
mod trash;
mod ui;

use eframe::egui::{self, Color32, ColorImage, Context, Pos2, Rect, TextureHandle, TextureOptions, pos2, vec2};
use fsops::{Entry, Kind, Scan, Sort, SortKey};
use image::{DynamicImage, ImageDecoder, ImageReader};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const BRAND: Color32 = Color32::from_rgb(252, 87, 0);
pub(crate) const BG: Color32 = Color32::from_rgb(14, 14, 16);
pub(crate) const CARD: Color32 = Color32::from_gray(30);
pub(crate) const ROW_H: f32 = 62.0;
const THUMB_PX: u32 = 160;
const TRASH_DAYS: u64 = 30;
const UNDO_SECS: f64 = 7.0;
const LIST_LIMIT: usize = 150;

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Screen {
    Home,
    Folder(PathBuf),
    Category(Kind),
    Recent,
    Large,
    Search(String),
    Trash,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum JobKind {
    Copy,
    Move,
    Trash,
    Restore,
    DeleteForever,
}

impl JobKind {
    fn title(self) -> &'static str {
        match self {
            JobKind::Copy => "Copying",
            JobKind::Move => "Moving",
            JobKind::Trash => "Moving to trash",
            JobKind::Restore => "Restoring",
            JobKind::DeleteForever => "Deleting",
        }
    }
}

#[derive(Default)]
pub(crate) struct Prog {
    pub done: u64,
    pub total: u64,
    pub current: String,
    pub item: usize,
    pub items: usize,
}

#[derive(Default)]
pub(crate) struct JobResult {
    pub ok: usize,
    pub failed: Vec<String>,
    /// Media files whose place changed, for Android's media database.
    pub touched: Vec<String>,
    /// Where trashed items now are, so the toast can offer Undo.
    pub trashed: Vec<PathBuf>,
    pub cancelled: bool,
}

pub(crate) struct Job {
    pub kind: JobKind,
    pub progress: Arc<Mutex<Prog>>,
    pub cancel: Arc<AtomicBool>,
    pub finished: Arc<AtomicBool>,
    pub result: Arc<Mutex<Option<JobResult>>>,
}

pub(crate) enum Dialog {
    Rename { path: PathBuf, text: String },
    NewFolder { text: String },
    ConfirmForever { paths: Vec<PathBuf> },
    Details { entry: Entry, stats: Option<Receiver<(u64, usize)>>, result: Option<(u64, usize)> },
    Sort,
    Problems(Vec<String>),
}

struct ThumbJob {
    path: PathBuf,
    mtime: SystemTime,
    size: u64,
    video: bool,
}

struct ThumbShared {
    jobs: Mutex<Vec<ThumbJob>>,
    cv: Condvar,
    cache_dir: PathBuf,
}

pub struct Files {
    pub(crate) ctx: Context,
    pub(crate) root: PathBuf,
    pub(crate) trash_dir: PathBuf,
    data_dir: PathBuf,
    pub(crate) stack: Vec<Screen>,
    pub(crate) entries: Vec<Entry>,
    pub(crate) list_error: Option<String>,
    pub(crate) sort: Sort,
    pub(crate) show_hidden: bool,
    // storage scan
    pub(crate) scan: Option<Arc<Scan>>,
    scan_rx: Option<Receiver<Scan>>,
    scan_cancel: Arc<AtomicBool>,
    last_scan: Instant,
    pub(crate) storage: Option<(u64, u64)>,
    // selection
    pub(crate) select: Option<HashSet<PathBuf>>,
    pub(crate) paint: Option<(bool, Option<Pos2>)>,
    pub(crate) swallow_click: bool,
    pub(crate) list_scroll_to: Option<f32>,
    pub(crate) bar_top: f32,
    pub(crate) list_top: f32,
    pub(crate) list_offset: f32,
    pub(crate) more_menu: bool,
    pub(crate) header_menu: bool,
    // clipboard, jobs, dialogs
    pub(crate) clip: Option<(Vec<PathBuf>, bool)>,
    pub(crate) job: Option<Job>,
    pub(crate) dialog: Option<Dialog>,
    pub(crate) toast: Option<(String, Vec<PathBuf>, f64)>,
    // thumbnails
    pub(crate) thumbs: HashMap<PathBuf, TextureHandle>,
    pub(crate) thumb_failed: HashSet<PathBuf>,
    thumb_requested: HashSet<PathBuf>,
    thumb_shared: Arc<ThumbShared>,
    thumb_rx: Receiver<(PathBuf, Option<ColorImage>)>,
    // search
    pub(crate) search_text: String,
    pub(crate) search_hits: Arc<Mutex<Vec<Entry>>>,
    pub(crate) searching: Arc<AtomicBool>,
    search_cancel: Arc<AtomicBool>,
    // access to all files
    pub(crate) access_ok: bool,
    last_access_check: Instant,
    pub(crate) insets: (f32, f32),
    was_focused: bool,
    started: Instant,
    first_frame_logged: bool,
    /// Desktop preview: FILES_START=folder:<name>|category:image|trash|settings|select
    start: Option<String>,
    #[cfg(target_os = "android")]
    pub(crate) android: Option<android_activity::AndroidApp>,
}

#[cfg(target_os = "android")]
const FALLBACK_INSETS: (f32, f32) = (30.0, 36.0);

/// The window loop's context, so the destroy hook (called from Java on the main thread) can end it.
#[cfg(target_os = "android")]
static CTX: Mutex<Option<Context>> = Mutex::new(None);

/// Called by `FilesActivity.onDestroy` just before Android waits for the app thread to stop. The
/// windowing library ignores Android's destroy notice, so ask the window to close here.
#[cfg(target_os = "android")]
extern "system" fn native_destroying(_env: jni::JNIEnv, _class: jni::objects::JClass) {
    diag::log("activity is being destroyed: closing the window");
    if let Some(ctx) = CTX.lock().ok().and_then(|g| g.clone()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }
}

fn decode(path: &Path, max: u32) -> Option<ColorImage> {
    let mut dec = ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_decoder().ok()?;
    let orientation = dec.orientation().ok();
    let mut img = DynamicImage::from_decoder(dec).ok()?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    if img.width().max(img.height()) > max {
        img = img.thumbnail(max, max);
    }
    let rgba = img.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

fn spawn_thumb_workers(shared: Arc<ThumbShared>, tx: Sender<(PathBuf, Option<ColorImage>)>, ctx: Context) {
    for _ in 0..2 {
        let (shared, tx, ctx) = (shared.clone(), tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            loop {
                let job = {
                    let mut q = shared.jobs.lock().unwrap();
                    while q.is_empty() {
                        q = shared.cv.wait(q).unwrap();
                    }
                    q.pop().unwrap()
                };
                let key = thumbcache::key(&job.path, job.mtime, job.size);
                let image = if let Some((img, _)) = thumbcache::load(&shared.cache_dir, key) {
                    Some(img)
                } else if job.video {
                    media::thumbnail(&job.path, THUMB_PX).map(|t| {
                        thumbcache::store(&shared.cache_dir, key, &t.image, Some(t.duration_ms));
                        t.image
                    })
                } else {
                    decode(&job.path, THUMB_PX).inspect(|img| thumbcache::store(&shared.cache_dir, key, img, None))
                };
                let _ = tx.send((job.path, image));
                ctx.request_repaint();
            }
        });
    }
}

fn data_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        PathBuf::from("/sdcard/Android/data/dev.heartnetapps.files/files")
    } else {
        std::env::temp_dir().join("files-data")
    }
}

fn start_scan(root: PathBuf, trash_dir: PathBuf, cancel: Arc<AtomicBool>, ctx: Context) -> Receiver<Scan> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let t = Instant::now();
        let scan = fsops::scan_storage(&root, &trash_dir, &cancel);
        diag::log(&format!("storage scan: {} files in {} ms", scan.files.len(), t.elapsed().as_millis()));
        let _ = tx.send(scan);
        ctx.request_repaint();
    });
    rx
}

impl Files {
    pub fn new(cc: &eframe::CreationContext<'_>, root: Option<PathBuf>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(Color32::from_gray(235));
        visuals.selection.bg_fill = BRAND;
        cc.egui_ctx.set_visuals(visuals);
        let root = root.unwrap_or_else(|| if cfg!(target_os = "android") { PathBuf::from("/sdcard") } else { std::env::temp_dir().join("files-demo") });
        let trash_dir = root.join(".FilesTrash");
        let data_dir = data_dir();
        diag::init(data_dir.clone());
        diag::log(&format!("---- start, version {}", env!("CARGO_PKG_VERSION")));

        let (sort, show_hidden) = load_settings(&data_dir);
        let thumb_shared = Arc::new(ThumbShared { jobs: Mutex::new(Vec::new()), cv: Condvar::new(), cache_dir: data_dir.join("thumbs") });
        let (thumb_tx, thumb_rx) = channel();
        spawn_thumb_workers(thumb_shared.clone(), thumb_tx, cc.egui_ctx.clone());
        let scan_cancel = Arc::new(AtomicBool::new(false));
        let scan_rx = Some(start_scan(root.clone(), trash_dir.clone(), scan_cancel.clone(), cc.egui_ctx.clone()));
        purge_old_trash(&trash_dir);

        let mut me = Self {
            ctx: cc.egui_ctx.clone(),
            storage: storage_numbers(&root),
            root,
            trash_dir,
            data_dir,
            stack: vec![Screen::Home],
            entries: Vec::new(),
            list_error: None,
            sort,
            show_hidden,
            scan: None,
            scan_rx,
            scan_cancel,
            last_scan: Instant::now(),
            select: None,
            paint: None,
            swallow_click: false,
            list_scroll_to: None,
            bar_top: f32::MAX,
            list_top: 0.0,
            list_offset: 0.0,
            more_menu: false,
            header_menu: false,
            clip: None,
            job: None,
            dialog: None,
            toast: None,
            thumbs: HashMap::new(),
            thumb_failed: HashSet::new(),
            thumb_requested: HashSet::new(),
            thumb_shared,
            thumb_rx,
            search_text: String::new(),
            search_hits: Arc::new(Mutex::new(Vec::new())),
            searching: Arc::new(AtomicBool::new(false)),
            search_cancel: Arc::new(AtomicBool::new(false)),
            access_ok: true,
            last_access_check: Instant::now(),
            insets: (0.0, 0.0),
            was_focused: true,
            started: Instant::now(),
            first_frame_logged: false,
            start: std::env::var("FILES_START").ok(),
            #[cfg(target_os = "android")]
            android: None,
        };
        me.apply_start();
        me
    }

    #[cfg(target_os = "android")]
    pub fn with_android(mut self, app: android_activity::AndroidApp) -> Self {
        *CTX.lock().unwrap() = Some(self.ctx.clone());
        let hooked = perm::register_destroy_hook(&app, native_destroying);
        diag::log(&format!("destroy hook registered: {hooked}"));
        media::init(&app);
        self.access_ok = perm::has_all_files_access(&app);
        self.android = Some(app);
        self
    }

    /// Desktop preview helper: jump straight onto a screen so it can be screenshotted.
    fn apply_start(&mut self) {
        let Some(start) = self.start.take() else { return };
        match start.split_once(':').unwrap_or((start.as_str(), "")) {
            ("folder", name) => {
                let p = if name.is_empty() { self.root.clone() } else { self.root.join(name) };
                self.go(Screen::Folder(p));
            }
            ("category", "image") => self.go(Screen::Category(Kind::Image)),
            ("category", "video") => self.go(Screen::Category(Kind::Video)),
            ("recent", _) => self.go(Screen::Recent),
            ("large", _) => self.go(Screen::Large),
            ("trash", _) => self.go(Screen::Trash),
            ("settings", _) => self.go(Screen::Settings),
            ("select", name) => {
                let p = if name.is_empty() { self.root.clone() } else { self.root.join(name) };
                self.go(Screen::Folder(p));
                self.select = Some(self.entries.iter().skip(1).take(3).map(|e| e.path.clone()).collect());
            }
            _ => {}
        }
    }

    pub(crate) fn current(&self) -> &Screen {
        self.stack.last().unwrap_or(&Screen::Home)
    }

    /// The folder whose contents are showing, if the current screen is a folder.
    pub(crate) fn cwd(&self) -> Option<&Path> {
        match self.current() {
            Screen::Folder(p) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn go(&mut self, screen: Screen) {
        self.exit_select();
        self.stack.push(screen);
        self.load_current();
    }

    /// One step back through the screens visited. False when already at the first one.
    pub(crate) fn back(&mut self) -> bool {
        if self.stack.len() > 1 {
            self.exit_select();
            self.search_cancel.store(true, Ordering::Relaxed);
            self.stack.pop();
            self.load_current();
            true
        } else {
            false
        }
    }

    pub(crate) fn exit_select(&mut self) {
        self.select = None;
        self.paint = None;
        self.more_menu = false;
        self.header_menu = false;
    }

    /// Fill `entries` for the current screen.
    pub(crate) fn load_current(&mut self) {
        self.thumb_shared.jobs.lock().unwrap().clear();
        self.thumb_requested.clear();
        self.list_error = None;
        self.list_scroll_to = Some(0.0);
        let screen = self.current().clone();
        self.entries = match &screen {
            Screen::Home | Screen::Settings => Vec::new(),
            Screen::Folder(dir) => match fsops::list_dir(dir, self.show_hidden) {
                Ok(mut v) => {
                    fsops::sort_entries(&mut v, self.sort);
                    v
                }
                Err(e) => {
                    self.list_error = Some(if e.kind() == std::io::ErrorKind::PermissionDenied {
                        "Android does not let apps open this folder.".to_string()
                    } else {
                        format!("Can't open this folder: {e}")
                    });
                    Vec::new()
                }
            },
            Screen::Category(kind) => self.scan_list(|e| e.kind == *kind, false),
            Screen::Recent => {
                let mut v = self.scan_list(|e| !e.name.starts_with('.'), false);
                v.truncate(LIST_LIMIT);
                v
            }
            Screen::Large => {
                let mut v = self.scan_list(|_| true, true);
                v.truncate(LIST_LIMIT);
                v
            }
            Screen::Search(_) => Vec::new(),
            Screen::Trash => self.trash_entries(),
        };
    }

    fn scan_list(&self, keep: impl Fn(&Entry) -> bool, by_size: bool) -> Vec<Entry> {
        let Some(scan) = &self.scan else { return Vec::new() };
        let mut v: Vec<Entry> = scan.files.iter().filter(|e| keep(e) && !e.name.starts_with('.')).cloned().collect();
        if by_size {
            v.sort_by_key(|e| std::cmp::Reverse(e.size));
        } else {
            v.sort_by_key(|e| std::cmp::Reverse(e.mtime));
        }
        v
    }

    /// The trash, shown with the original names (the trashed files are named `<time>_<encoded path>`).
    fn trash_entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.trash_dir) else { return out };
        for e in rd.flatten() {
            let path = e.path();
            let Some(orig) = trash::original_of(&path) else { continue };
            let Some(mut entry) = Entry::from_path(&path) else { continue };
            entry.name = orig.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            entry.kind = fsops::kind_of(&orig, entry.is_dir);
            out.push(entry);
        }
        out.sort_by_key(|e| std::cmp::Reverse(trash::trashed_at(&e.path).unwrap_or(0)));
        out
    }

    pub(crate) fn trash_count(&self) -> usize {
        std::fs::read_dir(&self.trash_dir).map(|rd| rd.flatten().filter(|e| trash::original_of(&e.path()).is_some()).count()).unwrap_or(0)
    }

    pub(crate) fn reload_after_change(&mut self) {
        self.load_current();
        self.storage = storage_numbers(&self.root);
        self.rescan();
    }

    pub(crate) fn rescan(&mut self) {
        self.scan_cancel.store(true, Ordering::Relaxed);
        self.scan_cancel = Arc::new(AtomicBool::new(false));
        self.last_scan = Instant::now();
        self.scan_rx = Some(start_scan(self.root.clone(), self.trash_dir.clone(), self.scan_cancel.clone(), self.ctx.clone()));
    }

    pub(crate) fn save_settings(&self) {
        let key = match self.sort.key {
            SortKey::Name => "name",
            SortKey::Date => "date",
            SortKey::Size => "size",
            SortKey::Kind => "kind",
        };
        let lines = [format!("sort={key}"), format!("desc={}", u8::from(self.sort.descending)), format!("hidden={}", u8::from(self.show_hidden))];
        let _ = std::fs::create_dir_all(&self.data_dir);
        let _ = std::fs::write(self.data_dir.join("settings.txt"), lines.join("\n"));
    }

    pub(crate) fn say(&mut self, msg: impl Into<String>, undo: Vec<PathBuf>) {
        let deadline = self.ctx.input(|i| i.time) + UNDO_SECS;
        self.toast = Some((msg.into(), undo, deadline));
        self.ctx.request_repaint_after(Duration::from_millis(250));
    }

    // ---------- thumbnails ----------

    pub(crate) fn want_thumb(&mut self, e: &Entry) {
        if self.thumbs.contains_key(&e.path) || self.thumb_failed.contains(&e.path) || !matches!(e.kind, Kind::Image | Kind::Video) {
            return;
        }
        if !self.thumb_requested.insert(e.path.clone()) {
            return;
        }
        self.thumb_shared.jobs.lock().unwrap().push(ThumbJob { path: e.path.clone(), mtime: e.mtime, size: e.size, video: e.kind == Kind::Video });
        self.thumb_shared.cv.notify_one();
    }

    fn drain(&mut self, ctx: &Context) {
        while let Ok((path, image)) = self.thumb_rx.try_recv() {
            self.thumb_requested.remove(&path);
            match image {
                Some(img) => {
                    let tex = ctx.load_texture(format!("t{}", self.thumbs.len()), img, TextureOptions::LINEAR);
                    self.thumbs.insert(path, tex);
                }
                None => {
                    self.thumb_failed.insert(path);
                }
            }
        }
        if let Some(rx) = &self.scan_rx {
            if let Ok(scan) = rx.try_recv() {
                self.scan = Some(Arc::new(scan));
                self.scan_rx = None;
                if matches!(self.current(), Screen::Category(_) | Screen::Recent | Screen::Large) {
                    let keep_scroll = self.list_scroll_to;
                    self.load_current();
                    self.list_scroll_to = keep_scroll;
                }
            }
        }
        if self.thumbs.len() > 600 {
            self.thumbs.clear(); // they are cheap to bring back from the disk cache
            self.thumb_requested.clear();
        }
    }

    // ---------- jobs ----------

    pub(crate) fn start_job(&mut self, kind: JobKind, sources: Vec<PathBuf>, dest: Option<PathBuf>) {
        if self.job.is_some() || sources.is_empty() {
            return;
        }
        let progress = Arc::new(Mutex::new(Prog { items: sources.len(), ..Default::default() }));
        let (cancel, finished, result) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None)));
        let job = Job { kind, progress: progress.clone(), cancel: cancel.clone(), finished: finished.clone(), result: result.clone() };
        let (root, trash_dir, ctx) = (self.root.clone(), self.trash_dir.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let res = run_job(kind, &sources, dest.as_deref(), &root, &trash_dir, &progress, &cancel);
            *result.lock().unwrap() = Some(res);
            finished.store(true, Ordering::SeqCst);
            ctx.request_repaint();
        });
        self.job = Some(job);
        self.exit_select();
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        if !job.finished.load(Ordering::SeqCst) {
            self.ctx.request_repaint_after(Duration::from_millis(120));
            return;
        }
        let kind = job.kind;
        let res = job.result.lock().unwrap().take().unwrap_or_default();
        self.job = None;
        let n = res.ok;
        let plural = if n == 1 { "item" } else { "items" };
        let msg = if res.cancelled {
            format!("Cancelled after {n} {plural}")
        } else if res.failed.is_empty() {
            match kind {
                JobKind::Copy => format!("Copied {n} {plural}"),
                JobKind::Move => format!("Moved {n} {plural}"),
                JobKind::Trash => format!("Moved {n} {plural} to the trash"),
                JobKind::Restore => format!("Restored {n} {plural}"),
                JobKind::DeleteForever => format!("Deleted {n} {plural} for good"),
            }
        } else {
            format!("{n} done, {} could not be done", res.failed.len())
        };
        if !res.failed.is_empty() {
            self.dialog = Some(Dialog::Problems(res.failed.clone()));
        }
        diag::log(&format!("job {kind:?}: {n} ok, {} failed", res.failed.len()));
        let undo = if kind == JobKind::Trash { res.trashed.clone() } else { Vec::new() };
        self.notify_media(&res.touched);
        self.clip = if kind == JobKind::Move || kind == JobKind::Copy { None } else { self.clip.take() };
        self.reload_after_change();
        self.say(msg, undo);
    }

    pub(crate) fn notify_media(&self, _paths: &[String]) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android {
            perm::media_scan(app, _paths);
        }
    }

    // ---------- opening ----------

    pub(crate) fn open_entry(&mut self, e: &Entry) {
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            if !perm::open_file(&app, &e.path.to_string_lossy(), false) {
                self.say("No app on this phone can open that kind of file.", Vec::new());
            }
            return;
        }
        self.say(format!("Would open {}", e.name), Vec::new());
    }

    pub(crate) fn share(&mut self, paths: &[PathBuf]) {
        let files: Vec<String> = paths.iter().filter(|p| p.is_file()).take(200).map(|p| p.to_string_lossy().into_owned()).collect();
        if files.is_empty() {
            self.say("Folders can't be shared. Pick files.", Vec::new());
            return;
        }
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            if !perm::share_files(&app, &files) {
                self.say("Could not open the share sheet.", Vec::new());
            }
            return;
        }
        self.say(format!("Would share {} file(s)", files.len()), Vec::new());
    }

    /// Quit by closing the window loop while the activity is still on screen; when `android_main`
    /// returns, the glue finishes the activity itself. Asking Android to finish first leaves the
    /// process stuck (see FilesActivity).
    pub(crate) fn exit(&self, ctx: &Context) {
        diag::log("quitting");
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }

    pub(crate) fn rename(&mut self, path: &Path, typed: &str) {
        let Some(name) = fsops::sanitize_name(typed) else {
            self.say("That name can't be used.", Vec::new());
            return;
        };
        let new = path.with_file_name(&name);
        if new == path {
            return;
        }
        if new.exists() {
            self.say("Something with that name already exists here.", Vec::new());
            return;
        }
        match std::fs::rename(path, &new) {
            Ok(()) => {
                self.notify_media(&[path.to_string_lossy().into_owned(), new.to_string_lossy().into_owned()]);
                self.reload_after_change();
                self.say(format!("Renamed to {name}"), Vec::new());
            }
            Err(e) => self.say(format!("Could not rename: {e}"), Vec::new()),
        }
    }

    pub(crate) fn new_folder(&mut self, typed: &str) {
        let Some(dir) = self.cwd().map(Path::to_path_buf) else { return };
        let Some(name) = fsops::sanitize_name(typed) else {
            self.say("That name can't be used.", Vec::new());
            return;
        };
        let path = dir.join(&name);
        if path.exists() {
            self.say("Something with that name already exists here.", Vec::new());
            return;
        }
        match std::fs::create_dir(&path) {
            Ok(()) => {
                self.load_current();
                self.say(format!("Created {name}"), Vec::new());
            }
            Err(e) => self.say(format!("Could not create the folder: {e}"), Vec::new()),
        }
    }

    pub(crate) fn start_search(&mut self, query: String) {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return;
        }
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_cancel = Arc::new(AtomicBool::new(false));
        self.search_hits.lock().unwrap().clear();
        self.searching.store(true, Ordering::SeqCst);
        // search from the folder being looked at, or the whole storage from anywhere else
        let from = self.cwd().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone());
        let (hits, searching, cancel, ctx, hidden) = (self.search_hits.clone(), self.searching.clone(), self.search_cancel.clone(), self.ctx.clone(), self.show_hidden);
        std::thread::spawn(move || {
            fsops::search(&from, &q, hidden, 500, &cancel, &mut |e| {
                hits.lock().unwrap().push(e);
                ctx.request_repaint();
            });
            searching.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
        if !matches!(self.current(), Screen::Search(_)) {
            self.exit_select();
            self.stack.push(Screen::Search(query));
        } else if let Some(s) = self.stack.last_mut() {
            *s = Screen::Search(query);
        }
        self.entries.clear();
    }

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

    fn check_access(&mut self) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android {
            if self.last_access_check.elapsed() < Duration::from_millis(800) {
                return;
            }
            self.last_access_check = Instant::now();
            let now = perm::has_all_files_access(app);
            if now && !self.access_ok {
                self.reload_after_change();
            }
            self.access_ok = now;
            if !now {
                self.ctx.request_repaint_after(Duration::from_millis(800));
            }
        }
    }
}

fn run_job(kind: JobKind, sources: &[PathBuf], dest: Option<&Path>, root: &Path, trash_dir: &Path, progress: &Arc<Mutex<Prog>>, cancel: &Arc<AtomicBool>) -> JobResult {
    let mut res = JobResult::default();
    if matches!(kind, JobKind::Copy | JobKind::Move) {
        let (bytes, _) = fsops::tree_size(sources, cancel);
        progress.lock().unwrap().total = bytes;
    }
    for (i, src) in sources.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            res.cancelled = true;
            break;
        }
        {
            let mut p = progress.lock().unwrap();
            p.item = i + 1;
            p.current = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        }
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let protected = if kind == JobKind::Copy || kind == JobKind::Restore { None } else if kind == JobKind::DeleteForever { None } else { fsops::protected_reason(src, root, trash_dir) };
        if let Some(why) = protected {
            res.failed.push(format!("{name}: {why}"));
            continue;
        }
        // pictures and videos at the old place, listed before anything moves
        let before = if matches!(kind, JobKind::Move | JobKind::Trash) { media_files_under(src, 300) } else { Vec::new() };
        let mut report = |n: u64, cur: &Path| {
            let mut p = progress.lock().unwrap();
            p.done += n;
            p.current = cur.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            !cancel.load(Ordering::Relaxed)
        };
        let outcome: std::io::Result<Option<PathBuf>> = match kind {
            JobKind::Copy => fsops::copy_into(src, dest.unwrap_or(root), &mut report).map(Some),
            JobKind::Move => fsops::move_into(src, dest.unwrap_or(root), &mut report).map(Some),
            JobKind::Trash => trash::move_to_trash(src, trash_dir).map(|t| {
                res.trashed.push(t.clone());
                Some(t)
            }),
            JobKind::Restore => trash::restore(src, trash_dir).map(Some),
            JobKind::DeleteForever => trash::delete_forever(src, trash_dir).map(|()| None),
        };
        match outcome {
            Ok(new) => {
                res.ok += 1;
                // media files in the old and new places, for Android's media database
                res.touched.extend(before);
                if let Some(n) = new {
                    if kind != JobKind::Trash {
                        res.touched.extend(media_files_under(&n, 300));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                res.cancelled = true;
                break;
            }
            Err(e) => res.failed.push(format!("{name}: {e}")),
        }
    }
    res
}

/// Picture, video and audio files at or under `path`, for the media database (at most `limit`).
fn media_files_under(path: &Path, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    stack.extend(rd.flatten().map(|e| e.path()));
                }
            }
            Ok(_) => {
                if matches!(fsops::kind_of(&p, false), Kind::Image | Kind::Video | Kind::Audio) {
                    out.push(p.to_string_lossy().into_owned());
                }
            }
            Err(_) => {}
        }
    }
    out
}

fn load_settings(dir: &Path) -> (Sort, bool) {
    let text = std::fs::read_to_string(dir.join("settings.txt")).unwrap_or_default();
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().to_string());
    let key = match get("sort=").as_deref() {
        Some("date") => SortKey::Date,
        Some("size") => SortKey::Size,
        Some("kind") => SortKey::Kind,
        _ => SortKey::Name,
    };
    (Sort { key, descending: get("desc=").as_deref() == Some("1") }, get("hidden=").as_deref() == Some("1"))
}

/// Trashed items older than TRASH_DAYS are erased.
fn purge_old_trash(trash_dir: &Path) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let Ok(rd) = std::fs::read_dir(trash_dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        if trash::trashed_at(&path).is_some_and(|t| now.saturating_sub(t) > TRASH_DAYS * 86_400) {
            let _ = trash::delete_forever(&path, trash_dir);
        }
    }
}

/// (total, free) bytes of the storage holding `root`.
#[cfg(unix)]
fn storage_numbers(root: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(root.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs only writes into the zeroed struct we pass.
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            return None;
        }
        let unit = s.f_frsize as u64;
        Some((s.f_blocks as u64 * unit, s.f_bavail as u64 * unit))
    }
}

#[cfg(not(unix))]
fn storage_numbers(_root: &Path) -> Option<(u64, u64)> {
    Some((256 * 1024 * 1024 * 1024, 91 * 1024 * 1024 * 1024)) // a stand-in for the desktop preview
}

impl eframe::App for Files {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            diag::log(&format!("first frame after {} ms", self.started.elapsed().as_millis()));
        }
        self.drain(&ctx);
        self.poll_job();
        self.check_access();
        // coming back to the front: look again, in case something changed meanwhile
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused {
            self.reload_after_change();
            ctx.request_repaint();
        }
        self.was_focused = focused;
        let (top, bottom) = self.insets(&ctx);
        self.insets = (top, bottom);
        ui::draw(self, ui, &ctx, top, bottom);
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
    let handle = app.clone();
    let options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
    if let Err(e) = eframe::run_native("Files", options, Box::new(move |cc| Ok(Box::new(Files::new(cc, None).with_android(handle.clone()))))) {
        diag::log(&format!("eframe exited with an error: {e}"));
        log::error!("files exited: {e}");
    }
    diag::log("android_main returning");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("files-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn run(kind: JobKind, sources: &[PathBuf], dest: Option<&Path>, root: &Path) -> JobResult {
        let trash_dir = root.join(".FilesTrash");
        run_job(kind, sources, dest, root, &trash_dir, &Arc::new(Mutex::new(Prog::default())), &Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn trashing_and_restoring_a_folder_through_jobs() {
        let root = temp("jobs");
        std::fs::create_dir_all(root.join("Trip/day1")).unwrap();
        std::fs::write(root.join("Trip/day1/a.jpg"), b"a").unwrap();
        let r = run(JobKind::Trash, &[root.join("Trip")], None, &root);
        assert_eq!((r.ok, r.failed.len(), r.trashed.len()), (1, 0, 1));
        assert!(!root.join("Trip").exists());
        assert_eq!(r.touched.len(), 1, "the picture inside is reported for the media database");
        let r2 = run(JobKind::Restore, &r.trashed, None, &root);
        assert_eq!(r2.ok, 1);
        assert_eq!(std::fs::read(root.join("Trip/day1/a.jpg")).unwrap(), b"a");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_storage_root_and_android_folders_cannot_be_trashed_or_moved() {
        let root = temp("protect");
        std::fs::create_dir_all(root.join("Android/data")).unwrap();
        for target in [root.clone(), root.join("Android"), root.join("Android/data")] {
            let r = run(JobKind::Trash, &[target.clone()], None, &root);
            assert_eq!(r.ok, 0, "{target:?}");
            assert_eq!(r.failed.len(), 1);
            assert!(target.exists());
        }
        let r = run(JobKind::Move, &[root.join("Android")], Some(&root.join("x")), &root);
        assert_eq!(r.ok, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn copying_and_moving_between_folders() {
        let root = temp("cm");
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("a/one.txt"), b"1").unwrap();
        std::fs::write(root.join("a/two.txt"), b"2").unwrap();
        let r = run(JobKind::Copy, &[root.join("a/one.txt"), root.join("a/two.txt")], Some(&root.join("b")), &root);
        assert_eq!(r.ok, 2);
        assert!(root.join("a/one.txt").exists() && root.join("b/one.txt").exists());
        let r = run(JobKind::Move, &[root.join("a/two.txt")], Some(&root.join("b")), &root);
        assert_eq!(r.ok, 1);
        assert!(!root.join("a/two.txt").exists());
        assert!(root.join("b/two (1).txt").exists(), "an existing name is never overwritten");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cancelled_job_reports_it() {
        let root = temp("cancel");
        std::fs::write(root.join("big.bin"), vec![0u8; 3_000_000]).unwrap();
        let trash_dir = root.join(".FilesTrash");
        let cancel = Arc::new(AtomicBool::new(true));
        let r = run_job(JobKind::Copy, &[root.join("big.bin")], Some(&root.join("out")), &root, &trash_dir, &Arc::new(Mutex::new(Prog::default())), &cancel);
        assert!(r.cancelled);
        assert_eq!(r.ok, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn settings_round_trip() {
        let dir = temp("settings");
        std::fs::write(dir.join("settings.txt"), "sort=size\ndesc=1\nhidden=1").unwrap();
        let (sort, hidden) = load_settings(&dir);
        assert_eq!(sort, Sort { key: SortKey::Size, descending: true });
        assert!(hidden);
        let (sort, hidden) = load_settings(&dir.join("none"));
        assert_eq!(sort, Sort::default());
        assert!(!hidden);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_trash_is_purged_and_recent_trash_kept() {
        let root = temp("purge");
        let trash_dir = root.join(".FilesTrash");
        std::fs::create_dir_all(&trash_dir).unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        std::fs::write(trash_dir.join(format!("{}_%2Fsdcard%2Fold.txt", now - 40 * 86_400)), "old").unwrap();
        std::fs::write(trash_dir.join(format!("{}_%2Fsdcard%2Fnew.txt", now - 86_400)), "new").unwrap();
        purge_old_trash(&trash_dir);
        let left: Vec<_> = std::fs::read_dir(&trash_dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left.len(), 1);
        assert!(left[0].contains("new.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
