//! Gallery page of the app window: a grid of shots, a panel for the selected one and a view
//! over the whole page.

use crate::ui;
use eframe::egui::{
    self, ColorImage, Frame, Key, Margin, Rect, RichText, Sense, TextEdit, TextureHandle,
    TextureOptions, UiBuilder, pos2, vec2,
};
use egui_phosphor::regular as icon;
use snapling_core::history::{self, Entry};
use snapling_core::i18n::{t, tf};
use snapling_core::storage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct Hooks {
    pub file: PathBuf,
    /// Current local time as `history::time_string`.
    pub now: fn() -> String,
    pub copy_image: fn(&Path) -> Result<(), String>,
    pub copy_text: fn(&str) -> Result<(), String>,
    /// A recording goes to the clipboard as a file.
    pub copy_file: fn(&Path) -> Result<(), String>,
    /// Opens the saved shot in the editor process.
    pub edit: fn(&Path, &str) -> Result<(), String>,
    /// Shows the file in its folder.
    pub reveal: fn(&Path),
    /// Pins the shot over all windows, in a process of its own.
    pub pin: fn(&Path) -> Result<(), String>,
    /// Text of a saved shot, by the engine from the settings. Blocks.
    pub recognize: fn(&Path) -> Result<String, String>,
    /// Shots folder from the config, opened by the header button.
    pub open_folder: fn(),
}

const CARD_H: f32 = 178.0;
const THUMB_H: f32 = 132.0;
const CARD_W: f32 = 210.0;
const GAP: f32 = 12.0;
const PANEL_W: f32 = 312.0;
const THUMB_SIDE: u32 = 480;
/// Previews kept in the GPU, about 200 MB; the ones on screen come back on the next frame.
const MAX_PICTURES: usize = 400;

type Picture = (String, Option<ColorImage>);

