//! File operations: listing, sorting, copying, moving, searching and a whole-storage scan. All plain
//! `std::fs`, no UI, so everything here is tested against real temporary folders.

use std::cmp::Ordering as Cmp;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    Folder,
    Image,
    Video,
    Audio,
    Document,
    Archive,
    Apk,
    Other,
}

impl Kind {
    pub const CATEGORIES: [Kind; 6] = [Kind::Image, Kind::Video, Kind::Audio, Kind::Document, Kind::Archive, Kind::Apk];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Folder => "Folders",
            Kind::Image => "Images",
            Kind::Video => "Videos",
            Kind::Audio => "Audio",
            Kind::Document => "Documents",
            Kind::Archive => "Archives",
            Kind::Apk => "Apps (APK)",
            Kind::Other => "Other",
        }
    }
}

const IMAGE: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "heic", "heif", "avif", "dng", "svg"];
const VIDEO: &[&str] = &["mp4", "mkv", "webm", "3gp", "3g2", "mov", "m4v", "ts", "m2ts", "mts", "avi", "mpg", "mpeg", "wmv", "flv", "ogv"];
const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "amr", "mid", "midi"];
const DOCUMENT: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "rtf", "odt", "ods", "odp", "csv", "md", "json", "xml", "html", "htm", "epub", "log"];
const ARCHIVE: &[&str] = &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz"];
const APK: &[&str] = &["apk", "apks", "xapk", "apkm"];

pub fn kind_of(path: &Path, is_dir: bool) -> Kind {
    if is_dir {
        return Kind::Folder;
    }
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let ext = ext.as_str();
    if IMAGE.contains(&ext) {
        Kind::Image
    } else if VIDEO.contains(&ext) {
        Kind::Video
    } else if AUDIO.contains(&ext) {
        Kind::Audio
    } else if DOCUMENT.contains(&ext) {
        Kind::Document
    } else if ARCHIVE.contains(&ext) {
        Kind::Archive
    } else if APK.contains(&ext) {
        Kind::Apk
    } else {
        Kind::Other
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: SystemTime,
    pub kind: Kind,
}

impl Entry {
    pub fn from_path(path: &Path) -> Option<Entry> {
        let meta = std::fs::metadata(path).ok()?;
        let is_dir = meta.is_dir();
        Some(Entry {
            path: path.to_path_buf(),
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned()),
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
            mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            kind: kind_of(path, is_dir),
        })
    }
}

/// What is directly inside `dir`. Hidden (dot) entries only when asked for.
pub fn list_dir(dir: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        if !show_hidden && e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if let Some(entry) = Entry::from_path(&e.path()) {
            out.push(entry);
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Date,
    Size,
    Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Sort { key: SortKey::Name, descending: false }
    }
}

/// Case-insensitive, with runs of digits compared as numbers ("file2" before "file10").
pub fn natural_cmp(a: &str, b: &str) -> Cmp {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Cmp::Equal,
            (None, Some(_)) => return Cmp::Less,
            (Some(_), None) => return Cmp::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut nx = String::new();
                while let Some(&c) = ai.peek().filter(|c| c.is_ascii_digit()) {
                    nx.push(c);
                    ai.next();
                }
                let mut ny = String::new();
                while let Some(&c) = bi.peek().filter(|c| c.is_ascii_digit()) {
                    ny.push(c);
                    bi.next();
                }
                let (tx, ty) = (nx.trim_start_matches('0'), ny.trim_start_matches('0'));
                let ord = tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty));
                if ord != Cmp::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Cmp::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// Folders always come first; inside each group the chosen order applies.
pub fn sort_entries(v: &mut [Entry], sort: Sort) {
    v.sort_by(|a, b| {
        let groups = b.is_dir.cmp(&a.is_dir); // folders first
        if groups != Cmp::Equal {
            return groups;
        }
        let ord = match sort.key {
            SortKey::Name => natural_cmp(&a.name, &b.name),
            SortKey::Date => a.mtime.cmp(&b.mtime).then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Size => a.size.cmp(&b.size).then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Kind => {
                let ext = |e: &Entry| e.path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                ext(a).cmp(&ext(b)).then_with(|| natural_cmp(&a.name, &b.name))
            }
        };
        if sort.descending { ord.reverse() } else { ord }
    });
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.2} {}", UNITS[u])
    }
}

/// Why something must not be moved or deleted, or None when it may be.
pub fn protected_reason(path: &Path, storage_root: &Path, trash_dir: &Path) -> Option<&'static str> {
    if path == storage_root {
        return Some("This is the whole internal storage.");
    }
    if !path.starts_with(storage_root) {
        return Some("This is outside the phone's shared storage.");
    }
    if path == trash_dir || path.starts_with(trash_dir) {
        return Some("This is the trash folder itself.");
    }
    let android = storage_root.join("Android");
    if path == android || path == android.join("data") || path == android.join("obb") || path == android.join("media") {
        return Some("Android needs this folder.");
    }
    None
}

