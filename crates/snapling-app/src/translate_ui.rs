//! OCR + translation window, its own process (`snapling --translate <text file>`) like the
//! editor: the model (~250 MB in memory) lives only while the window is open.

use crate::ui;
use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};
use egui_phosphor::regular as icon;
use snapling_translate::{Dir, Model};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct Hooks {
    pub copy_text: fn(&str) -> Result<(), String>,
    pub warn: fn(&str),
    pub models: PathBuf,
    /// `[translate] online = "deepl"`.
    pub online: bool,
    pub get_key: fn() -> Option<String>,
    pub set_key: fn(&str) -> Result<(), String>,
}

type Reply = (u64, Result<String, String>);
/// id, direction, text, DeepL key (`None` = offline).
type Job = (u64, Dir, String, Option<String>);

pub fn run(file: &Path, hooks: Hooks) {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            return (hooks.warn)(&format!(
                "Не удалось открыть текст.\n{}: {e}",
                file.display()
            ));
        }
    };
    let _ = std::fs::remove_file(file);
    let options = eframe::NativeOptions {
        viewport: ui::viewport("Snapling — текст и перевод", [400.0, 600.0], [360.0, 480.0]),
        ..Default::default()
    };
    let warn = hooks.warn;
    let res = eframe::run_native(
        "Snapling translate",
        options,
        Box::new(move |cc| {
            ui::init(cc);
            Ok(Box::new(Window::new(text, hooks, cc.egui_ctx.clone())))
        }),
    );
    if let Err(e) = res {
        warn(&format!("Окно перевода не запустилось.\n{e}"));
    }
}

