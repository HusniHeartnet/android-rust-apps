//! Thumbnails kept on disk, so scrolling and starting the app do not decode every original again.
//! Each is a small JPEG named after a hash of the file's path, size and modified time, so an edited
//! or replaced file simply gets a new entry. Videos also keep their length in a tiny side file.

use eframe::egui::ColorImage;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn key(path: &Path, mtime: SystemTime, size: u64) -> u64 {
    // FNV-1a over the path bytes, the size and the modified time in seconds
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    feed(path.to_string_lossy().as_bytes());
    feed(&size.to_le_bytes());
    feed(&mtime.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).to_le_bytes());
    h
}

fn jpg(dir: &Path, key: u64) -> PathBuf {
    dir.join(format!("{key:016x}.jpg"))
}

fn dur(dir: &Path, key: u64) -> PathBuf {
    dir.join(format!("{key:016x}.dur"))
}

pub fn load(dir: &Path, key: u64) -> Option<(ColorImage, Option<u64>)> {
    let bytes = std::fs::read(jpg(dir, key)).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let duration = std::fs::read_to_string(dur(dir, key)).ok().and_then(|t| t.trim().parse().ok());
    Some((ColorImage::from_rgba_unmultiplied(size, img.as_raw()), duration))
}

pub fn store(dir: &Path, key: u64, img: &ColorImage, duration: Option<u64>) {
    let _ = std::fs::create_dir_all(dir);
    let mut rgb = Vec::with_capacity(img.pixels.len() * 3);
    for p in &img.pixels {
        rgb.extend_from_slice(&[p.r(), p.g(), p.b()]);
    }
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
    if enc.write_image(&rgb, img.size[0] as u32, img.size[1] as u32, image::ExtendedColorType::Rgb8).is_err() {
        return;
    }
    // write to a temp name first so a half-written file is never read back
    let tmp = dir.join(format!("{key:016x}.tmp"));
    if std::fs::write(&tmp, &out).is_ok() {
        let _ = std::fs::rename(&tmp, jpg(dir, key));
    }
    if let Some(d) = duration {
        let _ = std::fs::write(dur(dir, key), d.to_string());
    }
}

pub fn forget(dir: &Path, key: u64) {
    let _ = std::fs::remove_file(jpg(dir, key));
    let _ = std::fs::remove_file(dur(dir, key));
}

/// Remove entries that no longer belong to any file, once there are clearly too many of them.
pub fn prune(dir: &Path, keep: &HashSet<u64>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let files: Vec<_> = rd.flatten().collect();
    if files.len() <= keep.len() * 2 + 100 {
        return;
    }
    for f in files {
        let name = f.file_name().to_string_lossy().into_owned();
        let stale = match name.split('.').next().and_then(|s| u64::from_str_radix(s, 16).ok()) {
            Some(k) => !keep.contains(&k),
            None => true,
        };
        if stale {
            let _ = std::fs::remove_file(f.path());
        }
    }
}

use image::ImageEncoder;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gallery-tc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn keys_change_with_path_size_and_time() {
        let t = UNIX_EPOCH + Duration::from_secs(1000);
        let k = key(Path::new("/a/b.jpg"), t, 10);
        assert_eq!(k, key(Path::new("/a/b.jpg"), t, 10));
        assert_ne!(k, key(Path::new("/a/c.jpg"), t, 10));
        assert_ne!(k, key(Path::new("/a/b.jpg"), t, 11));
        assert_ne!(k, key(Path::new("/a/b.jpg"), t + Duration::from_secs(1), 10));
    }

    #[test]
    fn thumbnails_round_trip_and_prune() {
        let dir = temp("rt");
        let img = ColorImage::new([40, 30], vec![eframe::egui::Color32::from_rgb(200, 40, 40); 40 * 30]);
        store(&dir, 7, &img, Some(65_000));
        store(&dir, 8, &img, None);
        let (back, d) = load(&dir, 7).expect("cached");
        assert_eq!(back.size, [40, 30]);
        assert_eq!(d, Some(65_000));
        let px = back.pixels[0];
        assert!(px.r() > 150 && px.g() < 90, "colour survives jpeg: {px:?}");
        assert_eq!(load(&dir, 8).unwrap().1, None);
        assert!(load(&dir, 9).is_none());

        // pruning keeps things alone until there are many strays
        prune(&dir, &HashSet::new());
        assert!(load(&dir, 7).is_some());
        for k in 100..400u64 {
            store(&dir, k, &img, None);
        }
        let keep: HashSet<u64> = [7u64].into_iter().collect();
        prune(&dir, &keep);
        assert!(load(&dir, 7).is_some());
        assert!(load(&dir, 8).is_none() && load(&dir, 200).is_none());
        forget(&dir, 7);
        assert!(load(&dir, 7).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
