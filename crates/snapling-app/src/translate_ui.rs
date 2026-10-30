//! OCR + translation window, its own process (`snapling --translate <text file>`) like the
//! editor: the model (~250 MB in memory) lives only while the window is open.

use crate::ui;
use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};
use egui_phosphor::regular as icon;
use snapling_core::config::Online;
use snapling_core::i18n::{t, tf};
use snapling_translate::{Dir, Model};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct Hooks {
    pub copy_text: fn(&str) -> Result<(), String>,
    pub warn: fn(&str),
    pub models: PathBuf,
    /// `[translate] online`.
    pub online: Online,
    /// Name on the chip.
    pub engine: &'static str,
    pub get_key: fn(Online) -> Option<String>,
    pub set_key: fn(Online, &str) -> Result<(), String>,
    /// Proxy for online requests, from Credential Manager: it may hold a password.
    pub get_proxy: fn() -> Option<String>,
}

type Reply = (u64, Result<String, String>);
/// id, direction, text, service and key (`None` = offline).
type Job = (u64, Dir, String, Option<(Online, String)>);

/// `text_only`: the result of recognition, translated only when asked.
pub fn run(file: &Path, hooks: Hooks, text_only: bool) {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            return (hooks.warn)(&tf(
                "Не удалось открыть текст.\n{0}: {e}",
                &[("e", &e), ("0", &file.display())],
            ));
        }
    };
    let _ = std::fs::remove_file(file);
    let options = eframe::NativeOptions {
        viewport: ui::viewport(
            t("Snapling — текст и перевод"),
            [400.0, 600.0],
            [360.0, 480.0],
        ),
        ..Default::default()
    };
    let warn = hooks.warn;
    let res = eframe::run_native(
        "Snapling translate",
        options,
        Box::new(move |cc| {
            ui::init(cc);
            Ok(Box::new(Window::new(
                text,
                hooks,
                text_only,
                cc.egui_ctx.clone(),
            )))
        }),
    );
    if let Err(e) = res {
        warn(&tf("Окно перевода не запустилось.\n{e}", &[("e", &e)]));
    }
}

