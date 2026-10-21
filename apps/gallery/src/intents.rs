//! Working with other apps: a photo or video opened from elsewhere ("Open with Gallery"), and the
//! picker other apps use to ask the user for a photo or video.

use super::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum PickKind {
    Any,
    Image,
    Video,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Pick {
    pub multiple: bool,
    pub kind: PickKind,
}

/// `"single;image/*"` or `"multiple;*/*"`, as handed over by the activity.
pub(crate) fn parse_pick(s: &str) -> Pick {
    let (mode, ty) = s.split_once(';').unwrap_or((s, "*/*"));
    let kind = if ty.starts_with("image/") {
        PickKind::Image
    } else if ty.starts_with("video/") {
        PickKind::Video
    } else {
        PickKind::Any
    };
    Pick { multiple: mode == "multiple", kind }
}

impl Gallery {
    /// Ask the activity whether another app sent something (about three times a second).
    pub(crate) fn poll_intents(&mut self, _ctx: &Context) {
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            if self.last_intent_poll.elapsed() < Duration::from_millis(300) {
                return;
            }
            self.last_intent_poll = Instant::now();
            if self.pick.is_none() {
                if let Some(s) = perm::take_pick(&app) {
                    self.start_pick(_ctx, parse_pick(&s));
                }
            }
            if let Some(p) = perm::take_view(&app) {
                self.open_external(_ctx, PathBuf::from(p));
            }
        }
    }

    /// Show a photo or video that another app opened with us.
    pub(crate) fn open_external(&mut self, ctx: &Context, path: PathBuf) {
        if !(is_image(&path) || media::is_video(&path)) {
            self.say(ctx, "Gallery can't open that kind of file.", None);
            return;
        }
        if self.pick.is_some() {
            return; // busy answering a picker request
        }
        let id = match self.items.iter().position(|i| i.path == path && !i.gone) {
            Some(id) => id,
            None => {
                let meta = std::fs::metadata(&path);
                self.items.push(Item {
                    video: media::is_video(&path),
                    mtime: meta.as_ref().ok().and_then(|m| m.modified().ok()).unwrap_or(UNIX_EPOCH),
                    size: meta.map(|m| m.len()).unwrap_or(0),
                    album: EXT_KEY.into(),
                    path,
                    trashed: false,
                    hidden: false,
                    orig: None,
                    gone: false,
                });
                self.items.len() - 1
            }
        };
        if self.items[id].hidden {
            return;
        }
        self.settings_open = false;
        self.exit_select();
        self.close_viewer();
        self.tab = Tab::Photos;
        self.album_open = false;
        self.filter = if self.items[id].album == EXT_KEY { Some(EXT_KEY.to_string()) } else { None };
        self.rebuild_view();
        if let Some(pos) = self.view.iter().position(|&i| i == id) {
            self.open_viewer(pos);
            self.external_viewer = true;
        }
    }

    /// Back / Close in the viewer: when another app sent us here, go back to it.
    pub(crate) fn leave_viewer(&mut self, ctx: &Context) {
        if self.external_viewer {
            self.external_viewer = false;
            self.exit(ctx);
        } else {
            self.close_viewer();
        }
    }

    pub(crate) fn start_pick(&mut self, _ctx: &Context, pick: Pick) {
        self.pick = Some(pick);
        self.settings_open = false;
        self.exit_select();
        self.go_to_tab(Tab::Photos);
        if pick.multiple {
            self.select = Some(HashSet::new());
        }
    }

    pub(crate) fn finish_pick(&mut self, ctx: &Context, ids: &[usize]) {
        let paths: Vec<String> = ids.iter().filter(|&&i| !self.items[i].hidden).map(|&i| self.items[i].path.to_string_lossy().into_owned()).collect();
        self.pick = None;
        self.exit_select();
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            perm::finish_pick(&app, &paths); // sets the result; closing the window loop finishes the activity
            self.exit(ctx);
            return;
        }
        self.say(ctx, format!("Picked {} item(s)", paths.len()), None);
        self.rebuild_view();
    }

    pub(crate) fn cancel_pick(&mut self) {
        self.pick = None;
        self.exit_select();
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            perm::cancel_pick(&app);
            let ctx = self.ctx.clone();
            self.exit(&ctx);
        }
        self.rebuild_view();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_requests_are_understood() {
        assert_eq!(parse_pick("single;image/*"), Pick { multiple: false, kind: PickKind::Image });
        assert_eq!(parse_pick("multiple;video/mp4"), Pick { multiple: true, kind: PickKind::Video });
        assert_eq!(parse_pick("multiple;*/*"), Pick { multiple: true, kind: PickKind::Any });
        assert_eq!(parse_pick("garbage"), Pick { multiple: false, kind: PickKind::Any });
    }
}
