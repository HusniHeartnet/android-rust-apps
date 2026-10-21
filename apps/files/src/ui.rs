//! Everything drawn on screen.

use super::*;
use egui::{Align2, FontId, Sense, Stroke};
use fsops::human_size;

// ---------- small drawing helpers ----------

pub(crate) fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Folder => Color32::from_rgb(245, 180, 60),
        Kind::Image => Color32::from_rgb(76, 175, 80),
        Kind::Video => Color32::from_rgb(156, 39, 176),
        Kind::Audio => Color32::from_rgb(233, 30, 99),
        Kind::Document => Color32::from_rgb(66, 133, 244),
        Kind::Archive => Color32::from_rgb(141, 110, 99),
        Kind::Apk => Color32::from_rgb(0, 150, 136),
        Kind::Other => Color32::from_rgb(120, 124, 135),
    }
}

/// A folder shape, or a coloured page with the file's extension on it.
pub(crate) fn draw_icon(p: &egui::Painter, rect: Rect, kind: Kind, name: &str) {
    let col = kind_color(kind);
    if kind == Kind::Folder {
        let body = Rect::from_min_max(rect.min + vec2(2.0, rect.height() * 0.24), rect.max - vec2(2.0, 4.0));
        let tab = Rect::from_min_size(rect.min + vec2(2.0, rect.height() * 0.12), vec2(rect.width() * 0.42, rect.height() * 0.2));
        p.rect_filled(tab, 4.0, col.gamma_multiply(0.8));
        p.rect_filled(body, 6.0, col);
        return;
    }
    let page = rect.shrink2(vec2(6.0, 2.0));
    p.rect_filled(page, 6.0, col);
    let fold = 11.0;
    p.add(egui::Shape::convex_polygon(
        vec![pos2(page.max.x - fold, page.min.y), pos2(page.max.x, page.min.y + fold), pos2(page.max.x - fold, page.min.y + fold)],
        Color32::from_black_alpha(70),
        Stroke::NONE,
    ));
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
    let label: String = ext.chars().take(4).collect();
    if !label.is_empty() {
        p.text(page.center() + vec2(0.0, 5.0), Align2::CENTER_CENTER, label, FontId::proportional(11.5), Color32::WHITE);
    }
}

fn check_circle(p: &egui::Painter, c: Pos2, on: bool) {
    if on {
        p.circle_filled(c, 12.0, BRAND);
        let tick = Stroke::new(2.2, Color32::WHITE);
        p.line_segment([c + vec2(-5.5, 0.0), c + vec2(-1.5, 4.5)], tick);
        p.line_segment([c + vec2(-1.5, 4.5), c + vec2(6.0, -4.5)], tick);
    } else {
        p.circle_stroke(c, 12.0, Stroke::new(1.6, Color32::from_gray(150)));
    }
}

fn button(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(egui::Button::new(text).min_size(vec2(44.0, 34.0))).clicked()
}

fn card(ui: &mut egui::Ui, rect: Rect, pressed: bool) {
    ui.painter().rect_filled(rect, 14.0, if pressed { Color32::from_gray(44) } else { CARD });
}

// ---------- the frame ----------

pub(crate) fn draw(app: &mut Files, ui: &mut egui::Ui, ctx: &Context, top: f32, bottom: f32) {
    if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::BrowserBack)) {
        go_back(app, ctx);
    }
    let margin = egui::Margin { left: 0, right: 0, top: top.round() as i8, bottom: bottom.round() as i8 };
    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(margin)).show(ui, |ui| {
        if !app.access_ok {
            permission_screen(app, ui);
            return;
        }
        match app.current().clone() {
            Screen::Home => home(app, ui),
            Screen::Settings => settings(app, ui),
            _ => list_screen(app, ui),
        }
    });
    if app.access_ok {
        overlays(app, ctx, bottom);
    }
}

fn go_back(app: &mut Files, ctx: &Context) {
    if app.dialog.is_some() {
        app.dialog = None;
    } else if app.more_menu || app.header_menu {
        app.more_menu = false;
        app.header_menu = false;
    } else if app.select.is_some() {
        app.exit_select();
    } else if app.job.is_some() {
        // a running job is cancelled with its own button
    } else if !app.back() {
        app.exit(ctx);
    }
}

fn permission_screen(app: &mut Files, ui: &mut egui::Ui) {
    ui.add_space(80.0);
    ui.vertical_centered(|ui| {
        ui.heading("Files needs access to all files");
        ui.add_space(8.0);
        ui.label("This is how it can show and manage everything on your phone's storage. Nothing leaves the phone.");
        ui.add_space(16.0);
        #[cfg(target_os = "android")]
        if let Some(a) = &app.android {
            if ui.add_sized([200.0, 44.0], egui::Button::new("Allow access")).clicked() {
                perm::open_all_files_settings(a);
            }
        }
        let _ = &app;
    });
}

// ---------- home ----------

