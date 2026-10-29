//! OCR + translation window, its own process (`snapling --translate <text file>`) like the
//! editor: the model (~250 MB in memory) lives only while the window is open.

use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers};
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
        viewport: egui::ViewportBuilder::default()
            .with_title("Snapling — текст и перевод")
            .with_inner_size([820.0, 480.0])
            .with_min_inner_size([480.0, 300.0])
            .with_icon(egui::IconData {
                rgba: crate::icon::frame_spark(64, [0xFF, 0xFF, 0xFF], [0xFF, 0x8A, 0x1F]),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    let warn = hooks.warn;
    let res = eframe::run_native(
        "Snapling translate",
        options,
        Box::new(move |cc| {
            crate::editor::setup_style(&cc.egui_ctx);
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
                    .color(Color32::from_gray(150)),
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

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = ui.ctx().clone();
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
        let bar = Color32::from_rgb(0x16, 0x1B, 0x26);
        egui::Panel::top("bar")
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .fill(bar),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let swap = format!(
                        "{}  {}",
                        self.dir.label(),
                        egui_phosphor::regular::ARROWS_LEFT_RIGHT
                    );
                    if ui
                        .button(swap)
                        .on_hover_text("Сменить направление")
                        .clicked()
                    {
                        self.dir = match self.dir {
                            Dir::RuEn => Dir::EnRu,
                            Dir::EnRu => Dir::RuEn,
                        };
                        self.request(None);
                    }
                    let via = if self.via_deepl {
                        "переведено через DeepL"
                    } else {
                        "офлайн, без отправки в сеть"
                    };
                    ui.label(
                        egui::RichText::new(via)
                            .small()
                            .color(Color32::from_gray(150)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let accent = Color32::from_rgb(0xFF, 0x8A, 0x1F);
                        let b = egui::Button::new(
                            egui::RichText::new("Копировать перевод").color(Color32::BLACK),
                        )
                        .fill(accent);
                        if ui
                            .add_enabled(!self.translation.is_empty(), b)
                            .on_hover_text("Ctrl+Shift+C")
                            .clicked()
                        {
                            self.copy();
                        }
                        if ui
                            .button("Перевести")
                            .on_hover_text("Ctrl+Enter — офлайн")
                            .clicked()
                        {
                            self.request(None);
                        }
                        if self.hooks.online {
                            let b = ui
                                .button(format!("{} DeepL", egui_phosphor::regular::CLOUD_ARROW_UP))
                                .on_hover_text("Отправит текст в DeepL и покажет их перевод");
                            if b.clicked() {
                                self.deepl();
                            }
                        }
                    });
                });
            });
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .fill(bar),
            )
            .show(ui, |ui| {
                let hint = "Текст можно поправить и перевести заново · Esc — закрыть";
                let text = if self.status.is_empty() {
                    hint
                } else {
                    &self.status
                };
                ui.label(
                    egui::RichText::new(text)
                        .small()
                        .color(Color32::from_gray(170)),
                );
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x0E, 0x11, 0x18))
                    .inner_margin(12),
            )
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    let h = cols[0].available_height() - 24.0;
                    cols[0].label("Распознанный текст");
                    egui::ScrollArea::vertical()
                        .id_salt("src")
                        .max_height(h)
                        .show(&mut cols[0], |ui| {
                            ui.add_sized(
                                [ui.available_width(), h],
                                egui::TextEdit::multiline(&mut self.original)
                                    .font(egui::TextStyle::Body),
                            );
                        });
                    cols[1].label("Перевод");
                    egui::ScrollArea::vertical()
                        .id_salt("dst")
                        .max_height(h)
                        .show(&mut cols[1], |ui| {
                            let mut view = self.translation.as_str();
                            ui.add_sized(
                                [ui.available_width(), h],
                                egui::TextEdit::multiline(&mut view).font(egui::TextStyle::Body),
                            );
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
