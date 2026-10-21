//! "Free up space": the largest files, and likely duplicates, with a way to move chosen ones to the trash.

use super::*;

pub(crate) struct Space {
    /// 0 largest files, 1 duplicates.
    pub tab: u8,
    pub sel: HashSet<usize>,
    /// The duplicate search runs in the background; its answer arrives here.
    pub dup_rx: Option<Receiver<Vec<Vec<PathBuf>>>>,
    /// Groups of identical items, the oldest first in each (that one is kept by default).
    pub dups: Option<Vec<Vec<usize>>>,
}

enum Row {
    Group(String),
    Item(usize),
}

fn mb(bytes: u64) -> String {
    if bytes >= 1_073_741_824 { format!("{:.2} GB", bytes as f64 / 1_073_741_824.0) } else { format!("{:.1} MB", bytes as f64 / 1_048_576.0) }
}

impl Gallery {
    pub(crate) fn open_space(&mut self, ctx: &Context) {
        let files: Vec<(PathBuf, u64)> = self
            .items
            .iter()
            .filter(|i| !i.gone && !i.trashed && !i.hidden && i.album != EXT_KEY)
            .map(|i| (i.path.clone(), i.size))
            .collect();
        let (tx, rx) = channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(albumops::find_duplicates(&files));
            c.request_repaint();
        });
        self.space = Some(Space { tab: 0, sel: HashSet::new(), dup_rx: Some(rx), dups: None });
        self.settings_open = false;
    }

    pub(crate) fn close_space(&mut self) {
        self.space = None;
        self.settings_open = true;
    }

    fn usable(&self, id: usize) -> bool {
        let it = &self.items[id];
        !it.gone && !it.trashed && !it.hidden && it.album != EXT_KEY
    }

    pub(crate) fn space_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(mut sp) = self.space.take() else { return };
        // everything shown here needs its thumbnail
        self.shared.vis_lo.store(0, Ordering::Relaxed);
        self.shared.vis_hi.store(usize::MAX / 4, Ordering::Relaxed);

        // pick up the duplicate search
        if let Some(rx) = &sp.dup_rx {
            match rx.try_recv() {
                Ok(groups) => {
                    let by_path: HashMap<&Path, usize> = self.items.iter().enumerate().filter(|(_, i)| !i.gone).map(|(n, i)| (i.path.as_path(), n)).collect();
                    let mut out: Vec<Vec<usize>> = groups
                        .iter()
                        .map(|g| {
                            let mut ids: Vec<usize> = g.iter().filter_map(|p| by_path.get(p.as_path()).copied()).collect();
                            ids.sort_by_key(|&i| self.items[i].mtime);
                            ids
                        })
                        .filter(|g| g.len() > 1)
                        .collect();
                    out.sort_by_key(|g| std::cmp::Reverse(self.items[g[0]].size * (g.len() as u64 - 1)));
                    // by default keep the oldest of each group and mark the rest
                    for g in &out {
                        sp.sel.extend(g.iter().skip(1).copied());
                    }
                    sp.dups = Some(out);
                    sp.dup_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(250)),
                Err(_) => sp.dup_rx = None,
            }
        }

        egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("‹ Back").clicked() {
                    sp.tab = 255; // closes below
                }
                ui.heading("Free up space");
            });
            ui.horizontal(|ui| {
                for (k, label) in [(0u8, "Largest files"), (1, "Duplicates")] {
                    if ui.selectable_label(sp.tab == k, egui::RichText::new(label).size(16.0)).clicked() {
                        sp.tab = k;
                    }
                }
            });
        });
        if sp.tab == 255 {
            self.close_space();
            return;
        }

        // the rows of the current tab
        let mut rows: Vec<Row> = Vec::new();
        let mut note = String::new();
        if sp.tab == 0 {
            let mut ids: Vec<usize> = (0..self.items.len()).filter(|&i| self.usable(i)).collect();
            ids.sort_by_key(|&i| std::cmp::Reverse(self.items[i].size));
            ids.truncate(60);
            rows.extend(ids.into_iter().map(Row::Item));
            note = "The 60 biggest photos and videos.".into();
        } else {
            match &sp.dups {
                None => note = "Looking for duplicates…".into(),
                Some(groups) => {
                    for g in groups {
                        let ids: Vec<usize> = g.iter().copied().filter(|&i| self.usable(i)).collect();
                        if ids.len() > 1 {
                            rows.push(Row::Group(format!("{} copies · {} each", ids.len(), mb(self.items[ids[0]].size))));
                            rows.extend(ids.into_iter().map(Row::Item));
                        }
                    }
                    note = if rows.is_empty() { "No duplicates found.".into() } else { "Files with the same size and content. The oldest of each is left unmarked.".into() };
                }
            }
        }
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(egui::RichText::new(note).small().weak());
        });

        let row_h = 74.0;
        let blue = Color32::from_rgb(70, 140, 255);
        egui::ScrollArea::vertical().id_salt("space-list").auto_shrink([false; 2]).show_rows(ui, row_h, rows.len() + 2, |ui, range| {
            for r in range {
                let Some(row) = rows.get(r) else {
                    ui.add_space(row_h); // room at the end for the bottom bar
                    continue;
                };
                match row {
                    Row::Group(text) => {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                        ui.painter().text(rect.left_bottom() + vec2(14.0, -10.0), egui::Align2::LEFT_BOTTOM, text, egui::FontId::proportional(15.0), Color32::from_gray(200));
                    }
                    Row::Item(id) => {
                        let id = *id;
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                        let thumb = Rect::from_min_size(rect.min + vec2(12.0, 5.0), vec2(64.0, 64.0));
                        self.cover_ui(ui, thumb, Some(id), 8.0);
                        let it = &self.items[id];
                        let name = it.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let clip = ui.painter().with_clip_rect(Rect::from_min_max(rect.min + vec2(86.0, 0.0), rect.max - vec2(48.0, 0.0)));
                        clip.text(rect.min + vec2(86.0, 18.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(15.0), Color32::WHITE);
                        let kind = if it.video { "Video" } else { "Photo" };
                        clip.text(rect.min + vec2(86.0, 42.0), egui::Align2::LEFT_CENTER, format!("{}  ·  {kind}  ·  {}", mb(it.size), local_time(it.mtime)), egui::FontId::proportional(12.0), Color32::GRAY);
                        let c = pos2(rect.max.x - 26.0, rect.center().y);
                        if sp.sel.contains(&id) {
                            ui.painter().circle_filled(c, 12.0, blue);
                            let tick = egui::Stroke::new(2.2, Color32::WHITE);
                            ui.painter().line_segment([c + vec2(-5.5, 0.0), c + vec2(-1.5, 4.5)], tick);
                            ui.painter().line_segment([c + vec2(-1.5, 4.5), c + vec2(6.0, -4.5)], tick);
                        } else {
                            ui.painter().circle_stroke(c, 12.0, egui::Stroke::new(1.6, Color32::from_gray(150)));
                        }
                        if resp.clicked() && !sp.sel.remove(&id) {
                            sp.sel.insert(id);
                        }
                    }
                }
            }
        });

        // bottom bar
        let chosen: Vec<usize> = sp.sel.iter().copied().filter(|&i| self.usable(i)).collect();
        let bytes: u64 = chosen.iter().map(|&i| self.items[i].size).sum();
        let bottom = self.insets.1;
        let mut go = false;
        egui::Area::new(egui::Id::new("space-bar")).anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -(bottom + 10.0))).order(egui::Order::Foreground).show(&ctx, |ui| {
            egui::Frame::new().fill(Color32::from_gray(38)).corner_radius(26.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                let label = if chosen.is_empty() { "Nothing selected".to_string() } else { format!("Move {} to trash  ({})", chosen.len(), mb(bytes)) };
                let b = egui::Button::new(egui::RichText::new(label).color(Color32::from_rgb(255, 120, 110))).min_size(vec2(250.0, 38.0)).corner_radius(18.0);
                if ui.add_enabled(!chosen.is_empty(), b).clicked() {
                    go = true;
                }
            });
        });
        if go {
            self.trash_items(&ctx, &chosen);
            sp.sel.clear();
        }
        self.space = Some(sp);
    }
}