fn home(app: &mut Files, ui: &mut egui::Ui) {
    let mut go: Option<Screen> = None;
    let mut search = false;
    egui::ScrollArea::vertical().id_salt("home").auto_shrink([false; 2]).show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.heading("Files");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                if button(ui, "Settings") {
                    go = Some(Screen::Settings);
                }
                if button(ui, "Search") {
                    search = true;
                }
            });
        });
        ui.add_space(10.0);
        let margin = 14.0;
        let width = ui.available_width() - 2.0 * margin;

        // storage
        if let Some((total, free)) = app.storage {
            let used = total.saturating_sub(free);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), Sense::hover());
            let rect = Rect::from_min_size(r.min + vec2(margin, 0.0), vec2(width, 90.0));
            card(ui, rect, false);
            let p = ui.painter();
            p.text(rect.min + vec2(16.0, 14.0), Align2::LEFT_TOP, "Internal storage", FontId::proportional(15.0), Color32::from_gray(200));
            p.text(rect.min + vec2(16.0, 36.0), Align2::LEFT_TOP, format!("{} used of {}", human_size(used), human_size(total)), FontId::proportional(20.0), Color32::WHITE);
            let bar = Rect::from_min_size(rect.min + vec2(16.0, 68.0), vec2(width - 32.0, 8.0));
            p.rect_filled(bar, 4.0, Color32::from_gray(55));
            let frac = (used as f32 / total.max(1) as f32).clamp(0.0, 1.0);
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, 8.0)), 4.0, if frac > 0.9 { Color32::from_rgb(230, 70, 60) } else { BRAND });
            ui.add_space(10.0);
        }

        // quick folders
        ui.horizontal(|ui| {
            ui.add_space(margin);
            ui.label(egui::RichText::new("Quick access").size(18.0).strong());
        });
        ui.add_space(6.0);
        let root = app.root.clone();
        let quick: Vec<(&str, PathBuf)> = [
            ("Internal storage", ""),
            ("Downloads", "Download"),
            ("Camera", "DCIM/Camera"),
            ("Screenshots", "Pictures/Screenshots"),
            ("Pictures", "Pictures"),
            ("Movies", "Movies"),
            ("Music", "Music"),
            ("Documents", "Documents"),
            ("WhatsApp", "Android/media/com.whatsapp/WhatsApp/Media"),
        ]
        .into_iter()
        .map(|(n, rel)| (n, if rel.is_empty() { root.clone() } else { root.join(rel) }))
        .filter(|(_, p)| p.is_dir())
        .collect();
        let (gap, tile_h) = (8.0, 56.0);
        let tile_w = (width - gap) / 2.0;
        let rows = quick.len().div_ceil(2);
        let (_, area) = ui.allocate_space(vec2(width + 2.0 * margin, rows as f32 * (tile_h + gap)));
        for (i, (name, path)) in quick.iter().enumerate() {
            let rect = Rect::from_min_size(area.min + vec2(margin + (i % 2) as f32 * (tile_w + gap), (i / 2) as f32 * (tile_h + gap)), vec2(tile_w, tile_h));
            let resp = ui.interact(rect, ui.make_persistent_id(("quick", i)), Sense::click());
            card(ui, rect, resp.is_pointer_button_down_on());
            draw_icon(ui.painter(), Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(36.0, 36.0)), Kind::Folder, "");
            ui.painter().with_clip_rect(rect.shrink(4.0)).text(rect.min + vec2(56.0, tile_h / 2.0), Align2::LEFT_CENTER, name, FontId::proportional(15.0), Color32::WHITE);
            if resp.clicked() {
                go = Some(Screen::Folder(path.clone()));
            }
        }

        // categories
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(margin);
            ui.label(egui::RichText::new("Categories").size(18.0).strong());
            if app.scan_rx.is_some() {
                ui.label(egui::RichText::new("counting…").small().weak());
            }
        });
        ui.add_space(6.0);
        let cats: Vec<(Kind, Option<(usize, u64)>)> = Kind::CATEGORIES.iter().map(|&k| (k, app.scan.as_ref().map(|s| (s.count(k), s.bytes(k))))).collect();
        let rows = cats.len().div_ceil(2);
        let (_, area) = ui.allocate_space(vec2(width + 2.0 * margin, rows as f32 * (tile_h + gap)));
        for (i, (kind, info)) in cats.iter().enumerate() {
            let rect = Rect::from_min_size(area.min + vec2(margin + (i % 2) as f32 * (tile_w + gap), (i / 2) as f32 * (tile_h + gap)), vec2(tile_w, tile_h));
            let resp = ui.interact(rect, ui.make_persistent_id(("cat", i)), Sense::click());
            card(ui, rect, resp.is_pointer_button_down_on());
            let icon = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(36.0, 36.0));
            ui.painter().rect_filled(icon.shrink(2.0), 9.0, kind_color(*kind));
            let clip = ui.painter().with_clip_rect(rect.shrink(4.0));
            clip.text(rect.min + vec2(56.0, 17.0), Align2::LEFT_CENTER, kind.label(), FontId::proportional(15.0), Color32::WHITE);
            let sub = info.map_or("…".to_string(), |(n, b)| format!("{n} · {}", human_size(b)));
            clip.text(rect.min + vec2(56.0, 38.0), Align2::LEFT_CENTER, sub, FontId::proportional(12.0), Color32::GRAY);
            if resp.clicked() {
                go = Some(Screen::Category(*kind));
            }
        }

        // more
        ui.add_space(8.0);
        let trash_n = app.trash_count();
        let extras = [("Recent files", Screen::Recent, String::new()), ("Largest files", Screen::Large, String::new()), ("Trash", Screen::Trash, if trash_n == 0 { "empty".to_string() } else { format!("{trash_n} items") })];
        let (_, area) = ui.allocate_space(vec2(width + 2.0 * margin, extras.len() as f32 * (52.0 + gap) + 90.0));
        for (i, (name, screen, sub)) in extras.iter().enumerate() {
            let rect = Rect::from_min_size(area.min + vec2(margin, i as f32 * (52.0 + gap)), vec2(width, 52.0));
            let resp = ui.interact(rect, ui.make_persistent_id(("extra", i)), Sense::click());
            card(ui, rect, resp.is_pointer_button_down_on());
            ui.painter().text(rect.min + vec2(16.0, 26.0), Align2::LEFT_CENTER, name, FontId::proportional(16.0), Color32::WHITE);
            ui.painter().text(rect.max - vec2(16.0, 26.0), Align2::RIGHT_CENTER, sub, FontId::proportional(13.0), Color32::GRAY);
            if resp.clicked() {
                go = Some(screen.clone());
            }
        }
    });
    if search {
        app.search_text.clear();
        app.start_search_screen();
    }
    if let Some(s) = go {
        app.go(s);
    }
}