/// Decodes pictures one by one, in the order they were asked for. `side` limits the longer
/// side of a preview; `None` keeps the shot as it is, for the big view.
fn pictures(rx: Receiver<String>, tx: Sender<Picture>, side: Option<u32>, ctx: egui::Context) {
    for path in rx {
        let path_ref = Path::new(&path);
        let px = match side {
            Some(side) => storage::thumbnail(path_ref, side),
            None => storage::load_bgra(path_ref).map(|(mut px, w, h)| {
                for p in px.as_chunks_mut::<4>().0 {
                    p.swap(0, 2);
                }
                (px, w, h)
            }),
        };
        let img = px
            .ok()
            .map(|(px, w, h)| ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px));
        if tx.send((path, img)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Fits `size` into `area`, centered, keeping the shape.
fn fit(area: Rect, size: [usize; 2]) -> Rect {
    let [w, h] = size.map(|v| v as f32);
    let k = (area.width() / w).min(area.height() / h);
    Rect::from_center_size(area.center(), vec2(w, h) * k)
}

pub struct History {
    hooks: Hooks,
    /// Newest first.
    entries: Vec<Entry>,
    query: String,
    period: usize,
    now: String,
    status: String,
    tex: HashMap<String, Option<TextureHandle>>,
    ask: Sender<String>,
    got: Receiver<Picture>,
    /// Index into `entries`.
    selected: Option<usize>,
    /// The big view is open.
    big: bool,
    full: Option<(String, Option<TextureHandle>)>,
    ask_full: Sender<String>,
    got_full: Receiver<Picture>,
    /// Scroll offset and height of the grid, for keeping the selection in view.
    scroll: (f32, f32),
    scroll_to: Option<f32>,
    columns: usize,
    /// Recognized text of a shot, while recognition runs.
    reading: Option<Receiver<(Entry, Result<String, String>)>>,
}

enum Act {
    Select(usize),
    Big(usize),
    Copy(usize),
    Edit(usize),
    Reveal(usize),
    Pin(usize),
    Read(usize),
    Remove(usize),
}

impl History {
    pub fn new(hooks: Hooks, ctx: egui::Context) -> Self {
        let (ask, wrx) = channel();
        let (wtx, got) = channel();
        let worker = ctx.clone();
        std::thread::spawn(move || pictures(wrx, wtx, Some(THUMB_SIDE), worker));
        let (ask_full, wrx) = channel();
        let (wtx, got_full) = channel();
        std::thread::spawn(move || pictures(wrx, wtx, None, ctx));
        let mut entries = history::load(&hooks.file);
        entries.reverse();
        Self {
            now: (hooks.now)(),
            hooks,
            entries,
            query: String::new(),
            period: 0,
            status: String::new(),
            tex: HashMap::new(),
            ask,
            got,
            selected: None,
            // `SNAP_BIG=1` opens the big view in debug builds, for `ui::preview`.
            big: cfg!(debug_assertions) && std::env::var_os("SNAP_BIG").is_some(),
            full: None,
            ask_full,
            got_full,
            scroll: (0.0, 0.0),
            scroll_to: None,
            columns: 1,
            reading: None,
        }
    }

    /// Entries passing the search and the period, with the text line that matched.
    fn visible(&self) -> Vec<(usize, String)> {
        let max_days = [None, Some(0), Some(6)][self.period];
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                max_days
                    .is_none_or(|m| history::days_ago(&e.time, &self.now).is_some_and(|d| d <= m))
            })
            .filter_map(|(i, e)| history::matches(e, &self.query).map(|m| (i, m)))
            .collect()
    }

    fn thumb(&mut self, path: &str) -> Option<TextureHandle> {
        if path.is_empty() {
            return None;
        }
        if !self.tex.contains_key(path) {
            self.tex.insert(path.to_string(), None);
            let _ = self.ask.send(path.to_string());
        }
        self.tex.get(path)?.clone()
    }

    /// The big picture of the selected shot; only one is kept in memory.
    fn full_picture(&mut self, path: &str) -> Option<TextureHandle> {
        if self.full.as_ref().is_none_or(|f| f.0 != path) {
            self.full = Some((path.to_string(), None));
            let _ = self.ask_full.send(path.to_string());
        }
        self.full.as_ref()?.1.clone()
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((path, img)) = self.got.try_recv() {
            if self.tex.len() >= MAX_PICTURES {
                self.tex.clear();
            }
            let tex = img.map(|i| ctx.load_texture(&path, i, TextureOptions::LINEAR));
            self.tex.insert(path, tex);
        }
        while let Ok((path, img)) = self.got_full.try_recv() {
            if let Some(f) = self.full.as_mut().filter(|f| f.0 == path) {
                f.1 = img.map(|i| ctx.load_texture("full", i, TextureOptions::LINEAR));
            }
        }
        if let Some((entry, res)) = self.reading.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.reading = None;
            self.status = match res {
                Ok(text) if text.trim().is_empty() => t("Текста на снимке нет").into(),
                Ok(text) => self.keep_text(&entry, text),
                Err(e) => tf("Не получилось: {e}", &[("e", &e)]),
            };
        }
    }

    fn keep_text(&mut self, entry: &Entry, text: String) -> String {
        let saved = history::set_text(&self.hooks.file, entry, &text);
        if let Some(e) = self.entries.iter_mut().find(|e| *e == entry) {
            e.text.clone_from(&text);
        }
        match saved.and_then(|()| (self.hooks.copy_text)(&text)) {
            Ok(()) => t("Текст скопирован в буфер").into(),
            Err(e) => tf("Не получилось: {e}", &[("e", &e)]),
        }
    }

    fn apply(&mut self, act: Act) {
        let res = match act {
            Act::Select(i) => {
                self.selected = Some(i);
                return;
            }
            Act::Big(i) => {
                self.selected = Some(i);
                let e = &self.entries[i];
                self.big = !e.path.is_empty() && !Self::is_recording(e);
                return;
            }
            Act::Copy(i) => {
                let e = &self.entries[i];
                let r = if e.path.is_empty() {
                    (self.hooks.copy_text)(&e.text)
                } else if Self::is_recording(e) {
                    (self.hooks.copy_file)(Path::new(&e.path))
                } else {
                    (self.hooks.copy_image)(Path::new(&e.path))
                };
                r.map(|()| t("Скопировано в буфер").to_string())
            }
            Act::Edit(i) => {
                let e = &self.entries[i];
                (self.hooks.edit)(Path::new(&e.path), &e.app).map(|()| String::new())
            }
            Act::Reveal(i) => {
                (self.hooks.reveal)(Path::new(&self.entries[i].path));
                Ok(String::new())
            }
            Act::Pin(i) => {
                (self.hooks.pin)(Path::new(&self.entries[i].path)).map(|()| String::new())
            }
            Act::Read(i) => {
                let entry = self.entries[i].clone();
                let run = self.hooks.recognize;
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let res = run(Path::new(&entry.path));
                    let _ = tx.send((entry, res));
                });
                self.reading = Some(rx);
                Ok(t("Распознаю…").to_string())
            }
            Act::Remove(i) => history::remove(&self.hooks.file, &self.entries[i]).map(|()| {
                self.entries.remove(i);
                self.big = false;
                t("Убрано из галереи, файл на месте").to_string()
            }),
        };
        self.status = res.unwrap_or_else(|e| tf("Не получилось: {e}", &[("e", &e)]));
    }

    fn is_recording(e: &Entry) -> bool {
        e.kind == "record"
    }

    fn title(e: &Entry) -> String {
        if !e.app.is_empty() {
            return e.app.clone();
        }
        match e.kind.as_str() {
            "screen" => t("Весь экран"),
            "window" => t("Окно"),
            "ocr" => t("Текст"),
            "record" => t("Запись"),
            "long" => t("Длинный скриншот"),
            _ => t("Область"),
        }
        .to_string()
    }

    fn meta(&self, e: &Entry) -> String {
        let mut parts = vec![history::when(&e.time, &self.now)];
        if !e.path.is_empty() {
            parts.push(format!("{}×{}", e.width, e.height));
            let ext = Path::new(&e.path)
                .extension()
                .map_or(String::new(), |x| x.to_string_lossy().to_uppercase());
            parts.push(ext);
        }
        parts.join(" · ")
    }

    /// Arrows and Enter over the visible shots.
    fn keys(&mut self, ctx: &egui::Context, visible: &[(usize, String)]) -> Option<Act> {
        if ctx.egui_wants_keyboard_input() || visible.is_empty() {
            return None;
        }
        let at = visible
            .iter()
            .position(|v| Some(v.0) == self.selected)
            .unwrap_or(0);
        let step = |k: Key| ctx.input(|i| i.key_pressed(k));
        let down = if self.big { 1 } else { self.columns };
        let to = if step(Key::ArrowRight) {
            at + 1
        } else if step(Key::ArrowLeft) {
            at.saturating_sub(1)
        } else if step(Key::ArrowDown) {
            at + down
        } else if step(Key::ArrowUp) {
            at.saturating_sub(down)
        } else if step(Key::Enter) {
            return Some(Act::Big(visible[at].0));
        } else {
            return None;
        };
        let to = to.min(visible.len() - 1);
        let row = (to / self.columns.max(1)) as f32;
        let (top, bottom) = (row * (CARD_H + GAP), (row + 1.0) * (CARD_H + GAP));
        let (offset, height) = self.scroll;
        if top < offset {
            self.scroll_to = Some(top);
        } else if bottom > offset + height {
            self.scroll_to = Some(bottom - height);
        }
        Some(Act::Select(visible[to].0))
    }

    fn card(&mut self, ui: &mut egui::Ui, rect: Rect, index: usize) -> Option<Act> {
        let ctx = ui.ctx().clone();
        let p = ui::pal(&ctx);
        let e = self.entries[index].clone();
        let r = ui.interact(rect, ui.id().with(("card", index)), Sense::click());
        ui.painter().rect_filled(rect, 12.0, p.card);
        let shot =
            Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, THUMB_H));
        ui.painter().rect_filled(shot, 9.0, p.well);
        match self.thumb(&e.path) {
            Some(t) => {
                let [w, h] = t.size().map(|v| v as f32);
                // Cover, not stretch: crop the longer side.
                let k = (shot.width() / w).max(shot.height() / h);
                let (u, v) = (shot.width() / w / k, shot.height() / h / k);
                let uv = Rect::from_center_size(pos2(0.5, 0.5), vec2(u, v));
                egui::Image::new((t.id(), shot.size()))
                    .uv(uv)
                    .corner_radius(9)
                    .paint_at(ui, shot);
            }
            None if e.path.is_empty() => {
                let mut area = ui.new_child(UiBuilder::new().max_rect(shot.shrink(10.0)));
                area.set_clip_rect(shot.shrink(10.0).intersect(ui.clip_rect()));
                area.label(RichText::new(&e.text).size(12.5).color(p.muted));
            }
            None => {}
        }
        let text = Rect::from_min_max(
            pos2(rect.left() + 10.0, shot.bottom() + 4.0),
            pos2(rect.right() - 10.0, rect.bottom() - 4.0),
        );
        let mut area = ui.new_child(
            UiBuilder::new()
                .max_rect(text)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        area.set_clip_rect(text.intersect(ui.clip_rect()));
        let when = history::when(&e.time, &self.now);
        let width = area
            .painter()
            .layout_no_wrap(when.clone(), egui::FontId::proportional(12.0), p.muted)
            .size()
            .x;
        area.scope(|ui| {
            ui.set_max_width(text.width() - width - 8.0);
            ui::short(ui, RichText::new(Self::title(&e)).size(13.0).color(p.soft));
        });
        area.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(when).size(12.0).color(p.muted));
        });
        if self.selected == Some(index) {
            let stroke = egui::Stroke::new(2.0, ui::ACCENT);
            let kind = egui::StrokeKind::Inside;
            ui.painter().rect_stroke(rect, 12.0, stroke, kind);
        } else {
            ui::outline(ui, rect, 12.0);
        }
        if r.double_clicked() {
            Some(Act::Big(index))
        } else {
            r.clicked().then_some(Act::Select(index))
        }
    }

    fn actions(&self, ui: &mut egui::Ui, index: usize, act: &mut Option<Act>) {
        let e = &self.entries[index];
        let w = ui.available_width();
        if ui::primary(ui, t("Копировать"), Some(w), 40.0).clicked() {
            *act = Some(Act::Copy(index));
        }
        let half = (w - 8.0) / 2.0;
        if Self::is_recording(e) {
            if ui::tone(ui, t("В папке"), Some(w), 38.0).clicked() {
                *act = Some(Act::Reveal(index));
            }
        } else if !e.path.is_empty() {
            ui.horizontal(|ui| {
                if ui::tone(ui, t("Редактор"), Some(half), 38.0).clicked() {
                    *act = Some(Act::Edit(index));
                }
                if ui::tone(ui, t("Закрепить"), Some(half), 38.0).clicked() {
                    *act = Some(Act::Pin(index));
                }
            });
            ui.horizontal(|ui| {
                if ui::tone(ui, t("В папке"), Some(half), 38.0).clicked() {
                    *act = Some(Act::Reveal(index));
                }
                let label = if self.reading.is_some() {
                    t("Распознаю…")
                } else {
                    t("Текст")
                };
                let free = self.reading.is_none();
                let r = ui.add_enabled_ui(free, |ui| ui::tone(ui, label, Some(half), 38.0));
                if r.inner.clicked() {
                    *act = Some(Act::Read(index));
                }
            });
        }
        if ui::ghost_text_w(ui, t("Убрать"), w, 38.0)
            .on_hover_text(t("Файл на диске остаётся"))
            .clicked()
        {
            *act = Some(Act::Remove(index));
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, index: usize, hit: &str, act: &mut Option<Act>) {
        let p = ui::pal(ui.ctx());
        let e = self.entries[index].clone();
        ui.spacing_mut().item_spacing.y = 10.0;
        let (area, r) = ui.allocate_exact_size(vec2(ui.available_width(), 176.0), Sense::click());
        ui.painter().rect_filled(area, 10.0, p.well);
        if let Some(t) = self.thumb(&e.path) {
            egui::Image::new((t.id(), area.size()))
                .corner_radius(8)
                .paint_at(ui, fit(area, t.size()));
        }
        if r.double_clicked() {
            *act = Some(Act::Big(index));
        }
        ui.spacing_mut().item_spacing.y = 2.0;
        ui::short(ui, RichText::new(Self::title(&e)).font(ui::semi(15.0)));
        ui::short(ui, RichText::new(self.meta(&e)).size(12.5).color(p.muted));
        ui.spacing_mut().item_spacing.y = 10.0;
        if !hit.is_empty() {
            let line = format!("«{hit}»");
            ui::short(ui, RichText::new(line).size(12.5).color(p.warn));
        }
        let rows = if e.path.is_empty() {
            1.0
        } else if Self::is_recording(&e) {
            2.0
        } else {
            3.0
        };
        let foot = 40.0 + 38.0 * rows + 10.0 * (rows + 1.0);
        if !e.path.is_empty()
            && !Self::is_recording(&e)
            && ui::ghost_text_w(ui, t("Открыть большим"), ui.available_width(), 34.0).clicked()
        {
            *act = Some(Act::Big(index));
        }
        let left = ui.available_height() - foot;
        if !e.text.is_empty() && left > 40.0 {
            Frame::new()
                .fill(p.well)
                .corner_radius(10)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .max_height(left - 20.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(&e.text).size(12.5).color(p.soft));
                        });
                });
        }
        ui.add_space((ui.available_height() - foot).max(0.0));
        self.actions(ui, index, act);
    }

    fn big_view(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        near: [Option<usize>; 2],
        act: &mut Option<Act>,
    ) {
        let ctx = ui.ctx().clone();
        let p = ui::pal(&ctx);
        let e = self.entries[index].clone();
        let tex = self.full_picture(&e.path);
        egui::Panel::top("big-head")
            .show_separator_line(false)
            .frame(Frame::new().fill(p.well).inner_margin(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui::ghost_icon(ui, icon::ARROW_LEFT, 38.0, t("Назад")).clicked() {
                        self.big = false;
                    }
                    for (other, glyph, tip) in [
                        (near[0], icon::CARET_LEFT, t("Предыдущий снимок")),
                        (near[1], icon::CARET_RIGHT, t("Следующий снимок")),
                    ] {
                        let free = other.is_some();
                        let r = ui.add_enabled_ui(free, |ui| ui::ghost_icon(ui, glyph, 38.0, tip));
                        if let (true, Some(i)) = (r.inner.clicked(), other) {
                            *act = Some(Act::Big(i));
                        }
                    }
                    ui.label(RichText::new(Self::title(&e)).font(ui::semi(14.0)));
                    ui.label(RichText::new(self.meta(&e)).size(12.5).color(p.muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::tone(ui, t("Редактор"), None, 38.0).clicked() {
                            *act = Some(Act::Edit(index));
                        }
                        if ui::primary(ui, t("Копировать"), None, 38.0).clicked() {
                            *act = Some(Act::Copy(index));
                        }
                    });
                });
            });
        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.well).inner_margin(12))
            .show(ui, |ui| {
                let area = ui.available_rect_before_wrap();
                let r = ui.allocate_rect(area, Sense::click());
                match tex {
                    Some(t) => {
                        egui::Image::new((t.id(), area.size()))
                            .corner_radius(6)
                            .paint_at(ui, fit(area, t.size()));
                    }
                    None => {
                        ui.painter().text(
                            area.center(),
                            egui::Align2::CENTER_CENTER,
                            t("Загружаю…"),
                            egui::FontId::proportional(14.0),
                            p.muted,
                        );
                    }
                }
                if r.double_clicked() {
                    self.big = false;
                }
            });
    }
}

