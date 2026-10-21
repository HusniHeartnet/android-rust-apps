//! A small reader for the EXIF block of a JPEG: camera, exposure and where the picture was taken.
//! Only what the Details panel shows; everything is bounds-checked, and a damaged block just
//! yields less information.

use std::io::Read;
use std::path::Path;

#[derive(Default, Debug, PartialEq)]
pub struct Exif {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub taken: Option<String>,
    pub exposure: Option<String>,
    pub aperture: Option<String>,
    pub iso: Option<u32>,
    pub focal: Option<String>,
    /// Latitude and longitude in degrees (south and west are negative).
    pub gps: Option<(f64, f64)>,
}

impl Exif {
    pub fn is_empty(&self) -> bool {
        *self == Exif::default()
    }

    pub fn camera(&self) -> Option<String> {
        match (&self.make, &self.model) {
            (Some(a), Some(b)) if b.starts_with(a.as_str()) => Some(b.clone()),
            (Some(a), Some(b)) => Some(format!("{a} {b}")),
            (None, Some(b)) => Some(b.clone()),
            (Some(a), None) => Some(a.clone()),
            _ => None,
        }
    }

    /// "f/1.8 · 1/120 s · ISO 200 · 5.9 mm"
    pub fn settings_line(&self) -> Option<String> {
        let mut parts = Vec::new();
        parts.extend(self.aperture.clone());
        parts.extend(self.exposure.clone());
        parts.extend(self.iso.map(|i| format!("ISO {i}")));
        parts.extend(self.focal.clone());
        (!parts.is_empty()).then(|| parts.join("  ·  "))
    }
}

/// Read the EXIF of a JPEG file (the first 256 KB is enough: it sits at the start).
pub fn read(path: &Path) -> Option<Exif> {
    let mut head = Vec::new();
    std::fs::File::open(path).ok()?.take(256 * 1024).read_to_end(&mut head).ok()?;
    let tiff = find_tiff(&head)?;
    let e = parse_tiff(tiff);
    (!e.is_empty()).then_some(e)
}

/// The TIFF structure inside the APP1 "Exif" segment.
fn find_tiff(jpeg: &[u8]) -> Option<&[u8]> {
    if jpeg.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= jpeg.len() {
        if jpeg[i] != 0xFF {
            return None;
        }
        let marker = jpeg[i + 1];
        if marker == 0xDA || marker == 0xD9 {
            return None; // image data starts: no EXIF
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        let body = jpeg.get(i + 4..i + 2 + len)?;
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return Some(&body[6..]);
        }
        i += 2 + len;
    }
    None
}

struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.d.get(at..at + 2)?.try_into().ok()?;
        Some(if self.le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.d.get(at..at + 4)?.try_into().ok()?;
        Some(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }

    /// The entries of the IFD at `offset`: (tag, type, count, position of the value field).
    fn entries(&self, offset: usize) -> Vec<(u16, u16, u32, usize)> {
        let Some(n) = self.u16(offset) else { return Vec::new() };
        (0..n as usize)
            .filter_map(|k| {
                let at = offset + 2 + k * 12;
                Some((self.u16(at)?, self.u16(at + 2)?, self.u32(at + 4)?, at + 8))
            })
            .collect()
    }

    /// Where the value bytes of an entry are: inline when they fit in four bytes.
    fn value_at(&self, ty: u16, count: u32, field: usize) -> Option<usize> {
        let size = match ty {
            1 | 2 | 7 => 1,
            3 => 2,
            4 | 9 => 4,
            5 | 10 => 8,
            _ => return None,
        } * count as usize;
        if size <= 4 { Some(field) } else { Some(self.u32(field)? as usize) }
    }

    fn text(&self, (_, ty, count, field): (u16, u16, u32, usize)) -> Option<String> {
        if ty != 2 {
            return None;
        }
        let at = self.value_at(ty, count, field)?;
        let raw = self.d.get(at..at + count as usize)?;
        let s = String::from_utf8_lossy(raw).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
        (!s.is_empty()).then_some(s)
    }

    fn short(&self, (_, ty, count, field): (u16, u16, u32, usize)) -> Option<u32> {
        if count == 0 {
            return None;
        }
        let at = self.value_at(ty, count, field)?;
        match ty {
            3 => self.u16(at).map(u32::from),
            4 => self.u32(at),
            _ => None,
        }
    }

    fn rational(&self, at: usize) -> Option<f64> {
        let (n, d) = (self.u32(at)?, self.u32(at + 4)?);
        (d != 0).then(|| f64::from(n) / f64::from(d))
    }

    fn rationals(&self, (_, ty, count, field): (u16, u16, u32, usize), want: usize) -> Option<Vec<f64>> {
        if ty != 5 || (count as usize) < want {
            return None;
        }
        let at = self.value_at(ty, count, field)?;
        (0..want).map(|k| self.rational(at + k * 8)).collect()
    }
}