// ---------- lists ----------

fn screen_title(app: &Files) -> String {
    match app.current() {
        Screen::Folder(p) => {
            if *p == app.root { "Internal storage".to_string() } else { p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() }
        }
        Screen::Category(k) => k.label().to_string(),
        Screen::Recent => "Recent files".into(),
        Screen::Large => "Largest files".into(),
        Screen::Search(_) => "Search".into(),
        Screen::Trash => "Trash".into(),
        Screen::Settings => "Settings".into(),
        Screen::Home => "Files".into(),
    }
}

fn subtitle(app: &Files, e: &Entry) -> String {
    match app.current() {
        Screen::Folder(_) => {
            if e.is_dir { timefmt::short(e.mtime) } else { format!("{}  ·  {}", human_size(e.size), timefmt::short(e.mtime)) }
        }
        Screen::Trash => {
            let orig = trash::original_of(&e.path);
            let from = orig.as_deref().and_then(Path::parent).and_then(|p| p.strip_prefix(&app.root).ok()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let left = trash::trashed_at(&e.path)
                .map(|t| {
                    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    let days = (TRASH_DAYS * 86_400).saturating_sub(now.saturating_sub(t)).div_ceil(86_400);
                    format!("{days} days left")
                })
                .unwrap_or_default();
            format!("{}  ·  from /{}  ·  {}", if e.is_dir { "folder".to_string() } else { human_size(e.size) }, from, left)
        }
        _ => {
            let parent = e.path.parent().and_then(|p| p.strip_prefix(&app.root).ok()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            format!("{}  ·  /{}  ·  {}", if e.is_dir { "folder".to_string() } else { human_size(e.size) }, parent, timefmt::short(e.mtime))
        }
    }
}

fn list_screen(app: &mut Files, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let screen = app.current().clone();
    let in_trash = matches!(screen, Screen::Trash);

    // ---- header ----
    let selecting = app.select.as_ref().map(|s| s.len());
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(n) = selecting {
                if button(ui, "‹ Cancel") {
                    app.exit_select();
                }
                ui.label(egui::RichText::new(format!("{n} selected")).size(18.0).strong());
                let all = n >= app.entries.len() && n > 0;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if button(ui, if all { "Select none" } else { "Select all" }) {
                        app.select = Some(if all { HashSet::new() } else { app.entries.iter().map(|e| e.path.clone()).collect() });
                    }
                });
            } else {
                if button(ui, "‹") {
                    app.back();
                }
                let title = screen_title(app);
                ui.label(egui::RichText::new(title).size(20.0).strong());
                if !matches!(screen, Screen::Search(_)) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if button(ui, "More") {
                            app.header_menu = !app.header_menu;
                        }
                        if matches!(screen, Screen::Folder(_)) {
                            if button(ui, "Search") {
                                app.search_text.clear();
                                app.start_search_screen();
                            }
                        }
                    });
                }
            }
        });
        // breadcrumbs
        if let (Screen::Folder(p), None) = (&screen, selecting) {
            let mut jump: Option<PathBuf> = None;
            egui::ScrollArea::horizontal().id_salt("crumbs").auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let rel = p.strip_prefix(&app.root).unwrap_or(p);
                    let mut acc = app.root.clone();
                    if ui.link("Internal storage").clicked() {
                        jump = Some(acc.clone());
                    }
                    for part in rel.components() {
                        acc.push(part);
                        ui.label("›");
                        if ui.link(part.as_os_str().to_string_lossy()).clicked() {
                            jump = Some(acc.clone());
                        }
                    }
                });
            });
            if let Some(j) = jump {
                if j != *p {
                    app.go(Screen::Folder(j));
                }
            }
        }
        // the search box
        if matches!(screen, Screen::Search(_)) && selecting.is_none() {
            ui.horizontal(|ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut app.search_text).hint_text("Search by name").desired_width(ui.available_width() - 80.0));
                let go = button(ui, "Go") || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if go {
                    let q = app.search_text.clone();
                    app.start_search(q);
                }
            });
            if app.searching.load(Ordering::SeqCst) {
                ui.label(egui::RichText::new("Searching…").small().weak());
            }
        }
    });

    // ---- the list ----
    let mut entries = if matches!(screen, Screen::Search(_)) { app.search_hits.lock().unwrap().clone() } else { std::mem::take(&mut app.entries) };
    if let Some(err) = app.list_error.clone() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| ui.label(err));
    } else if entries.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            let scanning_cat = matches!(screen, Screen::Category(_) | Screen::Recent | Screen::Large) && app.scan.is_none();
            ui.label(match &screen {
                _ if scanning_cat => "Looking through your files…",
                Screen::Trash => "The trash is empty.",
                Screen::Search(_) => {
                    if app.searching.load(Ordering::SeqCst) { "Searching…" } else if app.search_text.is_empty() { "Type a name and tap Go." } else { "Nothing found." }
                }
                Screen::Folder(_) => "This folder is empty.",
                _ => "Nothing here.",
            });
        });
    } else {
        let n = entries.len();
        let width = ui.available_width();
        let down = ctx.input(|i| i.pointer.primary_down());
        // painting a selection with the finger
        if let Some((add, last)) = app.paint {
            if !down {
                app.paint = None;
            } else if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                let from = last.unwrap_or(p);
                let steps = ((p - from).length() / 12.0).ceil().max(1.0) as usize;
                for k in 0..=steps {
                    let q = from + (p - from) * (k as f32 / steps as f32);
                    let idx = ((q.y - app.list_top + app.list_offset) / ROW_H).floor();
                    if idx >= 0.0 && (idx as usize) < n {
                        if let Some(sel) = app.select.as_mut() {
                            let path = &entries[idx as usize].path;
                            if add { sel.insert(path.clone()); } else { sel.remove(path); }
                        }
                    }
                }
                app.paint = Some((add, Some(p)));
                ctx.request_repaint();
            }
        }
        let mut area = egui::ScrollArea::vertical().id_salt(("list", format!("{screen:?}"))).auto_shrink([false; 2]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        if let Some(y) = app.list_scroll_to.take() {
            area = area.vertical_scroll_offset(y);
        }
        if app.paint.is_some() {
            area = area.scroll_source(egui::containers::scroll_area::ScrollSource { drag: egui::containers::scroll_area::DragScroll::Never, ..Default::default() });
        }
        let mut action: Option<(usize, bool)> = None; // (row, long press)
        let out = area.show_rows(ui, ROW_H, n + 2, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let Some(e) = entries.get(i) else {
                    ui.allocate_space(vec2(width, ROW_H)); // room for the bottom bars
                    continue;
                };
                let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
                let selected = app.select.as_ref().is_some_and(|s| s.contains(&e.path));
                if selected {
                    ui.painter().rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(252, 87, 0, 38));
                } else if resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(rect, 0.0, Color32::from_gray(26));
                }
                let icon = Rect::from_min_size(rect.min + vec2(14.0, 9.0), vec2(44.0, 44.0));
                app.want_thumb(e);
                match app.thumbs.get(&e.path) {
                    Some(tex) => {
                        let [tw, th] = tex.size();
                        let (tw, th) = (tw as f32, th as f32);
                        let uv = if tw > th {
                            let m = (1.0 - th / tw) / 2.0;
                            Rect::from_min_max(pos2(m, 0.0), pos2(1.0 - m, 1.0))
                        } else {
                            let m = (1.0 - tw / th) / 2.0;
                            Rect::from_min_max(pos2(0.0, m), pos2(1.0, 1.0 - m))
                        };
                        egui::Image::new((tex.id(), icon.size())).uv(uv).corner_radius(8.0).paint_at(ui, icon);
                        if e.kind == Kind::Video {
                            let c = icon.right_bottom() - vec2(9.0, 9.0);
                            ui.painter().circle_filled(c, 8.0, Color32::from_black_alpha(170));
                            ui.painter().add(egui::Shape::convex_polygon(vec![c + vec2(-2.5, -4.5), c + vec2(-2.5, 4.5), c + vec2(4.5, 0.0)], Color32::WHITE, Stroke::NONE));
                        }
                    }
                    None => draw_icon(ui.painter(), icon, e.kind, &e.name),
                }
                let clip = ui.painter().with_clip_rect(Rect::from_min_max(rect.min + vec2(68.0, 0.0), rect.max - vec2(if app.select.is_some() { 52.0 } else { 14.0 }, 0.0)));
                clip.text(rect.min + vec2(68.0, 21.0), Align2::LEFT_CENTER, &e.name, FontId::proportional(15.5), Color32::WHITE);
                clip.text(rect.min + vec2(68.0, 43.0), Align2::LEFT_CENTER, subtitle(app, e), FontId::proportional(12.0), Color32::GRAY);
                if app.select.is_some() {
                    check_circle(ui.painter(), pos2(rect.max.x - 26.0, rect.center().y), selected);
                }
                if resp.long_touched() && app.paint.is_none() {
                    action = Some((i, true));
                }
                if resp.clicked() && !app.swallow_click {
                    action = Some((i, false));
                }
            }
        });
        if !down {
            app.swallow_click = false;
        }
        app.list_top = out.inner_rect.min.y;
        app.list_offset = out.state.clamped_offset().y;
        // painting near the top or bottom edge scrolls the list
        if let (Some(_), Some(p)) = (app.paint, ctx.input(|i| i.pointer.interact_pos())) {
            let vis = out.inner_rect;
            let dt = ctx.input(|i| i.unstable_dt).min(0.05);
            let bottom_edge = app.bar_top.min(vis.max.y);
            let speed = if p.y < vis.min.y + 80.0 {
                -(1.0 - ((p.y - vis.min.y) / 80.0).clamp(0.0, 1.0))
            } else if p.y > bottom_edge - 80.0 {
                ((p.y - (bottom_edge - 80.0)) / 80.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if speed != 0.0 {
                app.list_scroll_to = Some((app.list_offset + speed * 1200.0 * dt).max(0.0));
                ctx.request_repaint();
            }
        }
        if !matches!(screen, Screen::Search(_)) {
            app.entries = std::mem::take(&mut entries);
        }
        // what was tapped
        if let Some((i, long)) = action {
            let e = if matches!(screen, Screen::Search(_)) { app.search_hits.lock().unwrap().get(i).cloned() } else { app.entries.get(i).cloned() };
            if let Some(e) = e {
                if long {
                    let sel = app.select.get_or_insert_with(HashSet::new);
                    let add = !sel.contains(&e.path);
                    if add { sel.insert(e.path.clone()); } else { sel.remove(&e.path); }
                    app.paint = Some((add, ctx.input(|i| i.pointer.interact_pos())));
                    app.swallow_click = true;
                } else if let Some(sel) = app.select.as_mut() {
                    if !sel.remove(&e.path) {
                        sel.insert(e.path.clone());
                    }
                } else if in_trash {
                    app.say("Tap and hold to select, then Restore.", Vec::new());
                } else if e.is_dir {
                    app.go(Screen::Folder(e.path.clone()));
                } else {
                    app.open_entry(&e);
                }
            }
        }
    }
    let _ = in_trash;
}

