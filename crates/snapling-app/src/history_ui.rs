//! History window, its own process (`snapling --history`): a grid of shots with text search.

use crate::ui;
use eframe::egui::{
    self, ColorImage, Frame, Margin, Rect, RichText, Sense, TextEdit, TextureHandle,
    TextureOptions, UiBuilder, pos2, vec2,
};
use egui_phosphor::regular as icon;
use snapling_core::history::{self, Entry};
use snapling_core::storage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct Hooks {
    pub warn: fn(&str),
    pub file: PathBuf,
    /// Current local time as `history::time_string`.
    pub now: fn() -> String,
    pub copy_image: fn(&Path) -> Result<(), String>,
    pub copy_text: fn(&str) -> Result<(), String>,
    /// Opens the saved shot in the editor process.
    pub edit: fn(&Path, &str) -> Result<(), String>,
    pub open: fn(&Path),
    /// Shots folder from the config, opened by the header button.
    pub open_folder: fn(),
}

const CARD_H: f32 = 232.0;
const THUMB_H: f32 = 132.0;
const GAP: f32 = 16.0;

pub fn run(hooks: Hooks) {
    let options = eframe::NativeOptions {
        viewport: ui::viewport("Snapling — история", [1040.0, 700.0], [560.0, 420.0]),
        ..Default::default()
    };
    let warn = hooks.warn;
    let res = eframe::run_native(
        "Snapling history",
        options,
        Box::new(move |cc| {
            ui::init(cc);
            Ok(Box::new(History::new(hooks, cc.egui_ctx.clone())))
        }),
    );
    if let Err(e) = res {
        warn(&format!("Окно истории не запустилось.\n{e}"));
    }
}

type Thumb = (String, Option<ColorImage>);