fn parse_tiff(d: &[u8]) -> Exif {
    let le = match d.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Exif::default(),
    };
    let t = Tiff { d, le };
    if t.u16(2) != Some(42) {
        return Exif::default();
    }
    let mut out = Exif::default();
    let Some(ifd0) = t.u32(4) else { return out };
    let (mut exif_ptr, mut gps_ptr) = (None, None);
    for e in t.entries(ifd0 as usize) {
        match e.0 {
            0x010F => out.make = t.text(e),
            0x0110 => out.model = t.text(e),
            0x0132 => out.taken = out.taken.take().or_else(|| t.text(e)),
            0x8769 => exif_ptr = t.short(e),
            0x8825 => gps_ptr = t.short(e),
            _ => {}
        }
    }
    if let Some(p) = exif_ptr {
        for e in t.entries(p as usize) {
            match e.0 {
                0x829A => {
                    out.exposure = t.value_at(e.1, e.2, e.3).and_then(|at| t.rational(at)).filter(|v| *v > 0.0).map(|v| {
                        if v >= 1.0 { format!("{v:.1} s") } else { format!("1/{} s", (1.0 / v).round() as u32) }
                    })
                }
                0x829D => out.aperture = t.value_at(e.1, e.2, e.3).and_then(|at| t.rational(at)).map(|v| format!("f/{v:.1}")),
                0x8827 => out.iso = t.short(e),
                0x9003 => out.taken = t.text(e).or(out.taken.take()),
                0x920A => out.focal = t.value_at(e.1, e.2, e.3).and_then(|at| t.rational(at)).map(|v| format!("{v:.1} mm")),
                0xA434 => out.lens = t.text(e),
                _ => {}
            }
        }
    }
    if let Some(p) = gps_ptr {
        let (mut lat_ref, mut lon_ref, mut lat, mut lon) = (None, None, None, None);
        for e in t.entries(p as usize) {
            match e.0 {
                1 => lat_ref = t.text(e),
                2 => lat = t.rationals(e, 3),
                3 => lon_ref = t.text(e),
                4 => lon = t.rationals(e, 3),
                _ => {}
            }
        }
        let deg = |v: Vec<f64>| v[0] + v[1] / 60.0 + v[2] / 3600.0;
        if let (Some(la), Some(lo)) = (lat, lon) {
            let la = deg(la) * if lat_ref.as_deref() == Some("S") { -1.0 } else { 1.0 };
            let lo = deg(lo) * if lon_ref.as_deref() == Some("W") { -1.0 } else { 1.0 };
            if (la != 0.0 || lo != 0.0) && la.abs() <= 90.0 && lo.abs() <= 180.0 {
                out.gps = Some((la, lo));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a little-endian TIFF with the given entries; values longer than four bytes go
    /// after the IFDs. Each entry is (tag, type, count, bytes).
    struct Builder {
        data: Vec<u8>,
    }

    fn entry(tag: u16, ty: u16, count: u32, val: &[u8]) -> (u16, u16, u32, Vec<u8>) {
        (tag, ty, count, val.to_vec())
    }

    fn ifd(b: &mut Builder, at: usize, items: Vec<(u16, u16, u32, Vec<u8>)>, blob_at: &mut usize) {
        let n = items.len();
        b.data.resize(b.data.len().max(at + 2 + n * 12 + 4), 0);
        b.data[at..at + 2].copy_from_slice(&(n as u16).to_le_bytes());
        for (k, (tag, ty, count, val)) in items.into_iter().enumerate() {
            let p = at + 2 + k * 12;
            b.data[p..p + 2].copy_from_slice(&tag.to_le_bytes());
            b.data[p + 2..p + 4].copy_from_slice(&ty.to_le_bytes());
            b.data[p + 4..p + 8].copy_from_slice(&count.to_le_bytes());
            if val.len() <= 4 {
                b.data[p + 8..p + 8 + val.len()].copy_from_slice(&val);
            } else {
                b.data[p + 8..p + 12].copy_from_slice(&(*blob_at as u32).to_le_bytes());
                b.data.resize(b.data.len().max(*blob_at + val.len()), 0);
                b.data[*blob_at..*blob_at + val.len()].copy_from_slice(&val);
                *blob_at += val.len();
            }
        }
    }

    fn rat(n: u32, d: u32) -> Vec<u8> {
        [n.to_le_bytes(), d.to_le_bytes()].concat()
    }

    fn sample() -> Vec<u8> {
        let mut b = Builder { data: b"II*\0\x08\0\0\0".to_vec() };
        let mut blob = 400;
        ifd(
            &mut b,
            8,
            vec![
                entry(0x010F, 2, 7, b"Xiaomi\0"),
                entry(0x0110, 2, 14, b"Xiaomi 24069PC\0"),
                entry(0x8769, 4, 1, &100u32.to_le_bytes()),
                entry(0x8825, 4, 1, &200u32.to_le_bytes()),
            ],
            &mut blob,
        );
        ifd(
            &mut b,
            100,
            vec![
                entry(0x829A, 5, 1, &rat(1, 120)),
                entry(0x829D, 5, 1, &rat(18, 10)),
                entry(0x8827, 3, 1, &200u16.to_le_bytes()),
                entry(0x9003, 2, 20, b"2026:10:21 14:05:09\0"),
                entry(0x920A, 5, 1, &rat(59, 10)),
            ],
            &mut blob,
        );
        let lat = [rat(6, 1), rat(12, 1), rat(36, 1)].concat();
        let lon = [rat(106, 1), rat(50, 1), rat(0, 1)].concat();
        ifd(&mut b, 200, vec![entry(1, 2, 2, b"S\0"), entry(2, 5, 3, &lat), entry(3, 2, 2, b"E\0"), entry(4, 5, 3, &lon)], &mut blob);
        b.data
    }

    fn jpeg_with(tiff: &[u8]) -> Vec<u8> {
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(tiff);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
        out
    }

    #[test]
    fn reads_camera_exposure_and_place() {
        let jpeg = jpeg_with(&sample());
        let e = parse_tiff(find_tiff(&jpeg).unwrap());
        assert_eq!(e.camera().as_deref(), Some("Xiaomi 24069PC"), "model already starts with the make");
        assert_eq!(e.exposure.as_deref(), Some("1/120 s"));
        assert_eq!(e.aperture.as_deref(), Some("f/1.8"));
        assert_eq!(e.iso, Some(200));
        assert_eq!(e.focal.as_deref(), Some("5.9 mm"));
        assert_eq!(e.taken.as_deref(), Some("2026:10:21 14:05:09"));
        let (la, lo) = e.gps.unwrap();
        assert!((la + 6.21).abs() < 1e-6, "south is negative: {la}");
        assert!((lo - 106.8333333).abs() < 1e-4, "{lo}");
        assert_eq!(e.settings_line().unwrap(), "f/1.8  ·  1/120 s  ·  ISO 200  ·  5.9 mm");
    }

    #[test]
    fn damaged_or_missing_exif_gives_nothing_and_never_panics() {
        assert!(find_tiff(b"not a jpeg").is_none());
        assert!(find_tiff(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]).is_none());
        let jpeg = jpeg_with(&sample());
        for cut in [20, 40, 80, 150, 300] {
            let _ = parse_tiff(&jpeg[12..jpeg.len().min(12 + cut)]); // truncated blocks
        }
        assert!(parse_tiff(b"II*\0\xFF\xFF\xFF\xFF").is_empty(), "offset past the end");
        assert!(parse_tiff(b"XX").is_empty());
        assert_eq!(Exif { make: Some("A".into()), model: Some("B".into()), ..Default::default() }.camera().as_deref(), Some("A B"));
    }

    #[test]
    fn reads_a_file() {
        let dir = std::env::temp_dir().join(format!("gallery-exifmod-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.jpg");
        std::fs::write(&p, jpeg_with(&sample())).unwrap();
        assert_eq!(read(&p).unwrap().iso, Some(200));
        std::fs::write(dir.join("b.jpg"), b"\xFF\xD8\xFF\xDA\0\x02").unwrap();
        assert!(read(&dir.join("b.jpg")).is_none());
        assert!(read(&dir.join("missing.jpg")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