// ---------- settings ----------

fn settings(app: &mut Files, ui: &mut egui::Ui) {
    let mut back = false;
    let mut changed = false;
    let mut empty_trash = false;
    let mut rescan = false;
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.horizontal(|ui| {
            if button(ui, "‹") {
                back = true;
            }
            ui.label(egui::RichText::new("Settings").size(20.0).strong());
        });
    });
    egui::ScrollArea::vertical().id_salt("settings").auto_shrink([false; 2]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Folders").size(18.0).strong());
            ui.add_space(6.0);
            if ui.add(egui::Checkbox::new(&mut app.show_hidden, egui::RichText::new("Show hidden files").size(16.0))).changed() {
                changed = true;
            }
            ui.label(egui::RichText::new("Files and folders whose names start with a dot.").small().weak());
            ui.add_space(14.0);
            ui.label(egui::RichText::new("Sort folders by").size(16.0));
            ui.horizontal(|ui| {
                for (k, label) in [(SortKey::Name, "Name"), (SortKey::Date, "Date"), (SortKey::Size, "Size"), (SortKey::Kind, "Type")] {
                    if ui.selectable_label(app.sort.key == k, egui::RichText::new(label).size(16.0)).clicked() {
                        app.sort.key = k;
                        changed = true;
                    }
                }
            });
            if ui.add(egui::Checkbox::new(&mut app.sort.descending, egui::RichText::new("Reverse order").size(16.0))).changed() {
                changed = true;
            }
            ui.add_space(16.0);
            ui.label(egui::RichText::new("Trash").size(18.0).strong());
            ui.add_space(6.0);
            ui.label(format!("Deleted items wait in the trash for {TRASH_DAYS} days, then are erased. {} in the trash now.", app.trash_count()));
            ui.add_space(6.0);
            if ui.add_sized([180.0, 38.0], egui::Button::new(egui::RichText::new("Empty trash").color(Color32::from_rgb(255, 120, 110)))).clicked() {
                empty_trash = true;
            }
            ui.add_space(16.0);
            ui.label(egui::RichText::new("Storage").size(18.0).strong());
            ui.add_space(6.0);
            if let Some(s) = &app.scan {
                ui.label(format!("{} files found on the phone.", s.files.len()));
            }
            if ui.add_sized([180.0, 38.0], egui::Button::new("Count again")).clicked() {
                rescan = true;
            }
            ui.add_space(60.0);
        });
    });
    if changed {
        app.save_settings();
        app.load_current();
    }
    if rescan {
        app.rescan();
        app.say("Counting…", Vec::new());
    }
    if empty_trash {
        let items: Vec<PathBuf> = std::fs::read_dir(&app.trash_dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| trash::original_of(p).is_some()).collect()).unwrap_or_default();
        if items.is_empty() {
            app.say("The trash is already empty.", Vec::new());
        } else {
            app.dialog = Some(Dialog::ConfirmForever { paths: items });
        }
    }
    if back {
        app.back();
    }
}

