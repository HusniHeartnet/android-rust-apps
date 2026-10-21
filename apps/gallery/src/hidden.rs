//! The hidden vault: photos and videos moved into a dot-folder that no gallery or media picker
//! lists, shown in this app only after the phone's fingerprint or screen lock is confirmed.
//!
//! The files are moved, not copied or encrypted: they are out of every other gallery and picker,
//! and out of Android's media database, but anyone who browses the phone's files with hidden
//! folders shown can still see them. The lock protects the view inside this app.

use super::*;

impl Gallery {
    /// Close the vault: forget its thumbnails and leave any screen that shows its contents.
    pub(crate) fn lock_hidden(&mut self, _ctx: &Context) {
        if !self.hidden_unlocked {
            return;
        }
        self.hidden_unlocked = false;
        if self.filter.as_deref() == Some(HIDDEN_KEY) {
            self.close_viewer();
            self.exit_select();
            self.album_open = false;
            self.filter = None;
        }
        let vault: HashSet<usize> = (0..self.items.len()).filter(|&i| self.items[i].hidden).collect();
        self.thumbs.retain(|id, _| !vault.contains(id));
        self.durations.retain(|id, _| !vault.contains(id));
        self.failed.retain(|id| !vault.contains(id));
        self.rebuild_view();
        self.update_secure();
    }

    /// Ask for the phone's fingerprint or screen lock; `poll_auth` picks up the answer.
    pub(crate) fn request_unlock(&mut self, ctx: &Context) {
        if self.hidden_unlocked {
            self.open_hidden_album();
            return;
        }
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            self.auth_pending = true;
            perm::authenticate(&app, "Hidden album");
            return;
        }
        // desktop preview: no phone lock to ask for
        let _ = ctx;
        self.hidden_unlocked = true;
        self.open_hidden_album();
    }

    pub(crate) fn poll_auth(&mut self, ctx: &Context) {
        if !self.auth_pending {
            return;
        }
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            match perm::take_auth(&app) {
                0 => ctx.request_repaint_after(Duration::from_millis(200)),
                1 => {
                    self.auth_pending = false;
                    self.hidden_unlocked = true;
                    self.open_hidden_album();
                }
                3 => {
                    self.auth_pending = false;
                    self.say(ctx, "Set a screen lock (PIN, pattern or fingerprint) in Android settings first.", None);
                }
                _ => self.auth_pending = false,
            }
        }
    }

    fn open_hidden_album(&mut self) {
        self.settings_open = false;
        self.open_album(HIDDEN_KEY);
        self.update_secure();
    }

    /// Block screenshots and blank the app-switcher card while the vault is on screen.
    pub(crate) fn update_secure(&mut self) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android {
            perm::set_secure(app, self.hidden_unlocked && (self.filter.as_deref() == Some(HIDDEN_KEY)));
        }
    }

    /// Move items into the vault.
    pub(crate) fn hide_items(&mut self, ctx: &Context, ids: &[usize]) -> usize {
        if !self.manage_ok {
            self.manage_prompt = true;
            return 0;
        }
        self.remember_position();
        self.player = None;
        if std::fs::create_dir_all(&self.hidden_dir).is_err() {
            self.say(ctx, "Could not create the hidden folder.", None);
            return 0;
        }
        // a .nomedia file keeps Android's media scanner and every gallery out of the folder
        let _ = std::fs::write(self.hidden_dir.join(".nomedia"), b"");
        let (mut done, mut failed) = (Vec::new(), 0);
        let mut old_paths = Vec::new();
        for &id in ids {
            let it = &self.items[id];
            if it.trashed || it.hidden || it.gone {
                continue;
            }
            let (old, key) = (it.path.clone(), thumbcache::key(&it.path, it.mtime, it.size));
            match trash::move_to_trash(&old, &self.hidden_dir) {
                Ok(dest) => {
                    thumbcache::forget(&self.shared.cache_dir, key); // no trace of it in the thumbnail cache
                    self.thumbs.remove(&id);
                    self.favs.remove(&old);
                    let it = &mut self.items[id];
                    it.orig = Some(old.clone());
                    it.path = dest;
                    it.hidden = true;
                    it.album = HIDDEN_KEY.into();
                    old_paths.push(old.to_string_lossy().into_owned());
                    done.push(id);
                }
                Err(_) => failed += 1,
            }
        }
        self.save_favs();
        self.rebuild_view();
        self.notify_media(&old_paths);
        let n = done.len();
        let msg = if failed == 0 { format!("Hidden {n}. Find them in Albums › Hidden.") } else { format!("Hidden {n}, {failed} failed") };
        self.say(ctx, msg, None);
        n
    }

    /// Take items out of the vault, back to where they came from.
    pub(crate) fn unhide_items(&mut self, ctx: &Context, ids: &[usize]) -> usize {
        self.remember_position();
        self.player = None;
        let mut new_paths = Vec::new();
        let mut n = 0;
        for &id in ids {
            if !self.items[id].hidden || self.items[id].gone {
                continue;
            }
            if let Ok(dest) = trash::restore(&self.items[id].path, &self.hidden_dir) {
                self.thumbs.remove(&id);
                let it = &mut self.items[id];
                it.album = dest.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                new_paths.push(dest.to_string_lossy().into_owned());
                it.path = dest;
                it.hidden = false;
                it.orig = None;
                n += 1;
            }
        }
        self.rebuild_view();
        self.notify_media(&new_paths);
        self.say(ctx, if n == 1 { "Unhidden".to_string() } else { format!("Unhidden {n}") }, None);
        n
    }

    /// Tell Android's media database about files that appeared or disappeared, so other apps'
    /// galleries and pickers stay right.
    pub(crate) fn notify_media(&self, _paths: &[String]) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android {
            perm::media_scan(app, _paths);
        }
    }
}