/// `dir/name`, or `dir/name (1).ext`, `(2)`… when taken, so nothing is overwritten.
pub fn unique_dest(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|c| !c.exists())
        .expect("an unused name exists")
}

/// A name typed by the user, made safe for one path component.
pub fn sanitize_name(s: &str) -> Option<String> {
    let cleaned: String = s
        .trim()
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty() && cleaned != "." && cleaned != "..").then_some(cleaned.chars().take(200).collect())
}

/// Total bytes and number of files under these paths.
pub fn tree_size(paths: &[PathBuf], cancel: &AtomicBool) -> (u64, usize) {
    let (mut bytes, mut count) = (0u64, 0usize);
    let mut stack: Vec<PathBuf> = paths.to_vec();
    while let Some(p) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if meta.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        } else {
            bytes += meta.len();
            count += 1;
        }
    }
    (bytes, count)
}

/// Called with the number of bytes just written and the file being worked on; returning false cancels.
pub type Report<'a> = &'a mut dyn FnMut(u64, &Path) -> bool;

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

fn copy_file(src: &Path, dest: &Path, report: Report) -> io::Result<()> {
    let mut input = std::fs::File::open(src)?;
    let mut output = std::fs::File::create(dest)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n])?;
        if !report(n as u64, src) {
            drop(output);
            let _ = std::fs::remove_file(dest);
            return Err(cancelled());
        }
    }
    output.flush()?;
    // the copy keeps the date of the original
    if let Ok(m) = std::fs::metadata(src).and_then(|m| m.modified()) {
        let _ = output.set_modified(m);
    }
    Ok(())
}

fn copy_tree(src: &Path, dest: &Path, report: Report) -> io::Result<()> {
    let meta = std::fs::metadata(src)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dest.join(e.file_name()), report)?;
        }
        Ok(())
    } else {
        copy_file(src, dest, report)
    }
}

/// Copy a file or folder into `dest_dir` under a name that is free. Returns the new path.
pub fn copy_into(src: &Path, dest_dir: &Path, report: Report) -> io::Result<PathBuf> {
    if dest_dir.starts_with(src) {
        return Err(io::Error::other("a folder cannot be copied into itself"));
    }
    std::fs::create_dir_all(dest_dir)?;
    let dest = unique_dest(dest_dir, src.file_name().ok_or_else(|| io::Error::other("no name"))?);
    match copy_tree(src, &dest, report) {
        Ok(()) => Ok(dest),
        Err(e) => {
            // a half-copied folder is removed; a cancelled or failed file is already gone
            if dest.is_dir() {
                let _ = std::fs::remove_dir_all(&dest);
            }
            Err(e)
        }
    }
}

/// Move into `dest_dir`: a rename when on the same storage, else copy and then remove the original
/// (only once the copy is complete).
pub fn move_into(src: &Path, dest_dir: &Path, report: Report) -> io::Result<PathBuf> {
    if src.parent() == Some(dest_dir) {
        return Err(io::Error::other("it is already in this folder"));
    }
    if dest_dir.starts_with(src) {
        return Err(io::Error::other("a folder cannot be moved into itself"));
    }
    std::fs::create_dir_all(dest_dir)?;
    let dest = unique_dest(dest_dir, src.file_name().ok_or_else(|| io::Error::other("no name"))?);
    if std::fs::rename(src, &dest).is_ok() {
        let size = tree_size(&[dest.clone()], &AtomicBool::new(false)).0;
        report(size, &dest);
        return Ok(dest);
    }
    match copy_tree(src, &dest, report) {
        Ok(()) => {
            if src.is_dir() { std::fs::remove_dir_all(src)? } else { std::fs::remove_file(src)? }
            Ok(dest)
        }
        Err(e) => {
            if dest.is_dir() {
                let _ = std::fs::remove_dir_all(&dest);
            }
            Err(e)
        }
    }
}

/// Walk the tree below `root` looking for names that contain `query` (already lowercase).
pub fn search(root: &Path, query: &str, show_hidden: bool, limit: usize, cancel: &AtomicBool, on_hit: &mut dyn FnMut(Entry)) {
    let mut stack = vec![root.to_path_buf()];
    let mut found = 0;
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) || found >= limit {
            return;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let path = e.path();
            // symlinked folders are not followed, so loops cannot happen
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if name.to_lowercase().contains(query) {
                if let Some(entry) = Entry::from_path(&path) {
                    on_hit(entry);
                    found += 1;
                    if found >= limit {
                        return;
                    }
                }
            }
            if is_dir {
                stack.push(path);
            }
        }
    }
}