// ---------- overlays: bars, menus, dialogs, toast ----------

fn overlays(app: &mut Files, ctx: &Context, bottom: f32) {
    let screen_rect = ctx.viewport_rect();
    let on_list = !matches!(app.current(), Screen::Home | Screen::Settings);
    let in_trash = matches!(app.current(), Screen::Trash);
    let busy = app.job.is_some();

    // header menu
    if app.header_menu && on_list {
        let mut close = false;
        let mut new_folder = false;
        let mut select_all = false;
        let mut toggle_hidden = false;
        let mut sort = false;
        let mut settings = false;
        let mut empty = false;
        let is_folder = matches!(app.current(), Screen::Folder(_));
        egui::Area::new(egui::Id::new("header-menu")).fixed_pos(pos2(screen_rect.max.x - 10.0, app.insets.0 + 56.0)).pivot(Align2::RIGHT_TOP).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(210.0);
                let entry = |ui: &mut egui::Ui, text: &str| ui.add_sized([210.0, 38.0], egui::Button::new(text)).clicked();
                if is_folder && entry(ui, "New folder") {
                    new_folder = true;
                }
                if entry(ui, "Select all") {
                    select_all = true;
                }
                if !in_trash && entry(ui, "Sort…") {
                    sort = true;
                }
                if is_folder && entry(ui, if app.show_hidden { "Hide hidden files" } else { "Show hidden files" }) {
                    toggle_hidden = true;
                }
                if in_trash && entry(ui, "Empty trash") {
                    empty = true;
                }
                if entry(ui, "Settings") {
                    settings = true;
                }
                if entry(ui, "Close") {
                    close = true;
                }
            });
        });
        if new_folder || select_all || toggle_hidden || sort || settings || empty || close {
            app.header_menu = false;
        }
        if new_folder {
            app.dialog = Some(Dialog::NewFolder { text: "New folder".into() });
        }
        if select_all {
            app.select = Some(app.entries.iter().map(|e| e.path.clone()).collect());
        }
        if toggle_hidden {
            app.show_hidden = !app.show_hidden;
            app.save_settings();
            app.load_current();
        }
        if sort {
            app.dialog = Some(Dialog::Sort);
        }
        if settings {
            app.go(Screen::Settings);
        }
        if empty {
            let items: Vec<PathBuf> = app.entries.iter().map(|e| e.path.clone()).collect();
            if !items.is_empty() {
                app.dialog = Some(Dialog::ConfirmForever { paths: items });
            }
        }
    }

    // selection bar
    if let Some(sel) = app.select.as_ref() {
        if on_list && !busy {
            let mut paths: Vec<PathBuf> = sel.iter().cloned().collect();
            paths.sort();
            selection_bar(app, ctx, bottom, paths, in_trash);
        }
    } else if on_list && !busy {
        app.bar_top = f32::MAX;
        paste_bar(app, ctx, bottom);
    }

    // a running job
    if let Some(job) = &app.job {
        let (done, total, current, item, items) = {
            let p = job.progress.lock().unwrap();
            (p.done, p.total, p.current.clone(), p.item, p.items)
        };
        let title = job.kind.title();
        let mut cancel = false;
        egui::Area::new(egui::Id::new("job")).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(screen_rect.width() - 56.0);
                ui.label(egui::RichText::new(format!("{title}…")).strong().size(18.0));
                ui.add_space(6.0);
                let frac = if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
                ui.add(egui::ProgressBar::new(if total > 0 { frac } else { 0.0 }).desired_width(ui.available_width()).animate(total == 0));
                ui.add_space(4.0);
                ui.add(egui::Label::new(egui::RichText::new(current).small().weak()).truncate());
                ui.label(egui::RichText::new(format!("{item} of {items}{}", if total > 0 { format!("  ·  {} of {}", human_size(done), human_size(total)) } else { String::new() })).small());
                ui.add_space(8.0);
                if ui.add_sized([ui.available_width(), 38.0], egui::Button::new("Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            job.cancel.store(true, Ordering::Relaxed);
        }
        ctx.request_repaint_after(Duration::from_millis(120));
    }

    dialogs(app, ctx);

    // toast
    let now = ctx.input(|i| i.time);
    if app.toast.as_ref().is_some_and(|t| now > t.2) {
        app.toast = None;
    }
    let mut undo: Option<Vec<PathBuf>> = None;
    if let Some((msg, undo_paths, _)) = app.toast.clone() {
        let lift = if app.select.is_some() || app.clip.is_some() { 84.0 } else { 20.0 };
        egui::Area::new(egui::Id::new("toast")).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -(bottom + lift))).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(screen_rect.width() - 40.0);
                ui.horizontal(|ui| {
                    ui.label(msg);
                    if !undo_paths.is_empty() && ui.button("Undo").clicked() {
                        undo = Some(undo_paths.clone());
                    }
                });
            });
        });
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    if let Some(paths) = undo {
        app.toast = None;
        app.start_job(JobKind::Restore, paths, None);
    }
}

