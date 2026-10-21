//! Moving and copying items between albums, and creating, renaming and deleting albums
//! (an album is a folder).

use super::*;
use std::ffi::OsStr;
use std::io;

/// What a typed album name becomes as a folder name, or None if nothing usable is left.
pub(crate) fn sanitize_album_name(s: &str) -> Option<String> {
    let cleaned: String = s
        .trim()
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let cleaned: String = cleaned.trim().trim_start_matches('.').trim().chars().take(60).collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// `dir/name`, or `dir/name (1).ext`, `(2)`… when that is taken, so nothing is overwritten.
pub(crate) fn unique_dest(dir: &Path, name: &OsStr) -> PathBuf {
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

pub(crate) fn move_into(src: &Path, dir: &Path) -> io::Result<PathBuf> {
    if src.parent() == Some(dir) {
        return Err(io::Error::other("already in that album"));
    }
    std::fs::create_dir_all(dir)?;
    let dest = unique_dest(dir, src.file_name().ok_or_else(|| io::Error::other("no file name"))?);
    trash::move_file(src, &dest)?;
    Ok(dest)
}

pub(crate) fn copy_into(src: &Path, dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let dest = unique_dest(dir, src.file_name().ok_or_else(|| io::Error::other("no file name"))?);
    let copied = std::fs::copy(src, &dest)?;
    if copied != std::fs::metadata(src)?.len() {
        let _ = std::fs::remove_file(&dest);
        return Err(io::Error::other("incomplete copy"));
    }
    // the copy keeps the date of the original, so it sits next to it in the timeline
    if let Ok(m) = std::fs::metadata(src).and_then(|m| m.modified()) {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&dest) {
            let _ = f.set_modified(m);
        }
    }
    Ok(dest)
}

/// A fingerprint of a file from its size and eight evenly spread 64 KB samples: cheap even for
/// multi-gigabyte videos, and two files with the same size and samples are the same in practice.
pub(crate) fn sample_hash(path: &Path, len: u64) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 64 * 1024;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ len;
    let mut buf = vec![0u8; CHUNK as usize];
    let spots: Vec<u64> = if len <= CHUNK * 8 { vec![0] } else { (0..8).map(|k| (len - CHUNK) / 7 * k).collect() };
    for at in spots {
        f.seek(SeekFrom::Start(at)).ok()?;
        let n = f.read(&mut buf).ok()?;
        for &b in &buf[..n] {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        if len <= CHUNK * 8 {
            // small file: read it all
            let mut rest = Vec::new();
            f.read_to_end(&mut rest).ok()?;
            for &b in &rest {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    Some(h)
}

/// Groups of files that are the same, each group in the order given (callers sort by date).
pub(crate) fn find_duplicates(files: &[(PathBuf, u64)]) -> Vec<Vec<PathBuf>> {
    let mut by_size: HashMap<u64, Vec<&PathBuf>> = HashMap::new();
    for (p, size) in files.iter().filter(|(_, s)| *s > 0) {
        by_size.entry(*size).or_default().push(p);
    }
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for (size, same_size) in by_size.into_iter().filter(|(_, v)| v.len() > 1) {
        let mut by_hash: HashMap<u64, Vec<PathBuf>> = HashMap::new();
        for p in same_size {
            if let Some(h) = sample_hash(p, size) {
                by_hash.entry(h).or_default().push(p.clone());
            }
        }
        groups.extend(by_hash.into_values().filter(|v| v.len() > 1));
    }
    groups
}

/// The album chooser for a move or copy.
pub(crate) struct AlbumPicker {
    pub op: AlbumOp,
    pub ids: Vec<usize>,
    pub name: String,
}

/// A question about one album, asked from its long-press menu.
pub(crate) enum AlbumDialog {
    Rename { key: String, text: String },
    Delete { key: String },
}

impl Gallery {
    pub(crate) fn open_album_picker(&mut self, op: AlbumOp, ids: Vec<usize>) {
        self.album_picker = Some(AlbumPicker { op, ids, name: "New album".into() });
    }

    /// Where new albums are made.
    fn new_album_root(&self) -> PathBuf {
        if cfg!(target_os = "android") { PathBuf::from("/sdcard/Pictures") } else { self.roots.first().cloned().unwrap_or_else(std::env::temp_dir) }
    }

    pub(crate) fn album_picker_ui(&mut self, ctx: &Context) {
        let Some(mut pk) = self.album_picker.take() else { return };
        let verb = if pk.op == AlbumOp::Move { "Move" } else { "Copy" };
        let n = pk.ids.len();
        let root = self.new_album_root();
        let screen = ctx.viewport_rect();
        let (mut chosen, mut cancel): (Option<PathBuf>, bool) = (None, false);
        egui::Area::new(egui::Id::new("album-picker")).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(screen.width() - 36.0);
                ui.label(egui::RichText::new(format!("{verb} {n} item{} to…", if n == 1 { "" } else { "s" })).strong().size(18.0));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut pk.name).desired_width(ui.available_width() - 120.0).hint_text("New album name"));
                    if ui.add_sized([110.0, 30.0], egui::Button::new(format!("New + {verb}"))).clicked() {
                        match sanitize_album_name(&pk.name) {
                            Some(name) => chosen = Some(root.join(name)),
                            None => {}
                        }
                    }
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(screen.height() * 0.5).auto_shrink([false, true]).show(ui, |ui| {
                    for a in &self.real_albums {
                        if ui.add_sized([ui.available_width(), 40.0], egui::Button::new(format!("{}   ({})", a.label, a.count))).clicked() {
                            chosen = Some(PathBuf::from(&a.key));
                        }
                    }
                });
                ui.add_space(6.0);
                if ui.add_sized([ui.available_width(), 38.0], egui::Button::new("Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        match chosen {
            Some(dir) => self.perform_album_op(ctx, pk.op, &pk.ids, &dir),
            None if !cancel => self.album_picker = Some(pk),
            None => {}
        }
    }

    pub(crate) fn perform_album_op(&mut self, ctx: &Context, op: AlbumOp, ids: &[usize], dir: &Path) {
        if op == AlbumOp::Move && !self.manage_ok {
            self.manage_prompt = true;
            return;
        }
        self.remember_position();
        self.player = None;
        let dir_key = dir.to_string_lossy().into_owned();
        let (mut done, mut failed) = (0usize, 0usize);
        let mut touched: Vec<String> = Vec::new();
        for &id in ids {
            let it = &self.items[id];
            if it.trashed || it.hidden || it.gone {
                continue;
            }
            let src = it.path.clone();
            match op {
                AlbumOp::Move => match move_into(&src, dir) {
                    Ok(dest) => {
                        touched.push(src.to_string_lossy().into_owned());
                        touched.push(dest.to_string_lossy().into_owned());
                        if self.favs.remove(&src) {
                            self.favs.insert(dest.clone());
                        }
                        let it = &mut self.items[id];
                        it.path = dest;
                        it.album = dir_key.clone();
                        done += 1;
                    }
                    Err(_) => failed += 1,
                },
                AlbumOp::Copy => match copy_into(&src, dir) {
                    Ok(dest) => {
                        touched.push(dest.to_string_lossy().into_owned());
                        done += 1;
                    }
                    Err(_) => failed += 1,
                },
            }
        }
        self.save_favs();
        if op == AlbumOp::Copy {
            self.rescan(ctx); // the copies show up when the new scan arrives
        }
        self.rebuild_view();
        self.notify_media(&touched);
        self.exit_select();
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let past = if op == AlbumOp::Move { "Moved" } else { "Copied" };
        self.say(ctx, if failed == 0 { format!("{past} {done} to {name}") } else { format!("{past} {done} to {name}, {failed} failed") }, None);
        // a moved item may have left the album being viewed
        if let Some(v) = self.viewer.as_ref().map(|v| (v.id, v.pos)) {
            match self.view.iter().position(|&i| i == v.0) {
                Some(p) => {
                    if let Some(vw) = self.viewer.as_mut() {
                        vw.pos = p;
                    }
                }
                None => self.viewer_after_removal(v.1),
            }
        }
    }

    pub(crate) fn rename_album(&mut self, ctx: &Context, key: &str, typed: &str) {
        let Some(name) = sanitize_album_name(typed) else {
            self.say(ctx, "That name can't be used.", None);
            return;
        };
        let old = PathBuf::from(key);
        let new = old.with_file_name(&name);
        if new == old {
            return;
        }
        if new.exists() {
            self.say(ctx, "An album with that name already exists.", None);
            return;
        }
        if let Err(e) = std::fs::rename(&old, &new) {
            self.say(ctx, format!("Could not rename: {e}"), None);
            return;
        }
        let swap = |p: &Path| p.strip_prefix(&old).ok().map(|rest| new.join(rest));
        let mut touched = Vec::new();
        for it in self.items.iter_mut().filter(|i| !i.gone && !i.trashed && !i.hidden) {
            if let Some(np) = swap(&it.path) {
                touched.push(it.path.to_string_lossy().into_owned());
                touched.push(np.to_string_lossy().into_owned());
                it.path = np;
                if let Some(a) = swap(Path::new(&it.album)) {
                    it.album = a.to_string_lossy().into_owned();
                }
            }
        }
        self.favs = self.favs.iter().map(|p| swap(p).unwrap_or_else(|| p.clone())).collect();
        self.save_favs();
        let new_key = new.to_string_lossy().into_owned();
        for pin in self.pins.iter_mut() {
            if let Some(a) = swap(Path::new(pin.as_str())) {
                *pin = a.to_string_lossy().into_owned();
            }
        }
        let pins = self.pins.clone();
        self.save_pins(&pins);
        if self.filter.as_deref() == Some(key) {
            self.filter = Some(new_key);
        }
        self.rebuild_view();
        self.notify_media(&touched);
        self.say(ctx, format!("Renamed to {name}"), None);
    }

    /// Move every item of an album to the trash (the folder is removed too when it ends up empty).
    pub(crate) fn delete_album(&mut self, ctx: &Context, key: &str) {
        let ids: Vec<usize> = (0..self.items.len()).filter(|&i| self.items[i].album == key && !self.items[i].gone && !self.items[i].trashed && !self.items[i].hidden).collect();
        if self.trash_items(ctx, &ids) > 0 || ids.is_empty() {
            let _ = std::fs::remove_dir(key); // only succeeds when the folder is empty
            self.pins.retain(|k| k != key);
            let pins = self.pins.clone();
            self.save_pins(&pins);
            if self.filter.as_deref() == Some(key) {
                self.filter = None;
                self.album_open = false;
            }
            self.rebuild_view();
        }
    }

    pub(crate) fn album_dialog_ui(&mut self, ctx: &Context) {
        let Some(dialog) = self.album_dialog.take() else { return };
        let screen = ctx.viewport_rect();
        let mut keep = true;
        let mut result: Option<(bool, String, String)> = None; // (rename?, key, text)
        let dialog = match dialog {
            AlbumDialog::Rename { key, mut text } => {
                egui::Area::new(egui::Id::new("album-dialog")).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(screen.width() - 48.0);
                        ui.label(egui::RichText::new("Rename album").strong().size(18.0));
                        ui.add_space(6.0);
                        ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.add_sized([ui.available_width() / 2.0 - 4.0, 36.0], egui::Button::new("Rename")).clicked() {
                                result = Some((true, key.clone(), text.clone()));
                                keep = false;
                            }
                            if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("Cancel")).clicked() {
                                keep = false;
                            }
                        });
                    });
                });
                AlbumDialog::Rename { key, text }
            }
            AlbumDialog::Delete { key } => {
                let count = self.items.iter().filter(|i| i.album == key && !i.gone && !i.trashed && !i.hidden).count();
                let label = self.album(&key).map(|a| a.label).unwrap_or_default();
                egui::Area::new(egui::Id::new("album-dialog")).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(screen.width() - 48.0);
                        ui.label(egui::RichText::new(format!("Delete \"{label}\"?")).strong().size(18.0));
                        ui.label(format!("{count} item{} will go to the trash, where they stay for {TRASH_DAYS} days.", if count == 1 { "" } else { "s" }));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.add_sized([ui.available_width() / 2.0 - 4.0, 36.0], egui::Button::new(egui::RichText::new("Move to trash").color(Color32::from_rgb(255, 120, 110)))).clicked() {
                                result = Some((false, key.clone(), String::new()));
                                keep = false;
                            }
                            if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("Cancel")).clicked() {
                                keep = false;
                            }
                        });
                    });
                });
                AlbumDialog::Delete { key }
            }
        };
        if keep {
            self.album_dialog = Some(dialog);
        }
        match result {
            Some((true, key, text)) => self.rename_album(ctx, &key, &text),
            Some((false, key, _)) => self.delete_album(ctx, &key),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gallery-albumops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn album_names_are_made_safe() {
        assert_eq!(sanitize_album_name("  Holiday 2026 ").as_deref(), Some("Holiday 2026"));
        assert_eq!(sanitize_album_name("a/b\\c:d").as_deref(), Some("a_b_c_d"));
        assert_eq!(sanitize_album_name("..secret").as_deref(), Some("secret"), "no hidden folders");
        assert_eq!(sanitize_album_name("   "), None);
        assert_eq!(sanitize_album_name("...."), None);
        assert_eq!(sanitize_album_name(&"x".repeat(100)).unwrap().len(), 60);
    }

    #[test]
    fn moving_and_copying_never_overwrite() {
        let root = temp("mc");
        let (a, b) = (root.join("A"), root.join("B"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("p.jpg"), b"first").unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("p.jpg"), b"already here").unwrap();

        let moved = move_into(&a.join("p.jpg"), &b).unwrap();
        assert!(moved.ends_with("p (1).jpg"));
        assert_eq!(std::fs::read(&moved).unwrap(), b"first");
        assert_eq!(std::fs::read(b.join("p.jpg")).unwrap(), b"already here");
        assert!(!a.join("p.jpg").exists());

        assert!(move_into(&moved, &b).is_err(), "already in that album");
        let copy = copy_into(&moved, &root.join("New album")).unwrap();
        assert!(moved.exists() && copy.exists());
        assert_eq!(std::fs::read(&copy).unwrap(), b"first");
        let (m1, m2) = (std::fs::metadata(&moved).unwrap().modified().unwrap(), std::fs::metadata(&copy).unwrap().modified().unwrap());
        assert_eq!(m1, m2, "a copy keeps the original's date");
        assert!(copy_into(&moved, &root.join("New album")).unwrap().ends_with("p (1) (1).jpg"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn duplicates_are_found_by_size_and_content() {
        let root = temp("dup");
        let big = |seed: u8| -> Vec<u8> { (0..2_000_000u32).map(|i| (i as u8).wrapping_mul(seed).wrapping_add(seed)).collect() };
        let files: Vec<(PathBuf, Vec<u8>)> = vec![
            (root.join("a.mp4"), big(3)),
            (root.join("a copy.mp4"), big(3)),
            (root.join("other.mp4"), big(5)),
            (root.join("small1.jpg"), b"hello".to_vec()),
            (root.join("small2.jpg"), b"hello".to_vec()),
            (root.join("small3.jpg"), b"jello".to_vec()),
            (root.join("empty1"), Vec::new()),
            (root.join("empty2"), Vec::new()),
        ];
        let mut list = Vec::new();
        for (p, data) in &files {
            std::fs::write(p, data).unwrap();
            list.push((p.clone(), data.len() as u64));
        }
        let mut groups = find_duplicates(&list);
        for g in &mut groups {
            g.sort();
        }
        groups.sort();
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert!(groups[0].iter().any(|p| p.ends_with("a copy.mp4")) && groups[0].iter().any(|p| p.ends_with("a.mp4")));
        assert!(groups[1].iter().any(|p| p.ends_with("small1.jpg")) && groups[1].iter().any(|p| p.ends_with("small2.jpg")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
