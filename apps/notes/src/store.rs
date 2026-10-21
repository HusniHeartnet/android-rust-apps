//! The notes on disk. Each note is a plain text file (`<id>.txt`, or `<id>.pin.txt` when pinned), so
//! they can be read and copied without this app. Deleted notes move to `.trash` and are removed for
//! good after 30 days. Nothing here touches the screen, so it is all unit tested.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const TRASH_DAYS: u64 = 30;

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: u64,
    pub text: String,
    /// Unix seconds of the last change (for a trashed note: when it was deleted).
    pub modified: u64,
    pub pinned: bool,
}

impl Note {
    /// The first line with something on it.
    pub fn title(&self) -> String {
        let t = self.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        if t.is_empty() { "Empty note".to_string() } else { t.chars().take(80).collect() }
    }

    /// The next few words after the title, for the list.
    pub fn preview(&self) -> String {
        let mut lines = self.text.lines().map(str::trim).filter(|l| !l.is_empty());
        lines.next();
        let rest: Vec<&str> = lines.take(2).collect();
        rest.join("  ").chars().take(120).collect()
    }

    /// True when every word of `query` appears in the note (ignoring case).
    pub fn matches(&self, query: &str) -> bool {
        let text = self.text.to_lowercase();
        query.split_whitespace().all(|w| text.contains(&w.to_lowercase()))
    }
}

pub struct Store {
    dir: PathBuf,
    pub notes: Vec<Note>,
    pub trashed: Vec<Note>,
}

fn trash_dir(dir: &Path) -> PathBuf {
    dir.join(".trash")
}

fn note_path(dir: &Path, id: u64, pinned: bool) -> PathBuf {
    dir.join(if pinned { format!("{id}.pin.txt") } else { format!("{id}.txt") })
}

fn write_atomic(path: &Path, text: &str) -> bool {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, path).is_ok()
}

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

/// `1700000000123.txt` or `1700000000123.pin.txt` -> (id, pinned)
fn parse_name(name: &str) -> Option<(u64, bool)> {
    if let Some(stem) = name.strip_suffix(".pin.txt") {
        return stem.parse().ok().map(|id| (id, true));
    }
    name.strip_suffix(".txt")?.parse().ok().map(|id| (id, false))
}