fn bar_frame(ctx: &Context, id: &str, bottom: f32, add: impl FnOnce(&mut egui::Ui)) -> Rect {
    egui::Area::new(egui::Id::new(id))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -(bottom + 10.0)))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new().fill(Color32::from_gray(38)).corner_radius(26.0).inner_margin(egui::Margin::symmetric(8, 8)).show(ui, |ui| {
                ui.horizontal(add);
            });
        })
        .response
        .rect
}

fn selection_bar(app: &mut Files, ctx: &Context, bottom: f32, paths: Vec<PathBuf>, in_trash: bool) {
    #[derive(PartialEq)]
    enum Act {
        Copy,
        Cut,
        Delete,
        Share,
        More,
        Restore,
        Forever,
    }
    let n = paths.len();
    let red = Color32::from_rgb(255, 120, 110);
    let mut act: Option<Act> = None;
    let btn = |text: egui::RichText| egui::Button::new(text).min_size(vec2(62.0, 38.0)).corner_radius(18.0);
    let rect = bar_frame(ctx, "select-bar", bottom, |ui| {
        if in_trash {
            if ui.add_enabled(n > 0, btn("Restore".into())).clicked() {
                act = Some(Act::Restore);
            }
            if ui.add_enabled(n > 0, btn(egui::RichText::new("Delete forever").color(red))).clicked() {
                act = Some(Act::Forever);
            }
        } else {
            if ui.add_enabled(n > 0, btn("Copy".into())).clicked() {
                act = Some(Act::Copy);
            }
            if ui.add_enabled(n > 0, btn("Move".into())).clicked() {
                act = Some(Act::Cut);
            }
            if ui.add_enabled(n > 0, btn(egui::RichText::new("Delete").color(red))).clicked() {
                act = Some(Act::Delete);
            }
            if ui.add_enabled(n > 0, btn("Share".into())).clicked() {
                act = Some(Act::Share);
            }
            if ui.add_enabled(n > 0, btn("More".into())).clicked() {
                act = Some(Act::More);
            }
        }
    });
    app.bar_top = rect.min.y;

    // the More menu above the bar
    if app.more_menu && n > 0 && !in_trash {
        let single = (n == 1).then(|| paths[0].clone());
        let in_folder = matches!(app.current(), Screen::Folder(_));
        let (mut rename, mut details, mut open_with, mut show, mut all) = (false, false, false, false, false);
        egui::Area::new(egui::Id::new("select-menu")).pivot(Align2::CENTER_BOTTOM).fixed_pos(pos2(rect.center().x, rect.min.y - 8.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(220.0);
                let entry = |ui: &mut egui::Ui, text: &str| ui.add_sized([220.0, 38.0], egui::Button::new(text)).clicked();
                if single.is_some() && in_folder && entry(ui, "Rename") {
                    rename = true;
                }
                if single.is_some() && entry(ui, "Details") {
                    details = true;
                }
                if single.as_ref().is_some_and(|p| p.is_file()) && entry(ui, "Open with…") {
                    open_with = true;
                }
                if single.is_some() && !in_folder && entry(ui, "Show in folder") {
                    show = true;
                }
                if entry(ui, "Select all") {
                    all = true;
                }
            });
        });
        if rename || details || open_with || show || all {
            app.more_menu = false;
        }
        if let Some(p) = single {
            if rename {
                let text = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                app.dialog = Some(Dialog::Rename { path: p.clone(), text });
            }
            if details {
                if let Some(entry) = Entry::from_path(&p) {
                    app.open_details(entry);
                }
            }
            if open_with {
                #[cfg(target_os = "android")]
                if let Some(a) = app.android.clone() {
                    if !perm::open_file(&a, &p.to_string_lossy(), true) {
                        app.say("No app on this phone can open that kind of file.", Vec::new());
                    }
                }
            }
            if show {
                if let Some(parent) = p.parent() {
                    app.go(Screen::Folder(parent.to_path_buf()));
                }
            }
        }
        if all {
            app.select = Some(app.entries.iter().map(|e| e.path.clone()).collect());
        }
    }

    match act {
        Some(Act::Copy) => {
            app.clip = Some((paths, false));
            app.exit_select();
            app.say("Copied. Open a folder and tap Paste.", Vec::new());
        }
        Some(Act::Cut) => {
            app.clip = Some((paths, true));
            app.exit_select();
            app.say("Ready to move. Open a folder and tap Paste.", Vec::new());
        }
        Some(Act::Delete) => app.start_job(JobKind::Trash, paths, None),
        Some(Act::Share) => app.share(&paths),
        Some(Act::More) => app.more_menu = !app.more_menu,
        Some(Act::Restore) => app.start_job(JobKind::Restore, paths, None),
        Some(Act::Forever) => app.dialog = Some(Dialog::ConfirmForever { paths }),
        None => {}
    }
}

