//! A small diagnostic log, written next to the app's other data (`diag.log`), so that a problem
//! that happens now and then, such as a blank screen on opening, leaves a trace to read afterwards.
//! It records starts, resumes, the first frame drawn after each, scans and any panic. It never
//! records file names.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
const MAX_BYTES: u64 = 120 * 1024;

pub fn init(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("diag.log");
    // keep the previous run's log as diag.old so a restart does not erase the evidence
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        let _ = std::fs::rename(&path, dir.join("diag.old"));
    }
    *FILE.lock().unwrap() = Some(path);
    // the release build aborts on a panic, so the message has to be written before that happens
    std::panic::set_hook(Box::new(|info| log(&format!("PANIC: {info}"))));
}

pub fn log(msg: &str) {
    let Some(path) = FILE.lock().ok().and_then(|g| g.clone()) else { return };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let line = format!("{:02}:{:02}:{:02}.{:03} {msg}\n", secs / 3600 % 24, secs / 60 % 60, secs % 60, now.subsec_millis());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}
