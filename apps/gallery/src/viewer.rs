//! Full-screen viewer: a pager (the neighbouring photo or video slides in beside the current one
//! while you drag), pinch/double-tap zoom for photos, in-app video controls, and the Float button.

use super::*;

const FULL_UV: Rect = Rect { min: pos2(0.0, 0.0), max: pos2(1.0, 1.0) };
const SETTLE_SECS: f64 = 0.2;
/// Height of the bottom action row (Share, Favourite, Delete, More), without the navigation bar.
const ACTION_ROW_H: f32 = 62.0;

fn fit(page: Rect, w: f32, h: f32) -> Rect {
    let s = (page.width() / w.max(1.0)).min(page.height() / h.max(1.0));
    Rect::from_center_size(page.center(), vec2(w * s, h * s))
}

impl Gallery {
    /// Decode the neighbours ahead of time, so swiping lands on a picture that is already there.
    pub(crate) fn prefetch(&mut self, pos: usize) {
        // every thumbnail wanted by the viewer must be decoded, wherever the grid was scrolled to
        self.shared.vis_lo.store(0, Ordering::Relaxed);
        self.shared.vis_hi.store(usize::MAX / 4, Ordering::Relaxed);

        let at = |p: isize| if p >= 0 { self.view.get(p as usize).copied() } else { None };
        // current last, so the stack-style job queue decodes it first
        let around: Vec<usize> = [at(pos as isize - 1), at(pos as isize + 1), at(pos as isize)].into_iter().flatten().collect();
        let wanted: HashSet<usize> = around.iter().copied().filter(|&id| !self.items[id].video).collect();

        *self.shared.wanted.lock().unwrap() = wanted.clone();
        self.full.retain(|id, _| wanted.contains(id));
        self.full_requested.retain(|id| wanted.contains(id));
        for id in around {
            self.request_thumb(id, 0);
            if wanted.contains(&id) && !self.full.contains_key(&id) && !self.full_failed.contains(&id) && self.full_requested.insert(id) {
                let path = self.items[id].path.clone();
                self.shared.jobs.lock().unwrap().push(Job::Full { id, path });
            }
        }
        self.shared.cv.notify_all();
    }

    /// Keep the position of a long video, so opening it again carries on from there.
    pub(crate) fn remember_position(&mut self) {
        let Some((pid, p)) = &self.player else { return };
        let (pos, dur) = (p.position_ms(), p.duration_ms());
        let path = self.items[*pid].path.clone();
        if dur > 60_000 && pos > 5_000 && pos < dur - 5_000 {
            self.resume.insert(path, pos);
        } else if self.resume.remove(&path).is_none() {
            return;
        }
        if self.resume.len() > 300 {
            let drop_keys: Vec<PathBuf> = self.resume.keys().take(50).cloned().collect();
            for k in drop_keys {
                self.resume.remove(&k);
            }
        }
        let lines: Vec<String> = self.resume.iter().map(|(p, ms)| format!("{ms}\t{}", p.to_string_lossy())).collect();
        write_lines(&self.data_dir, "resume.txt", &lines);
    }