fn paste_bar(app: &mut Files, ctx: &Context, bottom: f32) {
    let Some((paths, cut)) = app.clip.clone() else { return };
    let Some(dir) = app.cwd().map(Path::to_path_buf) else {
        // the clipboard stays; it just cannot be pasted on this screen
        return;
    };
    let mut paste = false;
    let mut cancel = false;
    let n = paths.len();
    bar_frame(ctx, "paste-bar", bottom, |ui| {
        let label = format!("{} {n} here", if cut { "Move" } else { "Paste" });
        if ui.add(egui::Button::new(egui::RichText::new(label).strong()).min_size(vec2(150.0, 38.0)).corner_radius(18.0).fill(BRAND)).clicked() {
            paste = true;
        }
        if ui.add(egui::Button::new("Cancel").min_size(vec2(80.0, 38.0)).corner_radius(18.0)).clicked() {
            cancel = true;
        }
    });
    if paste {
        app.start_job(if cut { JobKind::Move } else { JobKind::Copy }, paths, Some(dir));
    }
    if cancel {
        app.clip = None;
    }
}

// ---------- dialogs ----------

impl Files {
    pub(crate) fn start_search_screen(&mut self) {
        self.exit_select();
        self.search_hits.lock().unwrap().clear();
        self.entries.clear();
        self.stack.push(Screen::Search(String::new()));
    }

    pub(crate) fn open_details(&mut self, entry: Entry) {
        let stats = entry.is_dir.then(|| {
            let (tx, rx) = channel();
            let (p, ctx) = (entry.path.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(fsops::tree_size(&[p], &AtomicBool::new(false)));
                ctx.request_repaint();
            });
            rx
        });
        self.dialog = Some(Dialog::Details { entry, stats, result: None });
    }
}