/// Loads each direction's model once and translates requests in order; a key = online.
fn worker(
    models: PathBuf,
    proxy: fn() -> Option<String>,
    rx: Receiver<Job>,
    tx: Sender<Reply>,
    ctx: egui::Context,
) {
    let mut loaded: Vec<(Dir, Model)> = Vec::new();
    for (id, dir, text, key) in rx {
        let res = (|| {
            if let Some((service, key)) = key {
                let proxy = proxy();
                return snapling_translate::online(service, &key, &text, dir, proxy.as_deref());
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
    text_only: bool,
    hooks: Hooks,
    tx: Sender<Job>,
    rx: Receiver<Reply>,
    /// Id of the newest request; older replies are dropped.
    pending: Option<u64>,
    next_id: u64,
    copied_once: bool,
    status: String,
    /// Where the shown translation came from.
    via_online: bool,
    pending_online: bool,
    /// Key being typed (`Some` = the key row is open).
    key_input: Option<String>,
    /// Send confirmation shown; agreed once per window.
    confirm: bool,
    consented: bool,
}

impl Window {
    fn new(original: String, hooks: Hooks, text_only: bool, ctx: egui::Context) -> Self {
        let (tx, wrx) = channel();
        let (wtx, rx) = channel();
        let (models, proxy) = (hooks.models.clone(), hooks.get_proxy);
        std::thread::spawn(move || worker(models, proxy, wrx, wtx, ctx));
        let mut w = Self {
            dir: snapling_translate::detect(&original),
            original,
            translation: String::new(),
            text_only,
            hooks,
            tx,
            rx,
            pending: None,
            next_id: 0,
            copied_once: false,
            status: String::new(),
            via_online: false,
            pending_online: false,
            key_input: None,
            confirm: false,
            consented: false,
        };
        if !text_only {
            w.request(None);
        }
        w
    }

    fn request(&mut self, key: Option<(Online, String)>) {
        if self.original.trim().is_empty() {
            return;
        }
        self.next_id += 1;
        self.pending = Some(self.next_id);
        self.pending_online = key.is_some();
        self.status = match &key {
            Some((service, _)) => tf("Отправляю в {0}…", &[("0", &service.name())]),
            None => t("Перевожу офлайн…").into(),
        };
        let _ = self
            .tx
            .send((self.next_id, self.dir, self.original.clone(), key));
    }

    /// Service button: key first, then a one-time confirmation, then the send.
    fn send_online(&mut self) {
        let service = self.hooks.online;
        match (self.hooks.get_key)(service) {
            None => self.key_input = Some(String::new()),
            Some(_) if !self.consented => self.confirm = true,
            Some(key) => self.request(Some((service, key))),
        }
    }

    fn copy(&mut self) {
        self.status = match (self.hooks.copy_text)(&self.translation) {
            Ok(()) => t("Перевод скопирован в буфер").into(),
            Err(e) => tf("Не удалось скопировать: {e}", &[("e", &e)]),
        };
    }

    fn key_row(&mut self, ctx: &egui::Context) {
        let Some(key) = &mut self.key_input else {
            return;
        };
        let service = self.hooks.online;
        let mut save = false;
        let resp = egui::Modal::new(egui::Id::new("online-key")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tf("Ключ {0} API", &[("0", &service.name())]));
            ui.label(match service {
                Online::Yandex => t(
                    "Создайте сервисный аккаунт в Yandex Cloud с ролью ai.translate.user и \
                     API-ключ для него. Перевод платный.",
                ),
                Online::Google => t(
                    "Включите Cloud Translation API в Google Cloud и создайте API-ключ в разделе \
                     Credentials. Перевод платный.",
                ),
                _ => t(
                    "Ключ есть в аккаунте DeepL → API Keys. Бесплатный ключ оканчивается на «:fx».",
                ),
            });
            ui.label(
                egui::RichText::new(t(
                    "Хранится в диспетчере учётных данных Windows, не в файлах.",
                ))
                .small()
                .color(ui::pal(ctx).muted),
            );
            let r = ui.add(egui::TextEdit::singleline(key).password(true));
            r.request_focus();
            ui.horizontal(|ui| {
                save = ui.button(t("Сохранить")).clicked()
                    || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                if ui.button(t("Отмена")).clicked() {
                    ui.close();
                }
            });
        });
        if save && !key.trim().is_empty() {
            let key = key.trim().to_string();
            self.key_input = None;
            match (self.hooks.set_key)(service, &key) {
                Ok(()) => self.send_online(),
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
        let name = self.hooks.online.name();
        let mut send = false;
        let resp = egui::Modal::new(egui::Id::new("online-confirm")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tf("Отправить в {0}?", &[("0", &name)]));
            ui.label(tf(
                "Текст ({chars} символов) уйдёт на сервер {0} для перевода. Офлайн-перевод ничего \
                 не отправляет.",
                &[("chars", &chars), ("0", &name)],
            ));
            ui.horizontal(|ui| {
                send = ui.button(t("Отправить")).clicked();
                if ui.button(t("Отмена")).clicked() {
                    ui.close();
                }
            });
        });
        if send {
            self.confirm = false;
            self.consented = true;
            self.send_online();
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
                    self.via_online = self.pending_online;
                    self.status.clear();
                    if !self.copied_once {
                        self.copied_once = true;
                        self.copy();
                    }
                }
                Err(e) => self.status = tf("Ошибка перевода: {e}", &[("e", &e)]),
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
            Ok(()) => t("Текст скопирован в буфер").into(),
            Err(e) => tf("Не удалось скопировать: {e}", &[("e", &e)]),
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
                    ui::heading(ui, t("Текст"), 17.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::ghost_icon(ui, icon::X, 32.0, t("Закрыть")).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        egui::Frame::new()
                            .fill(p.ghost)
                            .corner_radius(10)
                            .inner_margin(egui::Margin::symmetric(9, 3))
                            .show(ui, |ui| {
                                let t = egui::RichText::new(self.hooks.engine).size(12.0);
                                ui.label(t.color(p.muted));
                            });
                    });
                });
                let gap = ui.spacing().item_spacing.y;
                let high = if self.text_only {
                    ui.available_height() - 40.0 - 38.0 - gap * 2.0
                } else {
                    104.0
                };
                ui.add_sized(
                    [width, high],
                    egui::TextEdit::multiline(&mut self.original)
                        .margin(egui::Margin::symmetric(14, 12)),
                );
                ui.horizontal(|ui| {
                    let line = 118.0;
                    if ui::primary(ui, t("Копировать текст"), Some(width - line - 8.0), 40.0)
                        .clicked()
                    {
                        self.copy_original();
                    }
                    if ui::tone(ui, t("В одну строку"), Some(line), 40.0).clicked() {
                        self.one_line();
                    }
                });
                if self.text_only {
                    let mut go = false;
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            go = ui::tone(ui, t("Перевести"), None, 38.0).clicked();
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let t =
                                        egui::RichText::new(&self.status).size(12.0).color(p.muted);
                                    ui.add(egui::Label::new(t).truncate());
                                },
                            );
                        });
                    });
                    if go {
                        self.text_only = false;
                        self.request(None);
                    }
                    return;
                }
                ui.horizontal(|ui| {
                    ui::heading(ui, t("Перевод"), 17.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.hooks.online != Online::Off {
                            let items = [t("На устройстве"), self.hooks.online.name()];
                            let now = usize::from(self.via_online);
                            match ui::segmented(ui, &items, now, 28.0) {
                                Some(1) => self.send_online(),
                                Some(_) => self.request(None),
                                None => {}
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    let (from, to) = match self.dir {
                        Dir::RuEn => (t("Русский"), t("Английский")),
                        Dir::EnRu => (t("Английский"), t("Русский")),
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
                    let b = ui::ghost_icon(ui, icon::ARROWS_LEFT_RIGHT, 36.0, t("Поменять языки"));
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
                    } else if self.via_online {
                        tf("{0} · онлайн", &[("0", &self.hooks.online.name())])
                    } else {
                        t("OPUS-MT · без интернета").into()
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(!self.translation.is_empty(), |ui| {
                            if ui::tone(ui, t("Копировать перевод"), None, 38.0)
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
        if run && !self.text_only {
            self.request(None);
        }
        self.key_row(&ctx);
        self.confirm_modal(&ctx);
        if copy && !self.translation.is_empty() {
            self.copy();
        }
    }
}