    pub(crate) fn open_viewer(&mut self, pos: usize) {
        let id = self.view[pos];
        self.remember_position();
        self.player = None; // releases the previous video, if any
        self.pending_player = None;
        self.confirm_forever = None;
        self.viewer_menu = false;
        self.details_open = false;
        self.prefetch(pos);
        let item = &self.items[id];
        if item.video {
            // opening a video takes a moment; do it off the UI thread so swiping stays smooth
            let (path, ctx) = (item.path.clone(), self.ctx.clone());
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(media::Player::open(&path));
                ctx.request_repaint();
            });
            self.pending_player = Some((id, rx));
        }
        self.viewer = Some(Viewer { pos, id, failed: false, zoom: 1.0, offset: Vec2::ZERO, swipe: 0.0, anim: None, scrub: None, scrub_was_playing: false, scrub_last_seek: f64::MIN, gesture: 0, drag_origin: None, skip_acc: 0, press: None, last_tap: None, skip_last: f64::MIN, skip_side: 0, skip_base: 0 });
    }

    pub(crate) fn close_viewer(&mut self) {
        self.remember_position();
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            if self.brightness.take().is_some() {
                perm::set_brightness(&app, -1.0);
            }
            if std::mem::take(&mut self.landscape) {
                perm::set_orientation(&app, -1);
            }
        }
        self.viewer = None;
        self.player = None;
        self.pending_player = None;
        self.confirm_forever = None;
        self.bars_visible = true;
        self.viewer_menu = false;
        self.details_open = false;
        self.shared.wanted.lock().unwrap().clear();
        self.full.clear();
        self.full_requested.clear();
    }

    /// After an item leaves the current view: show its neighbour, or go back to the grid.
    pub(crate) fn viewer_after_removal(&mut self, pos: usize) {
        if self.view.is_empty() {
            self.close_viewer();
        } else {
            self.open_viewer(pos.min(self.view.len() - 1));
        }
    }

    /// Pick up a video player that finished opening in the background.
    fn poll_player(&mut self, ctx: &Context) {
        let Some((pid, rx)) = &self.pending_player else { return };
        let pid = *pid;
        match rx.try_recv() {
            Ok(result) => {
                self.pending_player = None;
                match (self.viewer.as_mut().filter(|v| v.id == pid), result) {
                    (Some(_), Some(player)) => {
                        if !self.autoplay {
                            player.pause();
                        }
                        // carry on where a long video was left
                        if let Some(&ms) = self.resume.get(&self.items[pid].path) {
                            if player.duration_ms() > 60_000 {
                                player.seek(ms);
                            }
                        }
                        self.player = Some((pid, player));
                    }
                    (Some(v), None) => v.failed = true,
                    _ => {} // user already moved on; dropping the player releases it
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(40)),
            Err(_) => self.pending_player = None,
        }
    }

    fn page_texture(&self, id: usize) -> Option<TextureHandle> {
        self.full.get(&id).map(|(t, _)| t.clone()).or_else(|| self.thumbs.get(&id).cloned())
    }

    pub(crate) fn viewer_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.poll_player(&ctx);
        let Some(mut v) = self.viewer.take() else { return };
        let item = self.items[v.id].clone();
        let is_fav = self.is_fav(v.id);
        let mut action: Option<Action> = None;
        let mut step: i32 = 0;

        // The picture is centred on the whole screen; the bars float over it.
        let (top_inset, bottom_inset) = self.insets;
        let screen = ctx.viewport_rect();
        if !self.tap_hide {
            self.bars_visible = true;
        }
        let bars = self.bars_visible;
        let red = Color32::from_rgb(255, 120, 110);
        let mut open_settings = false;
        let mut share = false;
        if bars {
            egui::Area::new(egui::Id::new("viewer-header")).order(egui::Order::Foreground).fixed_pos(screen.min).show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(150))
                    .inner_margin(egui::Margin { left: 8, right: 8, top: (top_inset + 6.0) as i8, bottom: 6 })
                    .show(ui, |ui| {
                        ui.set_width(screen.width() - 16.0);
                        ui.horizontal(|ui| {
                            if ui.button("‹ Back").clicked() {
                                action = Some(Action::Close);
                            }
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(local_time(item.mtime)).strong());
                                ui.label(egui::RichText::new(format!("{} / {}", v.pos + 1, self.view.len())).small().weak());
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(egui::Button::new("Settings").min_size(vec2(40.0, 30.0))).clicked() {
                                    open_settings = true;
                                }
                            });
                        });
                    });
            });

            // bottom action row: Share, Favourite, Delete, More; Restore / Delete forever in the
            // trash; Share, Unhide, Delete, More in the hidden vault
            egui::Area::new(egui::Id::new("viewer-actions")).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, 0.0)).show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(170))
                    .inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: (bottom_inset + 6.0) as i8 })
                    .show(ui, |ui| {
                        ui.set_width(screen.width() - 16.0);
                        let cols = if item.trashed { 2 } else { 4 };
                        ui.columns(cols, |c| {
                            let h = ACTION_ROW_H - 12.0;
                            let mut b = |ui: &mut egui::Ui, text: egui::RichText| ui.add_sized([ui.available_width(), h], egui::Button::new(text)).clicked();
                            let armed = self.confirm_forever == Some(v.id);
                            if item.trashed {
                                if b(&mut c[0], "Restore".into()) {
                                    action = Some(Action::Restore);
                                }
                                let label = if armed { "Tap again to delete" } else { "Delete forever" };
                                if b(&mut c[1], egui::RichText::new(label).color(red)) {
                                    if armed {
                                        action = Some(Action::DeleteForever);
                                    } else {
                                        self.confirm_forever = Some(v.id);
                                    }
                                }
                            } else if item.hidden {
                                if b(&mut c[0], "Share".into()) {
                                    share = true;
                                }
                                if b(&mut c[1], "Unhide".into()) {
                                    action = Some(Action::Unhide);
                                }
                                if b(&mut c[2], egui::RichText::new(if armed { "Tap again" } else { "Delete" }).color(red)) {
                                    if armed {
                                        action = Some(Action::DeleteForever);
                                    } else {
                                        self.confirm_forever = Some(v.id);
                                    }
                                }
                                if b(&mut c[3], "More".into()) {
                                    self.viewer_menu = !self.viewer_menu;
                                }
                            } else {
                                if b(&mut c[0], "Share".into()) {
                                    share = true;
                                }
                                if b(&mut c[1], if is_fav { "Unfavourite" } else { "Favourite" }.into()) {
                                    action = Some(Action::ToggleFavourite);
                                }
                                if b(&mut c[2], egui::RichText::new("Delete").color(red)) {
                                    action = Some(Action::Trash);
                                }
                                if b(&mut c[3], "More".into()) {
                                    self.viewer_menu = !self.viewer_menu;
                                }
                            }
                        });
                    });
            });

            if self.viewer_menu && !item.trashed {
                let mut close = false;
                egui::Area::new(egui::Id::new("viewer-menu"))
                    .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-8.0, -(bottom_inset + ACTION_ROW_H + 10.0)))
                    .order(egui::Order::Foreground)
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_min_width(220.0);
                            let entry = |ui: &mut egui::Ui, text: &str| ui.add_sized([220.0, 38.0], egui::Button::new(text)).clicked();
                            if entry(ui, "Details") {
                                self.details_open = true;
                                close = true;
                            }
                            if !item.hidden {
                                if entry(ui, "Hide in private album") {
                                    action = Some(Action::Hide);
                                    close = true;
                                }
                                if entry(ui, "Move to album…") {
                                    action = Some(Action::MoveTo);
                                    close = true;
                                }
                                if entry(ui, "Copy to album…") {
                                    action = Some(Action::CopyTo);
                                    close = true;
                                }
                                if !item.video && entry(ui, "Set as wallpaper") {
                                    action = Some(Action::Wallpaper);
                                    close = true;
                                }
                            }
                            if item.video && !item.hidden && entry(ui, "Float over other apps") {
                                action = Some(Action::Float);
                                close = true;
                            }
                        });
                    });
                if close {
                    self.viewer_menu = false;
                }
            }
        }

        if self.details_open {
            if !item.video && self.details_exif.as_ref().map(|(i, _)| *i) != Some(v.id) {
                self.details_exif = Some((v.id, exif::read(&item.path)));
            }
            let exif = self.details_exif.as_ref().filter(|(i, _)| *i == v.id && !item.video).and_then(|(_, e)| e.as_ref());
            let mut extra: Vec<(&str, String)> = Vec::new();
            let mut gps = None;
            if let Some(e) = exif {
                extra.extend(e.camera().map(|c| ("Camera", c)));
                extra.extend(e.lens.clone().map(|l| ("Lens", l)));
                extra.extend(e.taken.clone().map(|t| ("Taken", t)));
                extra.extend(e.settings_line().map(|t| ("Exposure", t)));
                gps = e.gps;
            }
            let name = item.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let shown = item.orig.as_ref().and_then(|o| o.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
            let folder = item.orig.as_deref().unwrap_or(&item.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let size = std::fs::metadata(&item.path).map(|m| m.len()).unwrap_or(0);
            let dims = if item.video {
                self.player.as_ref().filter(|(pid, _)| *pid == v.id).map(|(_, p)| p.size()).map(|(w, h)| format!("{w}×{h}"))
            } else {
                self.full.get(&v.id).map(|(_, d)| format!("{}×{}", d[0], d[1]))
            };
            let length = if item.video { self.durations.get(&v.id).map(|d| media::format_duration(*d)) } else { None };
            let auto_delete = item.trashed.then(|| trash::trashed_at(&item.path)).flatten().map(|t| {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let left = (TRASH_DAYS * 86_400).saturating_sub(now.saturating_sub(t)).div_ceil(86_400);
                format!("in {left} day{}", if left == 1 { "" } else { "s" })
            });
            let mut close = false;
            let mut map = false;
            egui::Area::new(egui::Id::new("viewer-details")).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).order(egui::Order::Tooltip).show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(screen.width() - 48.0);
                    ui.label(egui::RichText::new("Details").strong().size(18.0));
                    ui.add_space(6.0);
                    let mut line = |k: &str, val: String| {
                        ui.label(egui::RichText::new(k).small().weak());
                        ui.label(val);
                        ui.add_space(4.0);
                    };
                    line("Name", shown);
                    line("Folder", folder);
                    line("Date", local_time(item.mtime));
                    line("Size", format!("{:.1} MB", size as f64 / 1_048_576.0));
                    if let Some(d) = dims {
                        line("Resolution", d);
                    }
                    if let Some(l) = length {
                        line("Length", l);
                    }
                    for (k, val) in extra {
                        line(k, val);
                    }
                    if let Some(g) = gps {
                        line("Location", format!("{:.5}, {:.5}", g.0, g.1));
                    }
                    if let Some(a) = auto_delete {
                        line("Deleted for good", a);
                    }
                    ui.horizontal(|ui| {
                        if gps.is_some() && ui.add_sized([ui.available_width() / 2.0 - 4.0, 36.0], egui::Button::new("Open map")).clicked() {
                            map = true;
                        }
                        if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("Close")).clicked() {
                            close = true;
                        }
                    });
                });
            });
            if map {
                #[cfg(target_os = "android")]
                if let (Some(app), Some((la, lo))) = (self.android.as_ref(), gps) {
                    perm::open_map(app, la, lo);
                }
            }
            if close {
                self.details_open = false;
            }
        }

        let area = screen;
        let resp = ui.interact(screen, ui.make_persistent_id("viewer-area"), Sense::click_and_drag());
        let now = ctx.input(|i| i.time);
        let w = area.width();
        let (has_prev, has_next) = (v.pos > 0, v.pos + 1 < self.view.len());
        let video = item.video;

        // ---- gestures ----
        let mut pinching = false;
        if let Some(mt) = ctx.multi_touch() {
            v.zoom = (v.zoom * mt.zoom_delta).clamp(1.0, 8.0);
            v.offset += mt.translation_delta;
            pinching = true;
        } else {
            let z = ctx.input(|i| i.zoom_delta());
            if z != 1.0 {
                v.zoom = (v.zoom * z).clamp(1.0, 8.0);
            }
        }
        if !video {
            if resp.double_clicked() {
                v.zoom = if v.zoom > 1.01 { 1.0 } else { 2.5 };
                v.offset = Vec2::ZERO;
            }
            if resp.clicked() {
                if self.viewer_menu {
                    self.viewer_menu = false;
                } else if self.tap_hide {
                    self.bars_visible = !self.bars_visible; // a double-tap flips twice, so it zooms without flicker
                }
            }
        }
        let zoomed = v.zoom > 1.01;
        if resp.drag_started() {
            // on a video an up/down drag is brightness or volume, a sideways one swipes to the next item
            v.gesture = if video && !zoomed { 0 } else { 1 };
            v.drag_origin = ctx.input(|i| i.pointer.press_origin());
        }
        if resp.dragged() && !pinching {
            if zoomed {
                v.offset += resp.drag_delta();
            } else if v.gesture == 0 {
                if let (Some(o), Some(p)) = (v.drag_origin, ctx.input(|i| i.pointer.interact_pos())) {
                    let d = p - o;
                    if d.x.abs() > 12.0 || d.y.abs() > 12.0 {
                        v.gesture = if d.y.abs() > d.x.abs() * 1.2 { 2 } else { 1 };
                        if v.gesture == 1 {
                            v.swipe += d.x;
                            v.anim = None;
                        }
                    }
                }
            } else if v.gesture == 1 {
                v.swipe += resp.drag_delta().x;
                v.anim = None;
            } else if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                self.video_gesture(&ctx, resp.drag_delta().y, p.x, screen);
            }
        }
        if resp.drag_stopped() {
            let vertical = v.gesture == 2;
            v.gesture = 0;
            if !zoomed && !pinching && !vertical {
                let vel = ctx.input(|i| i.pointer.velocity().x);
                let go_next = has_next && (v.swipe < -w * 0.25 || (vel < -700.0 && v.swipe < -8.0));
                let go_prev = has_prev && (v.swipe > w * 0.25 || (vel > 700.0 && v.swipe > 8.0));
                let target = if go_next { -w } else if go_prev { w } else { 0.0 };
                v.anim = Some((v.swipe, target, now));
            }
        }
        // settle animation: the page glides to the neighbour (or back), then we switch to it
        if let Some((from, to, t0)) = v.anim {
            let t = (((now - t0) / SETTLE_SECS) as f32).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - t).powi(3);
            v.swipe = from + (to - from) * eased;
            ctx.request_repaint();
            if t >= 1.0 {
                v.anim = None;
                if to < 0.0 {
                    step = 1;
                } else if to > 0.0 {
                    step = -1;
                } else {
                    v.swipe = 0.0;
                }
            }
        }
        v.swipe = v.swipe.clamp(if has_next { -w } else { 0.0 }, if has_prev { w } else { 0.0 });

        // ---- pages ----
        let painter = ui.painter_at(area);
        if v.swipe.abs() > 0.5 {
            for rel in [-1i32, 1] {
                let p = v.pos as i32 + rel;
                if p < 0 || p as usize >= self.view.len() {
                    continue;
                }
                let id = self.view[p as usize];
                let page = Rect::from_min_size(area.min + vec2(rel as f32 * w + v.swipe, 0.0), area.size());
                if let Some(tex) = self.page_texture(id) {
                    let [tw, th] = tex.size();
                    painter.image(tex.id(), fit(page, tw as f32, th as f32), FULL_UV, Color32::WHITE);
                }
            }
        }
        let page = Rect::from_min_size(area.min + vec2(if zoomed { 0.0 } else { v.swipe }, 0.0), area.size());
        if video {
            self.video_page(ui, &painter, area, page, &resp, &mut v);
        } else {
            match self.page_texture(v.id) {
                Some(tex) => {
                    let [tw, th] = tex.size();
                    let base = fit(page, tw as f32, th as f32);
                    let size = base.size() * v.zoom;
                    let max_off = ((size - area.size()) / 2.0).max(Vec2::ZERO);
                    v.offset = if v.zoom <= 1.01 { Vec2::ZERO } else { v.offset.clamp(-max_off, max_off) };
                    painter.image(tex.id(), Rect::from_center_size(page.center() + v.offset, size), FULL_UV, Color32::WHITE);
                    if self.details_strip && self.bars_visible {
                        let name = item.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        painter.text(
                            pos2(area.min.x + 12.0, area.max.y - bottom_inset - ACTION_ROW_H - 10.0),
                            egui::Align2::LEFT_BOTTOM,
                            format!("{name}  ·  {}", local_time(item.mtime)),
                            egui::FontId::proportional(12.0),
                            Color32::from_gray(200),
                        );
                    }
                    if !self.full.contains_key(&v.id) && !self.full_failed.contains(&v.id) {
                        painter.text(area.center_bottom() - vec2(0.0, bottom_inset + 24.0), egui::Align2::CENTER_CENTER, "Loading…", egui::FontId::proportional(13.0), Color32::GRAY);
                    }
                }
                None => {
                    if self.full_failed.contains(&v.id) {
                        painter.text(area.center(), egui::Align2::CENTER_CENTER, "Can't open this image", egui::FontId::proportional(16.0), Color32::GRAY);
                    }
                }
            }
        }

        // the little readout while brightness or volume is being changed
        if let Some((text, frac, until)) = self.hud.clone() {
            if now < until {
                let r = Rect::from_center_size(area.center() - vec2(0.0, 120.0), vec2(180.0, 64.0));
                painter.rect_filled(r, 16.0, Color32::from_black_alpha(200));
                painter.text(r.center_top() + vec2(0.0, 20.0), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(15.0), Color32::WHITE);
                let track = Rect::from_min_max(pos2(r.min.x + 16.0, r.max.y - 18.0), pos2(r.max.x - 16.0, r.max.y - 12.0));
                painter.rect_filled(track, 3.0, Color32::from_white_alpha(70));
                painter.rect_filled(Rect::from_min_max(track.min, pos2(track.min.x + track.width() * frac.clamp(0.0, 1.0), track.max.y)), 3.0, Color32::WHITE);
                ctx.request_repaint_after(Duration::from_millis(100));
            } else {
                self.hud = None;
            }
        }

        if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            step = 1;
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            step = -1;
        }

        let (id, pos) = (v.id, v.pos);
        self.viewer = Some(v);
        if open_settings {
            self.open_settings();
            return;
        }
        if share {
            self.share_items(&ctx, &[id]);
        }
        match action {
            Some(Action::Close) => self.leave_viewer(&ctx),
            Some(Action::Hide) => {
                if self.hide_items(&ctx, &[id]) > 0 {
                    self.viewer_after_removal(pos);
                }
            }
            Some(Action::Unhide) => {
                if self.unhide_items(&ctx, &[id]) > 0 {
                    self.viewer_after_removal(pos);
                }
            }
            Some(Action::Wallpaper) => self.set_wallpaper(&ctx, id),
            Some(Action::MoveTo) => self.open_album_picker(AlbumOp::Move, vec![id]),
            Some(Action::CopyTo) => self.open_album_picker(AlbumOp::Copy, vec![id]),
            Some(Action::Float) => self.float_video(&ctx, id),
            Some(Action::ToggleFavourite) => {
                self.toggle_favourite(id);
                // the open item may have left the current view (Favourites album); keep showing it
                if let Some(p) = self.view.iter().position(|&i| i == id) {
                    if let Some(v) = self.viewer.as_mut() {
                        v.pos = p;
                    }
                } else {
                    self.viewer_after_removal(pos);
                }
            }
            Some(Action::Trash) => {
                if self.trash_item(&ctx, id) {
                    self.viewer_after_removal(pos);
                }
            }
            Some(Action::Restore) => {
                if self.restore_item(&ctx, id) {
                    self.viewer_after_removal(pos);
                }
            }
            Some(Action::DeleteForever) => {
                self.confirm_forever = None;
                if self.delete_forever_item(&ctx, id) {
                    self.viewer_after_removal(pos);
                }
            }
            None => {
                let next = pos as i32 + step;
                if step != 0 && (0..self.view.len() as i32).contains(&next) {
                    self.open_viewer(next as usize);
                }
            }
        }
    }

    /// The current page when it is a video: the picture, a poster while it loads, and the controls.
    fn video_page(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, area: Rect, page: Rect, resp: &egui::Response, v: &mut Viewer) {
        let ctx = ui.ctx().clone();
        let bottom_inset = self.insets.1;
        let player = self.player.as_ref().filter(|(pid, _)| *pid == v.id).map(|(_, p)| p);
        let Some(player) = player else {
            // poster frame while the player is still opening
            if let Some(tex) = self.page_texture(v.id) {
                let [tw, th] = tex.size();
                painter.image(tex.id(), fit(page, tw as f32, th as f32), FULL_UV, Color32::WHITE);
            }
            let text = if v.failed { "Can't play this video" } else { "Loading…" };
            painter.text(area.center_bottom() - vec2(0.0, bottom_inset + 24.0), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(14.0), Color32::GRAY);
            return;
        };

        let (vw, vh) = player.size();
        // Until this video's first frame is on screen, show its own thumbnail (the video itself
        // is not drawn yet, so nothing left over from the previous video can flash up).
        if !player.has_frame() {
            if let Some(tex) = self.page_texture(v.id) {
                let [tw, th] = tex.size();
                painter.image(tex.id(), fit(page, tw as f32, th as f32), FULL_UV, Color32::WHITE);
            }
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        // pinch zoom: the picture grows around the middle and can be dragged about
        let base = fit(page, vw as f32, vh as f32);
        let video_rect = if v.zoom > 1.01 {
            let size = base.size() * v.zoom;
            let max_off = ((size - area.size()) / 2.0).max(Vec2::ZERO);
            v.offset = v.offset.clamp(-max_off, max_off);
            Rect::from_center_size(base.center() + v.offset, size)
        } else {
            v.offset = Vec2::ZERO;
            base
        };
        painter.add(player.paint_callback(video_rect));

        let duration = player.duration_ms().max(1);
        let pos_ms = player.position_ms();
        let playing = player.is_playing();
        let ended = pos_ms >= duration - 300;
        player.set_looping(self.loop_video);
        player.set_speed(self.video_speed);
        player.set_muted(self.video_muted);
        let toggle_play = |player: &media::Player| {
            if player.is_playing() {
                player.pause();
            } else {
                if player.position_ms() >= player.duration_ms() - 300 {
                    player.seek(0);
                }
                player.play();
            }
        };

        // Tapping the middle of the video plays or pauses it; tapping anywhere else shows or hides
        // the bars (with "tap to hide" off, a tap anywhere plays or pauses, as before).
        let (mut flip_bars, mut close_menu) = (false, false);
        let mid = pos2(video_rect.center().x.clamp(area.min.x + 100.0, area.max.x - 100.0), video_rect.center().y.clamp(area.min.y + 100.0, area.max.y - 100.0));
        let center_hit = Rect::from_center_size(mid, vec2(76.0, 76.0));
        let tap_time = ctx.input(|i| i.time);
        // Two narrow strips along the left and right edges are for skipping only: a double-tap
        // there skips, and every further tap on the same side within a second skips again. Taps in
        // a strip never show or hide the bars and never play or pause.
        let strip_w = (area.width() * 0.18).clamp(56.0, 110.0);
        let strip_top = area.min.y + self.insets.0 + 70.0;
        let strip_bottom = area.max.y - bottom_inset - if self.bars_visible { ACTION_ROW_H + 190.0 } else { 40.0 };
        let strips_on = self.double_tap_skip && v.zoom <= 1.01 && strip_bottom > strip_top + 60.0;
        let left_strip = Rect::from_min_max(pos2(area.min.x, strip_top), pos2(area.min.x + strip_w, strip_bottom));
        let right_strip = Rect::from_min_max(pos2(area.max.x - strip_w, strip_top), pos2(area.max.x, strip_bottom));
        // Taps in the strips are read from the raw touches, not from egui's click detection: egui
        // only counts a double-tap when the second touch lands within 6 points of the first, but
        // real double-taps land 8 to 25 points apart, so most of them were missed. Here any two quick
        // taps in the same strip count, wherever in the strip they land.
        let touches: Vec<(egui::Pos2, bool)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, .. } => Some((*pos, *pressed)),
                    _ => None,
                })
                .collect()
        });
        for (pos, pressed) in touches {
            if pressed {
                v.press = Some((tap_time, pos));
                continue;
            }
            let Some((t0, p0)) = v.press.take() else { continue };
            let is_tap = tap_time - t0 < 0.45 && (pos - p0).length() < 40.0;
            if !(is_tap && strips_on && !self.viewer_menu) {
                continue;
            }
            let side: i8 = if left_strip.contains(p0) { -1 } else if right_strip.contains(p0) { 1 } else { 0 };
            if side == 0 {
                continue;
            }
            let burst = v.skip_acc != 0 && v.skip_side == side && tap_time - v.skip_last < 1.0;
            let double = v.last_tap.is_some_and(|(t, s)| s == side && tap_time - t < 0.6);
            if burst || double {
                if !burst {
                    v.skip_base = pos_ms;
                    v.skip_acc = 0;
                }
                v.skip_side = side;
                v.skip_acc += i32::from(side) * self.skip_secs;
                v.skip_last = tap_time;
                v.last_tap = None;
                player.seek((v.skip_base + v.skip_acc * 1000).clamp(0, duration));
                v.scrub_last_seek = tap_time; // keeps frames coming while the seek lands
            } else {
                v.last_tap = Some((tap_time, side));
            }
        }

        // everything else: tapping the middle plays or pauses, tapping elsewhere shows or hides the
        // bars; a tap in a strip never does either
        if resp.clicked() {
            let at = resp.interact_pointer_pos().or_else(|| ctx.input(|i| i.pointer.latest_pos()));
            let in_strip = strips_on && at.is_some_and(|p| left_strip.contains(p) || right_strip.contains(p));
            if in_strip && !self.viewer_menu {
                // handled above
            } else if self.viewer_menu {
                close_menu = true;
            } else if !self.tap_hide || (self.bars_visible && at.is_some_and(|p| center_hit.contains(p))) {
                toggle_play(player);
            } else {
                flip_bars = true;
            }
        }

        // "+10s" / "-20s" in the strip that was tapped, fading after a moment
        if v.skip_acc != 0 && tap_time - v.skip_last < 0.9 {
            let strip = if v.skip_side < 0 { left_strip } else { right_strip };
            let c = pos2(strip.center().x, area.center().y);
            painter.circle_filled(c, 52.0, Color32::from_white_alpha(46));
            painter.text(c, egui::Align2::CENTER_CENTER, format!("{}{}s", if v.skip_acc > 0 { "+" } else { "-" }, v.skip_acc.abs()), egui::FontId::proportional(24.0), Color32::WHITE);
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let show_controls = self.bars_visible && v.swipe.abs() < 2.0;

        // the big play / pause mark in the middle of the video
        if show_controls {
            let c = center_hit.center();
            painter.circle_filled(c, 38.0, Color32::from_black_alpha(120));
            if playing {
                for dx in [-8.0f32, 8.0] {
                    painter.rect_filled(Rect::from_center_size(c + vec2(dx, 0.0), vec2(10.0, 34.0)), 2.0, Color32::WHITE);
                }
            } else {
                painter.add(egui::Shape::convex_polygon(vec![c + vec2(-11.0, -19.0), c + vec2(-11.0, 19.0), c + vec2(21.0, 0.0)], Color32::WHITE, egui::Stroke::NONE));
            }
        }

        // Controls (hidden while swiping between items): a row with play and the time, a row of
        // chips (speed, loop, mute, rotate), and the seek bar, whose whole 64-point-tall strip
        // takes the finger so dropping the video at the right spot needs no precise touch.
        let (mut flip_loop, mut flip_speed, mut flip_mute, mut flip_rotate) = (false, false, false, false);
        if show_controls {
            let now = ctx.input(|i| i.time);
            let bottom = area.max.y - bottom_inset - ACTION_ROW_H;
            let bar = Rect::from_min_max(pos2(area.min.x, bottom - 178.0), pos2(area.max.x, bottom));
            painter.rect_filled(Rect::from_min_max(bar.min, pos2(area.max.x, bottom)), 0.0, Color32::from_black_alpha(150));
            let white = Color32::WHITE;

            // play / pause
            let play_rect = Rect::from_min_size(bar.min + vec2(10.0, 6.0), vec2(56.0, 56.0));
            let pr = ui.interact(play_rect, ui.make_persistent_id("v-play"), Sense::click());
            let c = play_rect.center();
            if pr.is_pointer_button_down_on() {
                painter.circle_filled(c, 28.0, Color32::from_white_alpha(40));
            }
            if playing {
                for dx in [-6.5f32, 6.5] {
                    painter.rect_filled(Rect::from_center_size(c + vec2(dx, 0.0), vec2(8.0, 26.0)), 2.0, white);
                }
            } else if ended && !self.loop_video {
                painter.circle_stroke(c, 12.0, egui::Stroke::new(3.0, white)); // replay
                painter.add(egui::Shape::convex_polygon(vec![c + vec2(2.0, -17.0), c + vec2(2.0, -5.0), c + vec2(11.0, -11.0)], white, egui::Stroke::NONE));
            } else {
                painter.add(egui::Shape::convex_polygon(vec![c + vec2(-9.0, -15.0), c + vec2(-9.0, 15.0), c + vec2(15.0, 0.0)], white, egui::Stroke::NONE));
            }
            if pr.clicked() {
                toggle_play(player);
            }

            // time
            let shown = v.scrub.unwrap_or(pos_ms as f32).clamp(0.0, duration as f32);
            painter.text(
                pos2(play_rect.max.x + 8.0, c.y),
                egui::Align2::LEFT_CENTER,
                format!("{} / {}", media::format_duration(shown as u64), media::format_duration(duration as u64)),
                egui::FontId::proportional(16.0),
                white,
            );

            // chips
            let chip = |ui: &egui::Ui, rect: Rect, id: &str, label: &str, on: bool| -> bool {
                let r = ui.interact(rect, ui.make_persistent_id(id), Sense::click());
                let fill = if on { Color32::from_rgb(50, 110, 220) } else if r.is_pointer_button_down_on() { Color32::from_gray(70) } else { Color32::from_gray(48) };
                painter.rect_filled(rect, 18.0, fill);
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(15.0), Color32::WHITE);
                r.clicked()
            };
            let chip_w = (bar.width() - 20.0 - 18.0) / 4.0;
            let chip_at = |k: usize| Rect::from_min_size(pos2(bar.min.x + 10.0 + k as f32 * (chip_w + 6.0), bar.min.y + 68.0), vec2(chip_w, 36.0));
            let speed_label = format!("{}x", self.video_speed);
            flip_speed = chip(ui, chip_at(0), "v-speed", &speed_label, (self.video_speed - 1.0).abs() > 0.01);
            flip_loop = chip(ui, chip_at(1), "v-loop", "Loop", self.loop_video);
            flip_mute = chip(ui, chip_at(2), "v-mute", if self.video_muted { "Muted" } else { "Mute" }, self.video_muted);
            flip_rotate = chip(ui, chip_at(3), "v-rotate", "Rotate", self.landscape);

            // seek bar
            let zone = Rect::from_min_size(pos2(bar.min.x, bar.min.y + 108.0), vec2(bar.width(), 64.0));
            let sr = ui.interact(zone, ui.make_persistent_id("v-seek"), Sense::click_and_drag());
            let track = Rect::from_min_max(pos2(zone.min.x + 24.0, zone.center().y - 3.0), pos2(zone.max.x - 24.0, zone.center().y + 3.0));
            let pressed = sr.is_pointer_button_down_on() || sr.dragged();
            if pressed {
                if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                    let ms = ((p.x - track.min.x) / track.width()).clamp(0.0, 1.0) * duration as f32;
                    if v.scrub.is_none() {
                        v.scrub_was_playing = playing;
                        if playing {
                            player.pause();
                        }
                    }
                    v.scrub = Some(ms);
                    // live preview: ask for the picture under the finger, about twelve times a second
                    if now - v.scrub_last_seek > 0.08 {
                        player.seek(ms as i32);
                        v.scrub_last_seek = now;
                    }
                }
            } else if let Some(ms) = v.scrub.take() {
                player.seek(ms as i32);
                v.scrub_last_seek = now;
                if v.scrub_was_playing {
                    player.play();
                }
            }
            let frac = (v.scrub.unwrap_or(pos_ms as f32) / duration as f32).clamp(0.0, 1.0);
            let x = track.min.x + track.width() * frac;
            painter.rect_filled(track, 3.0, Color32::from_white_alpha(70));
            painter.rect_filled(Rect::from_min_max(track.min, pos2(x, track.max.y)), 3.0, white);
            painter.circle_filled(pos2(x, track.center().y), if pressed { 13.0 } else { 9.0 }, white);
            if v.scrub.is_some() {
                // a bubble with the time above the finger
                let label = media::format_duration(shown as u64);
                let galley = painter.layout_no_wrap(label, egui::FontId::proportional(18.0), white);
                let size = galley.size() + vec2(20.0, 10.0);
                let cx = x.clamp(area.min.x + size.x / 2.0 + 6.0, area.max.x - size.x / 2.0 - 6.0);
                let bubble = Rect::from_center_size(pos2(cx, zone.min.y - 18.0), size);
                painter.rect_filled(bubble, 12.0, Color32::from_black_alpha(200));
                painter.galley(bubble.center() - galley.size() / 2.0, galley, white);
            }
            if now - v.scrub_last_seek < 0.7 {
                ctx.request_repaint(); // the frame for a seek arrives a moment later
            }
        }
        if flip_loop {
            self.loop_video = !self.loop_video;
            self.save_settings();
        }
        if flip_speed {
            const SPEEDS: [f32; 4] = [1.0, 1.5, 2.0, 0.5];
            let i = SPEEDS.iter().position(|s| (s - self.video_speed).abs() < 0.01).unwrap_or(0);
            self.video_speed = SPEEDS[(i + 1) % SPEEDS.len()];
        }
        if flip_mute {
            self.video_muted = !self.video_muted;
        }
        if flip_rotate {
            self.landscape = !self.landscape;
            #[cfg(target_os = "android")]
            if let Some(app) = &self.android {
                perm::set_orientation(app, if self.landscape { 0 } else { -1 });
            }
        }
        if flip_bars {
            self.bars_visible = !self.bars_visible;
        }
        if close_menu {
            self.viewer_menu = false;
        }
        if playing || v.scrub.is_some() || v.swipe != 0.0 {
            ctx.request_repaint(); // keep drawing frames
        }
    }

    /// Up/down drag on a video: the left half changes the screen brightness, the right half the volume.
    fn video_gesture(&mut self, ctx: &Context, dy: f32, x: f32, screen: Rect) {
        let _ = (dy, x, screen);
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            let amount = -dy / (screen.height() * 0.55);
            let until = ctx.input(|i| i.time) + 1.2;
            if x < screen.center().x {
                let cur = *self.brightness.get_or_insert_with(|| perm::get_brightness(&app).unwrap_or(0.5));
                let n = (cur + amount).clamp(0.02, 1.0);
                self.brightness = Some(n);
                perm::set_brightness(&app, n);
                self.hud = Some((format!("Brightness  {}%", (n * 100.0).round()), n, until));
            } else {
                let cur = *self.volume.get_or_insert_with(|| perm::get_volume(&app).unwrap_or(0.5));
                let n = (cur + amount).clamp(0.0, 1.0);
                self.volume = Some(n);
                perm::set_volume(&app, n);
                self.hud = Some((format!("Volume  {}%", (n * 100.0).round()), n, until));
            }
            ctx.request_repaint();
        }
    }

    fn set_wallpaper(&mut self, ctx: &Context, _id: usize) {
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            perm::set_wallpaper(&app, &self.items[_id].path.to_string_lossy());
            return;
        }
        self.say(ctx, "Setting a wallpaper is available on the phone.", None);
    }

    /// Hand the video to the floating window and send the gallery to the background.
    fn float_video(&mut self, ctx: &Context, _id: usize) {
        self.remember_position();
        #[cfg(target_os = "android")]
        if let Some(app) = self.android.clone() {
            if !perm::can_draw_overlays(&app) {
                perm::open_overlay_settings(&app);
                self.say(ctx, "Allow \"Display over other apps\" for Gallery, then tap Float again.", None);
                return;
            }
            let pos = self.player.as_ref().filter(|(pid, _)| *pid == _id).map_or(0, |(_, p)| p.position_ms());
            let path = self.items[_id].path.to_string_lossy().into_owned();
            self.player = None; // stop the in-app copy; the floating window takes over from here
            if perm::start_floating(&app, &path, pos) {
                perm::move_task_to_back(&app);
            } else {
                self.say(ctx, "Could not start the floating player.", None);
            }
            return;
        }
        self.say(ctx, "The floating player is available on the phone.", None);
    }
}