fn dialogs(app: &mut Files, ctx: &Context) {
    let Some(dialog) = app.dialog.take() else { return };
    let screen = ctx.viewport_rect();
    let width = screen.width() - 48.0;
    let mut keep = true;
    let red = Color32::from_rgb(255, 120, 110);
    let dialog = match dialog {
        Dialog::Rename { path, mut text } => {
            let mut go = false;
            modal(ctx, width, |ui| {
                ui.label(egui::RichText::new("Rename").strong().size(18.0));
                ui.add_space(6.0);
                ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([ui.available_width() / 2.0 - 4.0, 36.0], egui::Button::new("Rename")).clicked() {
                        go = true;
                        keep = false;
                    }
                    if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("Cancel")).clicked() {
                        keep = false;
                    }
                });
            });
            if go {
                app.exit_select();
                app.rename(&path, &text);
            }
            Dialog::Rename { path, text }
        }
        Dialog::NewFolder { mut text } => {
            let mut go = false;
            modal(ctx, width, |ui| {
                ui.label(egui::RichText::new("New folder").strong().size(18.0));
                ui.add_space(6.0);
                ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([ui.available_width() / 2.0 - 4.0, 36.0], egui::Button::new("Create")).clicked() {
                        go = true;
                        keep = false;
                    }
                    if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("Cancel")).clicked() {
                        keep = false;
                    }
                });
            });
            if go {
                app.new_folder(&text);
            }
            Dialog::NewFolder { text }
        }
        Dialog::ConfirmForever { paths } => {
            let mut go = false;
            modal(ctx, width, |ui| {
                ui.label(egui::RichText::new("Delete for good?").strong().size(18.0));
                ui.label(format!("{} item{} will be erased and cannot be brought back.", paths.len(), if paths.len() == 1 { "" } else { "s" }));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([ui.available_width() / 2.0 - 4.0, 36.0], egui::Button::new(egui::RichText::new("Delete").color(red))).clicked() {
                        go = true;
                        keep = false;
                    }
                    if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("Cancel")).clicked() {
                        keep = false;
                    }
                });
            });
            if go {
                app.start_job(JobKind::DeleteForever, paths.clone(), None);
            }
            Dialog::ConfirmForever { paths }
        }
        Dialog::Details { entry, stats, mut result } => {
            if let Some(rx) = &stats {
                match rx.try_recv() {
                    Ok(r) => result = Some(r),
                    Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(200)),
                    Err(_) => {}
                }
            }
            modal(ctx, width, |ui| {
                ui.label(egui::RichText::new("Details").strong().size(18.0));
                ui.add_space(6.0);
                let mut line = |k: &str, v: String| {
                    ui.label(egui::RichText::new(k).small().weak());
                    ui.add(egui::Label::new(v).wrap());
                    ui.add_space(4.0);
                };
                line("Name", entry.name.clone());
                line("Where", entry.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
                line("Type", if entry.is_dir { "Folder".to_string() } else { entry.kind.label().trim_end_matches('s').to_string() });
                if entry.is_dir {
                    line("Size", result.map_or("counting…".to_string(), |(b, n)| format!("{}  ·  {n} files", human_size(b))));
                } else {
                    line("Size", format!("{}  ({} bytes)", human_size(entry.size), entry.size));
                }
                line("Changed", timefmt::full(entry.mtime));
                if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("Close")).clicked() {
                    keep = false;
                }
            });
            Dialog::Details { entry, stats, result }
        }
        Dialog::Sort => {
            modal(ctx, width, |ui| {
                ui.label(egui::RichText::new("Sort by").strong().size(18.0));
                ui.add_space(6.0);
                let mut picked = None;
                for (k, label) in [(SortKey::Name, "Name"), (SortKey::Date, "Date changed"), (SortKey::Size, "Size"), (SortKey::Kind, "Type")] {
                    let on = app.sort.key == k;
                    let text = format!("{}{label}{}", if on { "●  " } else { "○  " }, if on { if app.sort.descending { "   (high to low)" } else { "   (low to high)" } } else { "" });
                    if ui.add_sized([ui.available_width(), 38.0], egui::Button::new(text)).clicked() {
                        picked = Some(k);
                    }
                }
                if let Some(k) = picked {
                    if app.sort.key == k {
                        app.sort.descending = !app.sort.descending; // tapping the current one flips it
                    } else {
                        app.sort = Sort { key: k, descending: matches!(k, SortKey::Date | SortKey::Size) };
                    }
                    app.save_settings();
                    app.load_current();
                    keep = false;
                }
                if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("Close")).clicked() {
                    keep = false;
                }
            });
            Dialog::Sort
        }
        Dialog::Problems(list) => {
            modal(ctx, width, |ui| {
                ui.label(egui::RichText::new("Some things could not be done").strong().size(18.0));
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(screen.height() * 0.4).show(ui, |ui| {
                    for p in list.iter().take(30) {
                        ui.add(egui::Label::new(egui::RichText::new(p).small()).wrap());
                    }
                });
                ui.add_space(8.0);
                if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("OK")).clicked() {
                    keep = false;
                }
            });
            Dialog::Problems(list)
        }
    };
    if keep {
        // a dialog that opened another one in this pass keeps that one
        if app.dialog.is_none() {
            app.dialog = Some(dialog);
        }
    }
}

fn modal(ctx: &Context, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Area::new(egui::Id::new("dialog")).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(width);
            add(ui);
        });
    });
}