/// Everything that is a file under the storage root, with its kind (folders are not listed).
pub struct Scan {
    pub files: Vec<Entry>,
    pub counts: [usize; 8],
    pub bytes: [u64; 8],
}

fn kind_index(k: Kind) -> usize {
    k as usize
}

impl Scan {
    pub fn count(&self, k: Kind) -> usize {
        self.counts[kind_index(k)]
    }

    pub fn bytes(&self, k: Kind) -> u64 {
        self.bytes[kind_index(k)]
    }
}

pub fn scan_storage(root: &Path, trash_dir: &Path, cancel: &AtomicBool) -> Scan {
    let mut files = Vec::new();
    let (mut counts, mut bytes) = ([0usize; 8], [0u64; 8]);
    let mut stack = vec![root.to_path_buf()];
    let (data, obb) = (root.join("Android").join("data"), root.join("Android").join("obb"));
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if dir == trash_dir || dir == data || dir == obb {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let path = e.path();
            if t.is_dir() {
                stack.push(path);
            } else if t.is_file() {
                if let Some(entry) = Entry::from_path(&path) {
                    let i = kind_index(entry.kind);
                    counts[i] += 1;
                    bytes[i] += entry.size;
                    files.push(entry);
                }
            }
        }
    }
    Scan { files, counts, bytes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("files-fsops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn keep_going() -> impl FnMut(u64, &Path) -> bool {
        |_, _| true
    }

    #[test]
    fn kinds_come_from_the_extension() {
        assert_eq!(kind_of(Path::new("/a/b.JPG"), false), Kind::Image);
        assert_eq!(kind_of(Path::new("clip.ts"), false), Kind::Video);
        assert_eq!(kind_of(Path::new("song.flac"), false), Kind::Audio);
        assert_eq!(kind_of(Path::new("cv.pdf"), false), Kind::Document);
        assert_eq!(kind_of(Path::new("x.7z"), false), Kind::Archive);
        assert_eq!(kind_of(Path::new("app.apk"), false), Kind::Apk);
        assert_eq!(kind_of(Path::new("noext"), false), Kind::Other);
        assert_eq!(kind_of(Path::new("photos.jpg"), true), Kind::Folder, "a folder is a folder whatever its name");
    }

    #[test]
    fn names_sort_naturally_with_folders_first() {
        let dir = temp("sort");
        std::fs::create_dir(dir.join("zeta")).unwrap();
        for n in ["file10.txt", "file2.txt", "File1.txt", "b.txt", "a.txt"] {
            std::fs::write(dir.join(n), n).unwrap();
        }
        let mut v = list_dir(&dir, false).unwrap();
        sort_entries(&mut v, Sort::default());
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zeta", "a.txt", "b.txt", "File1.txt", "file2.txt", "file10.txt"]);
        sort_entries(&mut v, Sort { key: SortKey::Name, descending: true });
        assert_eq!(v[0].name, "zeta", "folders stay first when reversed");
        assert_eq!(v[1].name, "file10.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_files_are_listed_only_on_request() {
        let dir = temp("hidden");
        std::fs::write(dir.join(".secret"), "x").unwrap();
        std::fs::write(dir.join("seen"), "x").unwrap();
        assert_eq!(list_dir(&dir, false).unwrap().len(), 1);
        assert_eq!(list_dir(&dir, true).unwrap().len(), 2);
        assert!(list_dir(&dir.join("missing"), false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.50 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.00 MB");
        assert_eq!(human_size(1_610_612_736), "1.50 GB");
        assert_eq!(human_size(250 * 1024 * 1024), "250 MB");
    }

    #[test]
    fn important_places_are_protected() {
        let root = Path::new("/sdcard");
        let trash = Path::new("/sdcard/.FilesTrash");
        assert!(protected_reason(Path::new("/sdcard"), root, trash).is_some());
        assert!(protected_reason(Path::new("/sdcard/Android"), root, trash).is_some());
        assert!(protected_reason(Path::new("/sdcard/Android/data"), root, trash).is_some());
        assert!(protected_reason(Path::new("/sdcard/.FilesTrash"), root, trash).is_some());
        assert!(protected_reason(Path::new("/sdcard/.FilesTrash/1_x"), root, trash).is_some());
        assert!(protected_reason(Path::new("/data/data/x"), root, trash).is_some());
        assert!(protected_reason(Path::new("/sdcard/DCIM"), root, trash).is_none());
        assert!(protected_reason(Path::new("/sdcard/Android/media/com.whatsapp"), root, trash).is_none());
    }

    #[test]
    fn names_are_made_safe_and_unique() {
        assert_eq!(sanitize_name("  new/folder? ").as_deref(), Some("new_folder_"));
        assert_eq!(sanitize_name(".."), None);
        assert_eq!(sanitize_name("   "), None);
        let dir = temp("uniq");
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        assert!(unique_dest(&dir, "a.txt".as_ref()).ends_with("a (1).txt"));
        std::fs::write(dir.join("a (1).txt"), "2").unwrap();
        assert!(unique_dest(&dir, "a.txt".as_ref()).ends_with("a (2).txt"));
        assert!(unique_dest(&dir, "fresh".as_ref()).ends_with("fresh"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copying_a_folder_copies_everything_and_keeps_dates() {
        let root = temp("copy");
        let src = root.join("album");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), vec![7u8; 3_000_000]).unwrap();
        std::fs::write(src.join("sub/b.txt"), "bee").unwrap();
        let dest_dir = root.join("target");
        let mut bytes = 0u64;
        let out = copy_into(&src, &dest_dir, &mut |n, _| {
            bytes += n;
            true
        })
        .unwrap();
        assert_eq!(out, dest_dir.join("album"));
        assert_eq!(std::fs::read(out.join("sub/b.txt")).unwrap(), b"bee");
        assert_eq!(std::fs::metadata(out.join("a.txt")).unwrap().len(), 3_000_000);
        assert_eq!(bytes, 3_000_003);
        assert_eq!(std::fs::metadata(src.join("a.txt")).unwrap().modified().unwrap(), std::fs::metadata(out.join("a.txt")).unwrap().modified().unwrap());
        // copying again must not overwrite
        let again = copy_into(&src, &dest_dir, &mut keep_going()).unwrap();
        assert!(again.ends_with("album (1)"));
        // never into itself
        assert!(copy_into(&src, &src.join("sub"), &mut keep_going()).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cancelling_a_copy_leaves_no_partial_file() {
        let root = temp("cancel");
        std::fs::write(root.join("big.bin"), vec![1u8; 5_000_000]).unwrap();
        let r = copy_into(&root.join("big.bin"), &root.join("out"), &mut |_, _| false);
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(!root.join("out/big.bin").exists());
        assert!(root.join("big.bin").exists(), "the original is untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn moving_keeps_one_copy_and_refuses_silly_targets() {
        let root = temp("move");
        std::fs::create_dir_all(root.join("a/inner")).unwrap();
        std::fs::write(root.join("a/inner/f.txt"), "x").unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        let out = move_into(&root.join("a"), &root.join("b"), &mut keep_going()).unwrap();
        assert!(!root.join("a").exists());
        assert_eq!(std::fs::read(out.join("inner/f.txt")).unwrap(), b"x");
        assert!(move_into(&out, &root.join("b"), &mut keep_going()).is_err(), "already in that folder");
        assert!(move_into(&root.join("b"), &out, &mut keep_going()).is_err(), "not into itself");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_finds_names_below_a_folder() {
        let root = temp("search");
        std::fs::create_dir_all(root.join("x/y")).unwrap();
        std::fs::write(root.join("x/y/Holiday.JPG"), "1").unwrap();
        std::fs::write(root.join("x/holiday notes.txt"), "2").unwrap();
        std::fs::write(root.join("other.txt"), "3").unwrap();
        std::fs::write(root.join(".holiday-hidden"), "4").unwrap();
        let mut hits = Vec::new();
        search(&root, "holiday", false, 100, &AtomicBool::new(false), &mut |e| hits.push(e.name));
        hits.sort();
        assert_eq!(hits, ["Holiday.JPG", "holiday notes.txt"]);
        let mut limited = 0;
        search(&root, "holiday", true, 1, &AtomicBool::new(false), &mut |_| limited += 1);
        assert_eq!(limited, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_storage_scan_counts_each_kind_and_skips_the_trash() {
        let root = temp("scan");
        let trash = root.join(".FilesTrash");
        std::fs::create_dir_all(root.join("DCIM")).unwrap();
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(root.join("DCIM/a.jpg"), vec![0u8; 100]).unwrap();
        std::fs::write(root.join("DCIM/b.jpg"), vec![0u8; 50]).unwrap();
        std::fs::write(root.join("movie.mp4"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("notes.txt"), "hi").unwrap();
        std::fs::write(trash.join("1_old.jpg"), vec![0u8; 999]).unwrap();
        let s = scan_storage(&root, &trash, &AtomicBool::new(false));
        assert_eq!(s.count(Kind::Image), 2);
        assert_eq!(s.bytes(Kind::Image), 150);
        assert_eq!(s.count(Kind::Video), 1);
        assert_eq!(s.count(Kind::Document), 1);
        assert_eq!(s.files.len(), 4);
        let _ = std::fs::remove_dir_all(&root);
    }
}