/// Decodes previews one by one, in the order cards came on screen.
fn thumbs(rx: Receiver<String>, tx: Sender<Thumb>, ctx: egui::Context) {
    for path in rx {
        let img = storage::thumbnail(Path::new(&path), 480)
            .ok()
            .map(|(px, w, h)| ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px));
        if tx.send((path, img)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

struct History {
    hooks: Hooks,
    /// Newest first.
    entries: Vec<Entry>,
    query: String,
    period: usize,
    now: String,
    status: String,
    tex: HashMap<String, Option<TextureHandle>>,
    ask: Sender<String>,
    got: Receiver<Thumb>,
}

enum Act {
    Copy(usize),
    Edit(usize),
    Open(usize),
    Remove(usize),
}

impl History {
    fn new(hooks: Hooks, ctx: egui::Context) -> Self {
        let (ask, wrx) = channel();
        let (wtx, got) = channel();
        std::thread::spawn(move || thumbs(wrx, wtx, ctx));
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

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((path, img)) = self.got.try_recv() {
            let tex = img.map(|i| ctx.load_texture(&path, i, TextureOptions::LINEAR));
            self.tex.insert(path, tex);
        }
    }

    fn apply(&mut self, act: Act) {
        let res = match act {
            Act::Copy(i) => {
                let e = &self.entries[i];
                let r = if e.path.is_empty() {
                    (self.hooks.copy_text)(&e.text)
                } else {
                    (self.hooks.copy_image)(Path::new(&e.path))
                };
                r.map(|()| "Скопировано в буфер".to_string())
            }
            Act::Edit(i) => {
                let e = &self.entries[i];
                (self.hooks.edit)(Path::new(&e.path), &e.app).map(|()| String::new())
            }
            Act::Open(i) => {
                (self.hooks.open)(Path::new(&self.entries[i].path));
                Ok(String::new())
            }
            Act::Remove(i) => history::remove(&self.hooks.file, &self.entries[i]).map(|()| {
                self.entries.remove(i);
                "Убрано из истории, файл на месте".to_string()
            }),
        };
        self.status = res.unwrap_or_else(|e| format!("Не получилось: {e}"));
    }

    fn title(e: &Entry) -> String {
        if !e.app.is_empty() {
            return e.app.clone();
        }
        match e.kind.as_str() {
            "screen" => "Весь экран",
            "window" => "Окно",
            "ocr" => "Текст",
            _ => "Область",
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

    fn card(&mut self, ui: &mut egui::Ui, rect: Rect, index: usize, hit: &str) -> Option<Act> {
        let ctx = ui.ctx().clone();
        let p = ui::pal(&ctx);
        let e = self.entries[index].clone();
        let hover = ui.rect_contains_pointer(rect);
        let rect = rect.translate(vec2(0.0, if hover { -2.0 } else { 0.0 }));
        ui.painter().rect_filled(rect, 16.0, p.card);
        ui::outline(ui, rect, 16.0);
        let inner = rect.shrink(8.0);
        let shot = Rect::from_min_size(inner.min, vec2(inner.width(), THUMB_H));
        let tex = self.thumb(&e.path);
        let open = ui.interact(shot, ui.id().with(("open", index)), Sense::click());
        ui.painter().rect_filled(shot, 10.0, p.well);
        match &tex {
            Some(t) => {
                let [w, h] = t.size().map(|v| v as f32);
                // Cover, not stretch: crop the longer side.
                let k = (shot.width() / w).max(shot.height() / h);
                let (u, v) = (shot.width() / w / k, shot.height() / h / k);
                let uv = Rect::from_center_size(pos2(0.5, 0.5), vec2(u, v));
                egui::Image::new((t.id(), shot.size()))
                    .uv(uv)
                    .corner_radius(10)
                    .paint_at(ui, shot);
            }
            None if e.path.is_empty() => {
                let mut area = ui.new_child(UiBuilder::new().max_rect(shot.shrink(10.0)));
                area.set_clip_rect(shot.shrink(10.0));
                area.label(RichText::new(&e.text).size(12.5).color(p.muted));
            }
            None => {}
        }
        ui::outline(ui, shot, 10.0);
        let mut act = open
            .double_clicked()
            .then(|| (!e.path.is_empty()).then_some(Act::Open(index)))
            .flatten();

        let text_top = shot.bottom() + 10.0;
        let text = Rect::from_min_max(
            pos2(inner.left() + 6.0, text_top),
            pos2(inner.right() - 6.0, inner.bottom() - 40.0),
        );
        let mut area = ui.new_child(UiBuilder::new().max_rect(text));
        area.set_clip_rect(text.intersect(ui.clip_rect()));
        area.spacing_mut().item_spacing.y = 3.0;
        area.label(RichText::new(Self::title(&e)).font(ui::semi(14.0)));
        if hit.is_empty() {
            area.label(RichText::new(self.meta(&e)).size(12.5).color(p.muted));
        } else {
            let pill = format!("«{hit}»");
            area.label(RichText::new(pill).size(12.0).color(p.warn));
        }

        let bar = Rect::from_min_max(
            pos2(inner.left() + 2.0, inner.bottom() - 34.0),
            pos2(inner.right() - 2.0, inner.bottom()),
        );
        let mut row = ui.new_child(
            UiBuilder::new()
                .max_rect(bar)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        row.spacing_mut().item_spacing.x = 6.0;
        let mut rest = bar.width() - 34.0 - 6.0;
        if !e.path.is_empty() {
            rest -= 86.0 + 6.0;
        }
        if ui::primary(&mut row, "Копировать", Some(rest), 34.0).clicked() {
            act = Some(Act::Copy(index));
        }
        if !e.path.is_empty() && ui::tone(&mut row, "Редактор", Some(86.0), 34.0).clicked()
        {
            act = Some(Act::Edit(index));
        }
        if ui::ghost_icon(&mut row, icon::X, 34.0, "Убрать из истории").clicked() {
            act = Some(Act::Remove(index));
        }
        act
    }
}

impl eframe::App for History {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ui::preview(&ctx);
        self.poll(&ctx);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui::frame_edges(&ctx);
        let p = ui::pal(&ctx);
        egui::Panel::top("head")
            .show_separator_line(false)
            .frame(Frame::new().fill(p.bg).inner_margin(Margin {
                left: 20,
                right: 20,
                top: 18,
                bottom: 14,
            }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 14.0;
                let top = Rect::from_min_size(ui.max_rect().min, vec2(ui.available_width(), 36.0));
                ui::drag_area(ui, top);
                ui.horizontal(|ui| {
                    ui::heading(ui, "История", 24.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::ghost_icon(ui, icon::X, 36.0, "Закрыть").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui::tone(ui, "Открыть папку", None, 36.0).clicked() {
                            (self.hooks.open_folder)();
                        }
                    });
                });
                ui.horizontal(|ui| {
                    let right = 240.0;
                    Frame::new()
                        .fill(p.card)
                        .stroke(egui::Stroke::new(1.0, p.line))
                        .corner_radius(12)
                        .inner_margin(Margin::symmetric(14, 0))
                        .show(ui, |ui| {
                            ui.set_height(42.0);
                            ui.set_width(ui.available_width() - right);
                            ui.horizontal_centered(|ui| {
                                ui.label(
                                    RichText::new(icon::MAGNIFYING_GLASS)
                                        .font(ui::icon_font(18.0))
                                        .color(p.muted),
                                );
                                ui.add(
                                    TextEdit::singleline(&mut self.query)
                                        .frame(Frame::NONE)
                                        .hint_text("Поиск по тексту на снимках")
                                        .desired_width(f32::INFINITY),
                                );
                            });
                        });
                    let items = ["Всё", "Сегодня", "Неделя"];
                    if let Some(i) = ui::segmented(ui, &items, self.period, 34.0) {
                        self.period = i;
                    }
                });
            });
        egui::Panel::bottom("foot")
            .show_separator_line(false)
            .frame(Frame::new().fill(p.bg).inner_margin(Margin {
                left: 20,
                right: 20,
                top: 10,
                bottom: 14,
            }))
            .show(ui, |ui| {
                let text = if self.status.is_empty() {
                    "Убрать из истории — файл на диске остаётся. История хранится только на этом \
                     компьютере."
                } else {
                    &self.status
                };
                ui.label(RichText::new(text).size(12.5).color(p.muted));
            });
        let visible = self.visible();
        let mut act = None;
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(20, 4)),
            )
            .show(ui, |ui| {
                if visible.is_empty() {
                    ui.centered_and_justified(|ui| {
                        let text = if self.entries.is_empty() {
                            "Здесь появятся сохранённые снимки"
                        } else {
                            "Ничего не найдено"
                        };
                        ui.label(RichText::new(text).color(p.muted));
                    });
                    return;
                }
                let columns = (((ui.available_width() + GAP) / 300.0) as usize).max(1);
                let width = (ui.available_width() - GAP * (columns - 1) as f32) / columns as f32;
                let rows = visible.len().div_ceil(columns);
                egui::ScrollArea::vertical().show_rows(
                    ui,
                    CARD_H + GAP - ui.spacing().item_spacing.y,
                    rows,
                    |ui, range| {
                        for r in range {
                            let (rect, _) = ui.allocate_exact_size(
                                vec2(ui.available_width(), CARD_H + GAP),
                                Sense::hover(),
                            );
                            for c in 0..columns {
                                let Some((index, hit)) = visible.get(r * columns + c) else {
                                    break;
                                };
                                let min = rect.min + vec2((width + GAP) * c as f32, 0.0);
                                let cell = Rect::from_min_size(min, vec2(width, CARD_H));
                                if let Some(a) = self.card(ui, cell, *index, hit) {
                                    act = Some(a);
                                }
                            }
                        }
                    },
                );
            });
        if let Some(a) = act {
            self.apply(a);
        }
    }
}
