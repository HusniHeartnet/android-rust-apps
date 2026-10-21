//! Trash: "deleting" moves a photo into one folder; nothing is erased until "Delete forever".
//! A trashed file is named `<unix-seconds>_<percent-encoded original path>` so it can be restored.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn default_trash_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        PathBuf::from("/sdcard/.GalleryTrash")
    } else {
        std::env::temp_dir().join("gallery-trash")
    }
}

pub fn default_hidden_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        PathBuf::from("/sdcard/.GalleryHidden")
    } else {
        std::env::temp_dir().join("gallery-hidden")
    }
}

/// When a file went into the trash (unix seconds), from the start of its name.
pub fn trashed_at(trashed: &Path) -> Option<u64> {
    trashed.file_name()?.to_str()?.split_once('_')?.0.parse().ok()
}

fn encode(path: &Path) -> String {
    let mut out = String::new();
    for b in path.to_string_lossy().bytes() {
        match b {
            b'/' | b'%' | b'\\' | b':' => out.push_str(&format!("%{b:02X}")),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(s: &str) -> Option<PathBuf> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

/// Where a trashed file came from, if its name follows our scheme.
pub fn original_of(trashed: &Path) -> Option<PathBuf> {
    let name = trashed.file_name()?.to_str()?;
    decode(name.split_once('_')?.1)
}

pub fn in_trash(path: &Path, trash_dir: &Path) -> bool {
    path.parent() == Some(trash_dir)
}

pub(crate) fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // different filesystem: copy, then remove the source only if the copy is complete
    let copied = fs::copy(from, to)?;
    if copied != fs::metadata(from)?.len() {
        let _ = fs::remove_file(to);
        return Err(io::Error::other("incomplete copy"));
    }
    fs::remove_file(from)
}

pub fn move_to_trash(path: &Path, trash_dir: &Path) -> io::Result<PathBuf> {
    if in_trash(path, trash_dir) {
        return Err(io::Error::other("already in trash"));
    }
    fs::create_dir_all(trash_dir)?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let dest = trash_dir.join(format!("{ts}_{}", encode(path)));
    move_file(path, &dest)?;
    Ok(dest)
}

/// Move a trashed file back. If something already sits at the original path, the
/// restored copy gets " (restored)" added so nothing is overwritten.
pub fn restore(trashed: &Path, trash_dir: &Path) -> io::Result<PathBuf> {
    if !in_trash(trashed, trash_dir) {
        return Err(io::Error::other("not in trash"));
    }
    let orig = original_of(trashed).ok_or_else(|| io::Error::other("unknown original location"))?;
    if let Some(parent) = orig.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut dest = orig.clone();
    let mut n = 0;
    while dest.exists() {
        n += 1;
        let stem = orig.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = orig.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let tag = if n == 1 { " (restored)".to_string() } else { format!(" (restored {n})") };
        dest = orig.with_file_name(format!("{stem}{tag}{ext}"));
    }
    move_file(trashed, &dest)?;
    Ok(dest)
}

/// Permanently remove one trashed file. Refuses anything outside the trash folder.
pub fn delete_forever(trashed: &Path, trash_dir: &Path) -> io::Result<()> {
    if !in_trash(trashed, trash_dir) {
        return Err(io::Error::other("refusing to delete a file that is not in the trash"));
    }
    fs::remove_file(trashed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gallery-trash-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn encoding_round_trips_awkward_names() {
        for p in ["/sdcard/DCIM/Camera/IMG_1.jpg", "/a b/100% sure/x%2Fy.png", "/sdcard/Pictures/ünï/été 📷.jpg", "C:\\x\\y.jpg"] {
            let p = PathBuf::from(p);
            let e = encode(&p);
            assert!(!e.contains('/') && !e.contains('\\'), "{e}");
            assert_eq!(decode(&e).unwrap(), p);
        }
    }

    #[test]
    fn trash_time_is_read_from_the_name() {
        assert_eq!(trashed_at(Path::new("/t/1700000000_%2Fsdcard%2Fa.jpg")), Some(1_700_000_000));
        assert_eq!(trashed_at(Path::new("/t/junk.txt")), None);
        assert_eq!(trashed_at(Path::new("/t/abc_x.jpg")), None);
    }

    #[test]
    fn trash_and_restore_round_trip() {
        let root = temp("rt");
        let trash = root.join(".trash");
        let photo = root.join("Camera").join("IMG 1.jpg");
        fs::create_dir_all(photo.parent().unwrap()).unwrap();
        fs::write(&photo, b"pixels").unwrap();

        let t = move_to_trash(&photo, &trash).unwrap();
        assert!(!photo.exists() && t.exists() && in_trash(&t, &trash));
        assert_eq!(original_of(&t).unwrap(), photo);

        let back = restore(&t, &trash).unwrap();
        assert_eq!(back, photo);
        assert_eq!(fs::read(&back).unwrap(), b"pixels");
        assert!(!t.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_never_overwrites() {
        let root = temp("ow");
        let trash = root.join(".trash");
        let photo = root.join("a.jpg");
        fs::write(&photo, b"old").unwrap();
        let t = move_to_trash(&photo, &trash).unwrap();
        fs::write(&photo, b"NEW file taken later").unwrap();

        let back = restore(&t, &trash).unwrap();
        assert_ne!(back, photo);
        assert_eq!(fs::read(&photo).unwrap(), b"NEW file taken later");
        assert_eq!(fs::read(&back).unwrap(), b"old");
        assert!(back.file_name().unwrap().to_string_lossy().contains("(restored)"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_forever_only_inside_trash() {
        let root = temp("df");
        let trash = root.join(".trash");
        let outside = root.join("precious.jpg");
        fs::write(&outside, b"keep me").unwrap();
        assert!(delete_forever(&outside, &trash).is_err());
        assert!(outside.exists(), "file outside the trash must survive");
        // path tricks must not count as "in trash"
        fs::create_dir_all(&trash).unwrap();
        let sneaky = trash.join("..").join("precious.jpg");
        assert!(delete_forever(&sneaky, &trash).is_err());
        assert!(outside.exists());

        let t = move_to_trash(&outside, &trash).unwrap();
        assert!(delete_forever(&t, &trash).is_ok());
        assert!(!t.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cannot_trash_twice_or_restore_non_trash() {
        let root = temp("tw");
        let trash = root.join(".trash");
        let photo = root.join("a.jpg");
        fs::write(&photo, b"x").unwrap();
        let t = move_to_trash(&photo, &trash).unwrap();
        assert!(move_to_trash(&t, &trash).is_err());
        let other = root.join("b.jpg");
        fs::write(&other, b"y").unwrap();
        assert!(restore(&other, &trash).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