/// Loads each direction's model once and translates requests in order; a key = DeepL.
fn worker(models: PathBuf, rx: Receiver<Job>, tx: Sender<Reply>, ctx: egui::Context) {
    let mut loaded: Vec<(Dir, Model)> = Vec::new();
    for (id, dir, text, key) in rx {
        let res = (|| {
            if let Some(key) = key {
                return snapling_translate::deepl(&key, &text, dir);
            }
            if !loaded.iter().any(|(d, _)| *d == dir) {
                loaded.push((dir, Model::load(&models, dir)?));
            }
            let (_, m) = loaded
                .iter_mut()
                .find(|(d, _)| *d == dir)
                .expect("just loaded");
            m.translate(&text)
        })();
        if tx.send((id, res)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

struct Window {
    original: String,
    translation: String,
    dir: Dir,
    hooks: Hooks,
    tx: Sender<Job>,
    rx: Receiver<Reply>,
    /// Id of the newest request; older replies are dropped.
    pending: Option<u64>,
    next_id: u64,
    copied_once: bool,
    status: String,
    /// Where the shown translation came from.
    via_deepl: bool,
    pending_deepl: bool,
    /// Key being typed (`Some` = the key row is open).
    key_input: Option<String>,
    /// Send confirmation shown; agreed once per window.
    confirm: bool,
    consented: bool,
}

impl Window {
    fn new(original: String, hooks: Hooks, ctx: egui::Context) -> Self {
        let (tx, wrx) = channel();
        let (wtx, rx) = channel();
        let models = hooks.models.clone();
        std::thread::spawn(move || worker(models, wrx, wtx, ctx));
        let mut w = Self {
            dir: snapling_translate::detect(&original),
            original,
            translation: String::new(),
            hooks,
            tx,
            rx,
            pending: None,
            next_id: 0,
            copied_once: false,
            status: String::new(),
            via_deepl: false,
            pending_deepl: false,
            key_input: None,
            confirm: false,
            consented: false,
        };
        w.request(None);
        w
    }

    fn request(&mut self, key: Option<String>) {
        if self.original.trim().is_empty() {
            return;
        }
        self.next_id += 1;
        self.pending = Some(self.next_id);
        self.pending_deepl = key.is_some();
        self.status = if key.is_some() {
            "Отправляю в DeepL…"
        } else {
            "Перевожу офлайн…"
        }
        .into();
        let _ = self
            .tx
            .send((self.next_id, self.dir, self.original.clone(), key));
    }

    /// DeepL button: key first, then a one-time confirmation, then the send.
    fn deepl(&mut self) {
        match (self.hooks.get_key)() {
            None => self.key_input = Some(String::new()),
            Some(_) if !self.consented => self.confirm = true,
            Some(key) => self.request(Some(key)),
        }
    }

    fn copy(&mut self) {
        self.status = match (self.hooks.copy_text)(&self.translation) {
            Ok(()) => "Перевод скопирован в буфер".into(),
            Err(e) => format!("Не удалось скопировать: {e}"),
        };
    }

    fn key_row(&mut self, ctx: &egui::Context) {
        let Some(key) = &mut self.key_input else {
            return;
        };
        let mut save = false;
        let resp = egui::Modal::new(egui::Id::new("deepl-key")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Ключ DeepL API");
            ui.label(
                "Ключ есть в аккаунте DeepL → API Keys. Бесплатный ключ оканчивается на «:fx».",
            );
            ui.label(
                egui::RichText::new("Хранится в диспетчере учётных данных Windows, не в файлах.")
                    .small()
                    .color(ui::pal(ctx).muted),
            );
            let r = ui.add(
                egui::TextEdit::singleline(key)
                    .password(true)
                    .hint_text("xxxxxxxx-…:fx"),
            );
            r.request_focus();
            ui.horizontal(|ui| {
                save = ui.button("Сохранить").clicked()
                    || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                if ui.button("Отмена").clicked() {
                    ui.close();
                }
            });
        });
        if save && !key.trim().is_empty() {
            let key = key.trim().to_string();
            self.key_input = None;
            match (self.hooks.set_key)(&key) {
                Ok(()) => self.deepl(),
                Err(e) => self.status = e,
            }
        } else if resp.should_close() {
            self.key_input = None;
        }
    }

    fn confirm_modal(&mut self, ctx: &egui::Context) {
        if !self.confirm {
            return;
        }
        let chars = self.original.chars().count();
        let mut send = false;
        let resp = egui::Modal::new(egui::Id::new("deepl-confirm")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Отправить в DeepL?");
            ui.label(format!(
                "Текст ({chars} символов) уйдёт на сервер DeepL для перевода. Офлайн-перевод ничего не отправляет."
            ));
            ui.horizontal(|ui| {
                send = ui.button("Отправить").clicked();
                if ui.button("Отмена").clicked() {
                    ui.close();
                }
            });
        });
        if send {
            self.confirm = false;
            self.consented = true;
            self.deepl();
        } else if resp.should_close() {
            self.confirm = false;
        }
    }

    fn poll(&mut self) {
        while let Ok((id, res)) = self.rx.try_recv() {
            if Some(id) != self.pending {
                continue;
            }
            self.pending = None;
            match res {
                Ok(t) => {
                    self.translation = t;
                    self.via_deepl = self.pending_deepl;
                    self.status.clear();
                    if !self.copied_once {
                        self.copied_once = true;
                        self.copy();
                    }
                }
                Err(e) => self.status = format!("Ошибка перевода: {e}"),
            }
        }
    }
}

impl Window {
    fn swap(&mut self) {
        self.dir = match self.dir {
            Dir::RuEn => Dir::EnRu,
            Dir::EnRu => Dir::RuEn,
        };
        self.request(None);
    }

    /// Lines joined with spaces: OCR keeps the layout's line breaks.
    fn one_line(&mut self) {
        let joined: Vec<&str> = self.original.split_whitespace().collect();
        self.original = joined.join(" ");
    }

    fn copy_original(&mut self) {
        self.status = match (self.hooks.copy_text)(&self.original) {
            Ok(()) => "Текст скопирован в буфер".into(),
            Err(e) => format!("Не удалось скопировать: {e}"),
        };
    }
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = ui.ctx().clone();
        ui::preview(&ctx);
        let (run, copy, close) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter)),
                i.consume_shortcut(&KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::SHIFT,
                    Key::C,
                )),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui::frame_edges(&ctx);
        let p = ui::pal(&ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.main).inner_margin(16))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 14.0;
                let width = ui.available_width();
                let top = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(width, 32.0));
                ui::drag_area(ui, top);
                ui.horizontal(|ui| {
                    ui::heading(ui, "Текст", 17.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::ghost_icon(ui, icon::X, 32.0, "Закрыть").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        egui::Frame::new()
                            .fill(p.ghost)
                            .corner_radius(10)
                            .inner_margin(egui::Margin::symmetric(9, 3))
                            .show(ui, |ui| {
                                let t = egui::RichText::new("Windows OCR").size(12.0);
                                ui.label(t.color(p.muted));
                            });
                    });
                });
                ui.add_sized(
                    [width, 104.0],
                    egui::TextEdit::multiline(&mut self.original)
                        .margin(egui::Margin::symmetric(14, 12)),
                );
                ui.horizontal(|ui| {
                    let line = 118.0;
                    if ui::primary(ui, "Копировать текст", Some(width - line - 8.0), 40.0).clicked()
                    {
                        self.copy_original();
                    }
                    if ui::tone(ui, "В одну строку", Some(line), 40.0).clicked() {
                        self.one_line();
                    }
                });
                ui.horizontal(|ui| {
                    ui::heading(ui, "Перевод", 17.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.hooks.online {
                            let items = ["На устройстве", "DeepL"];
                            let now = usize::from(self.via_deepl);
                            match ui::segmented(ui, &items, now, 28.0) {
                                Some(1) => self.deepl(),
                                Some(_) => self.request(None),
                                None => {}
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    let (from, to) = match self.dir {
                        Dir::RuEn => ("Русский", "Английский"),
                        Dir::EnRu => ("Английский", "Русский"),
                    };
                    let w = (width - 36.0 - 16.0) / 2.0;
                    let pill = |ui: &mut egui::Ui, text: &str| {
                        let size = egui::vec2(w, 36.0);
                        let (r, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                        ui.painter().rect_filled(r, 10.0, p.ghost);
                        ui.painter().text(
                            r.left_center() + egui::vec2(12.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            text,
                            egui::FontId::proportional(13.5),
                            p.text,
                        );
                        resp.clicked()
                    };
                    let a = pill(ui, from);
                    let b = ui::ghost_icon(ui, icon::ARROWS_LEFT_RIGHT, 36.0, "Поменять языки");
                    let c = pill(ui, to);
                    if a || b.clicked() || c {
                        self.swap();
                    }
                });
                let foot = 38.0 + ui.spacing().item_spacing.y;
                let h = ui.available_height() - foot;
                egui::Frame::new()
                    .fill(p.tone)
                    .stroke(egui::Stroke::new(1.0, ui::ACCENT.gamma_multiply(0.45)))
                    .corner_radius(12)
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width(width - 28.0);
                        ui.set_height(h - 24.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let mut view = self.translation.as_str();
                            ui.add(
                                egui::TextEdit::multiline(&mut view)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    });
                ui.horizontal(|ui| {
                    let via = if !self.status.is_empty() {
                        self.status.clone()
                    } else if self.via_deepl {
                        "DeepL · онлайн".into()
                    } else {
                        "OPUS-MT · без интернета".into()
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(!self.translation.is_empty(), |ui| {
                            if ui::tone(ui, "Копировать перевод", None, 38.0)
                                .on_hover_text("Ctrl+Shift+C")
                                .clicked()
                            {
                                self.copy();
                            }
                        });
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let t = egui::RichText::new(via).size(12.0).color(p.muted);
                            ui.add(egui::Label::new(t).truncate());
                        });
                    });
                });
            });
        if run {
            self.request(None);
        }
        self.key_row(&ctx);
        self.confirm_modal(&ctx);
        if copy && !self.translation.is_empty() {
            self.copy();
        }
    }
}