impl Store {
    /// Read every note in `dir` and clear out trash older than 30 days.
    pub fn open(dir: &Path, now: u64) -> Store {
        let mut store = Store { dir: dir.to_path_buf(), notes: Vec::new(), trashed: Vec::new() };
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let (Some((id, pinned)), Ok(text)) = (parse_name(&name), std::fs::read_to_string(e.path())) {
                    store.notes.push(Note { id, text, modified: mtime(&e.path()), pinned });
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(trash_dir(dir)) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                // <deleted-seconds>_<id>.txt
                let parsed = name.strip_suffix(".txt").and_then(|s| s.split_once('_')).and_then(|(d, i)| Some((d.parse::<u64>().ok()?, i.parse::<u64>().ok()?)));
                let Some((deleted, id)) = parsed else { continue };
                if now.saturating_sub(deleted) > TRASH_DAYS * 86_400 {
                    let _ = std::fs::remove_file(e.path());
                } else if let Ok(text) = std::fs::read_to_string(e.path()) {
                    store.trashed.push(Note { id, text, modified: deleted, pinned: false });
                }
            }
        }
        store.trashed.sort_by_key(|n| std::cmp::Reverse(n.modified));
        store
    }

    /// A new, empty note. It is only written to disk once it has some text.
    pub fn create(&mut self, now: u64) -> u64 {
        let used = self.notes.iter().chain(&self.trashed).map(|n| n.id).max().unwrap_or(0);
        let id = (now * 1000).max(used + 1);
        self.notes.push(Note { id, text: String::new(), modified: now, pinned: false });
        id
    }

    pub fn get(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    /// Store new text. An empty note is dropped, so tapping + and going back leaves nothing behind.
    /// Returns false when the file could not be written.
    pub fn save(&mut self, id: u64, text: &str, now: u64) -> bool {
        let Some(i) = self.notes.iter().position(|n| n.id == id) else { return false };
        if text.trim().is_empty() {
            let n = self.notes.remove(i);
            let _ = std::fs::remove_file(note_path(&self.dir, n.id, n.pinned));
            return true;
        }
        if self.notes[i].text == text {
            return true;
        }
        let ok = write_atomic(&note_path(&self.dir, id, self.notes[i].pinned), text);
        if ok {
            self.notes[i].text = text.to_string();
            self.notes[i].modified = now;
        }
        ok
    }

    pub fn set_pinned(&mut self, id: u64, pinned: bool) {
        let Some(i) = self.notes.iter().position(|n| n.id == id) else { return };
        if self.notes[i].pinned == pinned {
            return;
        }
        let from = note_path(&self.dir, id, self.notes[i].pinned);
        let to = note_path(&self.dir, id, pinned);
        // a note that has never been written has no file to rename
        if !from.exists() || std::fs::rename(&from, &to).is_ok() {
            self.notes[i].pinned = pinned;
        }
    }

    /// Move a note to the trash.
    pub fn delete(&mut self, id: u64, now: u64) {
        let Some(i) = self.notes.iter().position(|n| n.id == id) else { return };
        let mut n = self.notes.remove(i);
        let from = note_path(&self.dir, n.id, n.pinned);
        if !n.text.trim().is_empty() {
            let to = trash_dir(&self.dir).join(format!("{now}_{}.txt", n.id));
            let _ = std::fs::create_dir_all(trash_dir(&self.dir));
            if std::fs::rename(&from, &to).is_ok() {
                n.modified = now;
                n.pinned = false;
                self.trashed.insert(0, n);
            }
        } else {
            let _ = std::fs::remove_file(from);
        }
    }

    pub fn restore(&mut self, id: u64) {
        let Some(i) = self.trashed.iter().position(|n| n.id == id) else { return };
        let n = &self.trashed[i];
        let from = trash_dir(&self.dir).join(format!("{}_{}.txt", n.modified, n.id));
        let to = note_path(&self.dir, n.id, false);
        if std::fs::rename(&from, &to).is_ok() {
            let mut n = self.trashed.remove(i);
            n.modified = mtime(&to);
            self.notes.push(n);
        }
    }

    pub fn delete_forever(&mut self, id: u64) {
        if let Some(i) = self.trashed.iter().position(|n| n.id == id) {
            let n = self.trashed.remove(i);
            let _ = std::fs::remove_file(trash_dir(&self.dir).join(format!("{}_{}.txt", n.modified, n.id)));
        }
    }

    pub fn empty_trash(&mut self) {
        let ids: Vec<u64> = self.trashed.iter().map(|n| n.id).collect();
        for id in ids {
            self.delete_forever(id);
        }
    }

    /// Indexes into `notes` that match the search, pinned first, newest first.
    pub fn view(&self, query: &str) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.notes.len()).filter(|&i| query.trim().is_empty() || self.notes[i].matches(query)).collect();
        idx.sort_by_key(|&i| (!self.notes[i].pinned, std::cmp::Reverse(self.notes[i].modified), std::cmp::Reverse(self.notes[i].id)));
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_saved_note_comes_back_after_reopening() {
        let d = tmp("reopen");
        let mut s = Store::open(&d, 1000);
        let id = s.create(1000);
        assert!(s.save(id, "Shopping\nmilk\neggs", 1001));
        let s = Store::open(&d, 1002);
        assert_eq!(s.notes.len(), 1);
        assert_eq!(s.notes[0].text, "Shopping\nmilk\neggs");
        assert_eq!(s.notes[0].title(), "Shopping");
        assert_eq!(s.notes[0].preview(), "milk  eggs");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_empty_note_leaves_nothing_behind() {
        let d = tmp("empty");
        let mut s = Store::open(&d, 1000);
        let id = s.create(1000);
        assert!(s.save(id, "  \n ", 1001));
        assert!(s.notes.is_empty());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        // emptying an existing note removes it too
        let id = s.create(1002);
        s.save(id, "x", 1003);
        s.save(id, "", 1004);
        assert!(s.notes.is_empty());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pinned_notes_sort_first_and_keep_their_text() {
        let d = tmp("pin");
        let mut s = Store::open(&d, 1000);
        let a = s.create(1000);
        s.save(a, "old", 1000);
        let b = s.create(2000);
        s.save(b, "new", 2000);
        assert_eq!(s.notes[s.view("")[0]].text, "new");
        s.set_pinned(a, true);
        assert_eq!(s.notes[s.view("")[0]].text, "old");
        let s = Store::open(&d, 3000);
        assert!(s.get(a).unwrap().pinned);
        assert_eq!(s.get(a).unwrap().text, "old");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn editing_a_pinned_note_keeps_it_pinned() {
        let d = tmp("pinedit");
        let mut s = Store::open(&d, 1000);
        let a = s.create(1000);
        s.save(a, "one", 1000);
        s.set_pinned(a, true);
        s.save(a, "two", 1001);
        let s = Store::open(&d, 1002);
        assert_eq!(s.notes.len(), 1, "no second copy");
        assert!(s.notes[0].pinned);
        assert_eq!(s.notes[0].text, "two");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn delete_restore_and_forever() {
        let d = tmp("trash");
        let mut s = Store::open(&d, 1000);
        let a = s.create(1000);
        s.save(a, "keep me", 1000);
        s.delete(a, 5000);
        assert!(s.notes.is_empty() && s.trashed.len() == 1);
        let s2 = Store::open(&d, 5001);
        assert_eq!(s2.trashed.len(), 1, "trash survives a restart");
        s.restore(a);
        assert_eq!(s.notes[0].text, "keep me");
        assert!(s.trashed.is_empty());
        s.delete(a, 6000);
        s.delete_forever(a);
        assert!(Store::open(&d, 6001).trashed.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_trash_is_purged_on_open() {
        let d = tmp("purge");
        let mut s = Store::open(&d, 1000);
        let a = s.create(1000);
        s.save(a, "gone soon", 1000);
        s.delete(a, 1000);
        let later = 1000 + TRASH_DAYS * 86_400 + 1;
        assert!(Store::open(&d, later).trashed.is_empty());
        assert_eq!(std::fs::read_dir(d.join(".trash")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn search_needs_every_word_in_any_order_ignoring_case() {
        let n = Note { id: 1, text: "Buy Milk\nand eggs".into(), modified: 0, pinned: false };
        assert!(n.matches("eggs milk"));
        assert!(n.matches("BUY"));
        assert!(!n.matches("milk bread"));
        assert!(n.matches("  "));
    }

    #[test]
    fn ids_never_repeat_even_in_the_same_second() {
        let d = tmp("ids");
        let mut s = Store::open(&d, 1000);
        let a = s.create(1000);
        let b = s.create(1000);
        assert_ne!(a, b);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stray_files_are_ignored() {
        let d = tmp("stray");
        std::fs::write(d.join("readme.txt"), "x").unwrap();
        std::fs::write(d.join("12.tmp"), "x").unwrap();
        assert!(Store::open(&d, 1).notes.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