impl History {
    /// The big view is open: Esc belongs to it.
    pub fn is_big(&self) -> bool {
        self.big
    }

    /// Draws the page into the free space of `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        if self.big && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.big = false;
        }
        let p = ui::pal(&ctx);
        let visible = self.visible();
        if !visible.iter().any(|v| Some(v.0) == self.selected) {
            self.big &= self.selected.is_none();
            self.selected = visible.first().map(|v| v.0);
        }
        let mut act = self.keys(&ctx, &visible);
        if self.big
            && let Some(i) = self.selected
        {
            let shots: Vec<usize> = visible
                .iter()
                .map(|v| v.0)
                .filter(|i| {
                    let e = &self.entries[*i];
                    !e.path.is_empty() && !Self::is_recording(e)
                })
                .collect();
            let at = shots.iter().position(|s| *s == i).unwrap_or(0);
            let near = [
                at.checked_sub(1).and_then(|a| shots.get(a)).copied(),
                shots.get(at + 1).copied(),
            ];
            self.big_view(ui, i, near, &mut act);
            if let Some(a) = act {
                self.apply(a);
            }
            return;
        }
        egui::Panel::top("head")
            .show_separator_line(false)
            .frame(Frame::new().fill(p.bg).inner_margin(Margin {
                left: 20,
                right: 12,
                top: 14,
                bottom: 12,
            }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui::heading(ui, t("Галерея"), 20.0);
                    ui.add_space(8.0);
                    let items = [t("Всё"), t("Сегодня"), t("Неделя")];
                    if let Some(i) = ui::segmented(ui, &items, self.period, 32.0) {
                        self.period = i;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::tone(ui, t("Открыть папку"), None, 36.0).clicked() {
                            (self.hooks.open_folder)();
                        }
                        Frame::new()
                            .fill(p.card)
                            .stroke(egui::Stroke::new(1.0, p.line))
                            .corner_radius(10)
                            .inner_margin(Margin::symmetric(12, 0))
                            .show(ui, |ui| {
                                ui.set_height(36.0);
                                ui.set_width(ui.available_width().min(280.0));
                                ui.horizontal_centered(|ui| {
                                    ui.label(
                                        RichText::new(icon::MAGNIFYING_GLASS)
                                            .font(ui::icon_font(16.0))
                                            .color(p.muted),
                                    );
                                    ui.add(
                                        TextEdit::singleline(&mut self.query)
                                            .frame(Frame::NONE)
                                            .hint_text(t("Поиск по тексту на снимках"))
                                            .desired_width(f32::INFINITY),
                                    );
                                });
                            });
                    });
                });
            });
        egui::Panel::bottom("foot")
            .show_separator_line(false)
            .frame(Frame::new().fill(p.bg).inner_margin(Margin {
                left: 20,
                right: 20,
                top: 6,
                bottom: 10,
            }))
            .show(ui, |ui| {
                let text = if self.status.is_empty() {
                    t(
                        "Стрелки выбирают снимок, Enter открывает большим. Галерея хранится только \
                     на этом компьютере.",
                    )
                } else {
                    &self.status
                };
                ui.label(RichText::new(text).size(12.5).color(p.muted));
            });
        if let Some(i) = self.selected {
            let hit = visible
                .iter()
                .find(|v| v.0 == i)
                .map_or(String::new(), |v| v.1.clone());
            egui::Panel::right("info")
                .exact_size(PANEL_W + 12.0)
                .show_separator_line(false)
                .frame(Frame::new().fill(p.bg).inner_margin(Margin {
                    left: 0,
                    right: 12,
                    top: 0,
                    bottom: 0,
                }))
                .show(ui, |ui| {
                    Frame::new()
                        .fill(p.main)
                        .stroke(egui::Stroke::new(1.0, p.line))
                        .corner_radius(14)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.set_min_height(ui.available_height());
                            ui.set_width(ui.available_width());
                            self.panel(ui, i, &hit, &mut act);
                        });
                });
        }
        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin {
                left: 20,
                right: 12,
                top: 0,
                bottom: 0,
            }))
            .show(ui, |ui| {
                if visible.is_empty() {
                    ui.centered_and_justified(|ui| {
                        let text = if self.entries.is_empty() {
                            t("Здесь появятся сохранённые снимки")
                        } else {
                            t("Ничего не найдено")
                        };
                        ui.label(RichText::new(text).color(p.muted));
                    });
                    return;
                }
                let columns = (((ui.available_width() + GAP) / (CARD_W + GAP)) as usize).max(1);
                self.columns = columns;
                let width = (ui.available_width() - GAP * (columns - 1) as f32) / columns as f32;
                let rows = visible.len().div_ceil(columns);
                let mut area = egui::ScrollArea::vertical().auto_shrink(false);
                if let Some(y) = self.scroll_to.take() {
                    area = area.vertical_scroll_offset(y);
                }
                ui.spacing_mut().item_spacing.y = 0.0;
                let out = area.show_rows(ui, CARD_H + GAP, rows, |ui, range| {
                    for r in range {
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), CARD_H + GAP),
                            Sense::hover(),
                        );
                        for c in 0..columns {
                            let Some((index, _)) = visible.get(r * columns + c) else {
                                break;
                            };
                            let min = rect.min + vec2((width + GAP) * c as f32, 0.0);
                            let cell = Rect::from_min_size(min, vec2(width, CARD_H));
                            if let Some(a) = self.card(ui, cell, *index) {
                                act = Some(a);
                            }
                        }
                    }
                });
                self.scroll = (out.state.offset.y, out.inner_rect.height());
            });
        if let Some(a) = act {
            self.apply(a);
        }
    }
}
