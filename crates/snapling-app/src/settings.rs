//! Settings window, its own process (`snapling --settings`). Every change goes straight into
//! config.toml, comments kept; the tray picks it up by mtime, like a hand edit.

use crate::ui::{self, caption, card, divider, row, segmented, toggle};
use eframe::egui::{self, Align, Color32, Frame, Layout, Margin, RichText, Sense, TextEdit, vec2};
use egui_phosphor::regular as icon;
use snapling_core::config::{Action, Config, Format, Hotkeys, Online, SoundCfg, SoundPack, Theme};
use snapling_core::hotkey::{self, Hotkey};
use snapling_core::sound::Sound;
use snapling_core::storage;
use std::path::{Path, PathBuf};

pub struct Hooks {
    pub warn: fn(&str),
    pub autostart: fn() -> bool,
    pub set_autostart: fn(bool) -> Result<(), String>,
    pub pick_folder: fn() -> Option<String>,
    pub pressed_combo: fn() -> Option<String>,
    pub pause_hotkeys: fn(bool),
    pub reserved: fn(&Hotkey) -> Option<&'static str>,
    pub open: fn(&Path),
    pub log: PathBuf,
    /// Folder used when `[save] dir` is empty.
    pub default_dir: fn() -> Option<PathBuf>,
    pub pick_wav: fn() -> Option<String>,
    /// Plays one sound as these settings would.
    pub preview: fn(&SoundCfg, Sound) -> Result<(), String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    General,
    Capture,
    Hotkeys,
    Text,
    Privacy,
    About,
}

const TABS: [(Tab, &str, &str); 6] = [
    (Tab::General, icon::GEAR_SIX, "Основные"),
    (Tab::Capture, icon::CORNERS_OUT, "Снимки"),
    (Tab::Hotkeys, icon::KEYBOARD, "Горячие клавиши"),
    (Tab::Text, icon::TRANSLATE, "Текст и перевод"),
    (Tab::Privacy, icon::SHIELD_CHECK, "Приватность и сеть"),
    (Tab::About, icon::INFO, "О программе"),
];

pub fn run(hooks: Hooks) {
    let path = ui::config_path();
    let cfg = match Config::load_or_create(&path) {
        Ok(c) => c,
        Err(e) => return (hooks.warn)(&format!("Ошибка в config.toml.\n{e}")),
    };
    let options = eframe::NativeOptions {
        viewport: ui::viewport("Snapling — настройки", [1040.0, 700.0], [760.0, 480.0]),
        ..Default::default()
    };
    let warn = hooks.warn;
    let res = eframe::run_native(
        "Snapling settings",
        options,
        Box::new(move |cc| {
            ui::init(cc);
            Ok(Box::new(Settings::new(path, cfg, hooks)))
        }),
    );
    if let Err(e) = res {
        warn(&format!("Окно настроек не запустилось.\n{e}"));
    }
}

/// `SNAP_TAB=<0-5>` opens that tab in debug builds, for `ui::preview`.
fn first_tab() -> Tab {
    let wanted = cfg!(debug_assertions)
        .then(|| std::env::var("SNAP_TAB").ok()?.parse::<usize>().ok())
        .flatten();
    wanted
        .and_then(|i| TABS.get(i))
        .map_or(Tab::General, |t| t.0)
}

/// Writes keys and remembers the last failure for the status line.
struct Store {
    path: PathBuf,
    error: String,
}

impl Store {
    fn set(&mut self, section: &str, key: &str, value: impl Into<toml::Value>) {
        self.error = Config::update(&self.path, section, key, value)
            .err()
            .unwrap_or_default();
    }
}

struct Settings {
    cfg: Config,
    store: Store,
    hooks: Hooks,
    tab: Tab,
    autostart: bool,
    recording: Option<Action>,
}

impl Settings {
    fn new(path: PathBuf, cfg: Config, hooks: Hooks) -> Self {
        Self {
            cfg,
            store: Store {
                path,
                error: String::new(),
            },
            autostart: (hooks.autostart)(),
            hooks,
            tab: first_tab(),
            recording: None,
        }
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui.spacing_mut().item_spacing.y = 2.0;
        for (tab, glyph, label) in TABS {
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
            let on = self.tab == tab;
            let fill = if on {
                p.tone
            } else if r.hovered() {
                p.ghost
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 10.0, fill);
            let color = if on { p.tone_text } else { p.soft };
            let font = if on {
                ui::semi(14.0)
            } else {
                egui::FontId::proportional(14.0)
            };
            let y = rect.center().y;
            ui.painter().text(
                egui::pos2(rect.left() + 12.0, y),
                egui::Align2::LEFT_CENTER,
                glyph,
                ui::icon_font(18.0),
                color,
            );
            ui.painter().text(
                egui::pos2(rect.left() + 40.0, y),
                egui::Align2::LEFT_CENTER,
                label,
                font,
                color,
            );
            if r.clicked() {
                self.tab = tab;
                self.stop_recording();
            }
        }
    }

    fn flag(
        ui: &mut egui::Ui,
        store: &mut Store,
        title: &str,
        hint: &str,
        value: &mut bool,
        key: (&str, &str),
    ) {
        row(ui, title, hint, |ui| {
            if toggle(ui, value).changed() {
                store.set(key.0, key.1, *value);
            }
        });
    }

    fn general(&mut self, ui: &mut egui::Ui) {
        let store = &mut self.store;
        ui::heading(ui, "Основные", 24.0);
        card(ui, |ui| {
            row(ui, "Запускать вместе с Windows", "", |ui| {
                if toggle(ui, &mut self.autostart).changed()
                    && let Err(e) = (self.hooks.set_autostart)(self.autostart)
                {
                    store.error = e;
                    self.autostart = (self.hooks.autostart)();
                }
            });
            divider(ui);
            row(ui, "Тема", "", |ui| {
                let now = match self.cfg.ui.theme {
                    Theme::System => 0,
                    Theme::Dark => 1,
                    Theme::Light => 2,
                };
                let items = ["Системная", "Тёмная", "Светлая"];
                if let Some(i) = segmented(ui, &items, now, 30.0) {
                    let (t, key) = [
                        (Theme::System, "system"),
                        (Theme::Dark, "dark"),
                        (Theme::Light, "light"),
                    ][i];
                    self.cfg.ui.theme = t;
                    store.set("ui", "theme", key);
                    ui::set_theme(ui.ctx(), t);
                }
            });
        });
        caption(ui, "Отклик");
        card(ui, |ui| {
            let s = &mut self.cfg.sound;
            Self::flag(ui, store, "Звуки", "", &mut s.enabled, ("sound", "enabled"));
            divider(ui);
            row(ui, "Набор звуков", "", |ui| {
                let packs = [
                    (SoundPack::Soft, "soft"),
                    (SoundPack::Real, "real"),
                    (SoundPack::System, "system"),
                ];
                let now = packs.iter().position(|p| p.0 == s.pack).unwrap_or(0);
                let items = ["Мягкий", "Реалистичный", "Системный"];
                if let Some(i) = segmented(ui, &items, now, 30.0) {
                    s.pack = packs[i].0;
                    store.set("sound", "pack", packs[i].1);
                    store.error = (self.hooks.preview)(s, Sound::Capture)
                        .err()
                        .unwrap_or_default();
                }
            });
            divider(ui);
            row(ui, "Громкость", "", |ui| {
                ui.spacing_mut().slider_width = 200.0;
                let r = ui.add(egui::Slider::new(&mut s.volume, 0..=100).show_value(false));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    store.set("sound", "volume", i64::from(s.volume));
                }
            });
            divider(ui);
            let title = "Без звука в играх на весь экран";
            let key = ("sound", "mute_in_games");
            Self::flag(ui, store, title, "", &mut s.mute_in_games, key);
        });
        caption(ui, "Свои звуки, файлы WAV");
        let events = [
            (Sound::Capture, "Снимок", "capture_file"),
            (Sound::Copy, "Копирование", "copy_file"),
            (Sound::Error, "Ошибка", "error_file"),
        ];
        card(ui, |ui| {
            for (i, (event, title, key)) in events.into_iter().enumerate() {
                if i > 0 {
                    divider(ui);
                }
                let s = &mut self.cfg.sound;
                let file = match event {
                    Sound::Capture => &mut s.capture_file,
                    Sound::Copy => &mut s.copy_file,
                    Sound::Error => &mut s.error_file,
                };
                let name = std::path::Path::new(file.as_str())
                    .file_name()
                    .map_or("Из набора".to_string(), |n| {
                        n.to_string_lossy().into()
                    });
                let (mut clear, mut pick, mut play) = (false, false, false);
                let has_file = !file.is_empty();
                row(ui, title, &name, |ui| {
                    if has_file {
                        clear =
                            ui::ghost_icon(ui, icon::X, 34.0, "Вернуть звук из набора").clicked();
                    }
                    pick = ui::tone(ui, "Выбрать…", None, 34.0).clicked();
                    play = ui::ghost_icon(ui, icon::PLAY, 34.0, "Послушать").clicked();
                });
                if clear {
                    file.clear();
                    store.set("sound", key, "");
                }
                if pick && let Some(path) = (self.hooks.pick_wav)() {
                    *file = path.clone();
                    store.set("sound", key, path);
                    play = true;
                }
                if play {
                    store.error = (self.hooks.preview)(s, event).err().unwrap_or_default();
                }
            }
        });
    }

    fn folder_card(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        let dir = self.cfg.save.dir.trim().to_string();
        let resolved = if dir.is_empty() {
            (self.hooks.default_dir)()
        } else {
            Some(PathBuf::from(&dir))
        };
        let problem = match &resolved {
            None => Some("Папка «Изображения» не найдена".to_string()),
            Some(d) => storage::shape_error(d),
        };
        card(ui, |ui| {
            Frame::new()
                .inner_margin(Margin {
                    left: 14,
                    right: 8,
                    top: 8,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(icon::FOLDER)
                                .font(ui::icon_font(20.0))
                                .color(p.tone_text),
                        );
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            let shown = resolved
                                .as_ref()
                                .map_or(String::new(), |d| d.display().to_string());
                            ui.label(RichText::new(shown).font(ui::mono(14.0)));
                            match &problem {
                                Some(e) => {
                                    ui.label(RichText::new(e).size(12.5).color(p.warn));
                                }
                                None => {
                                    ui.label(
                                        RichText::new("Папка подходит").size(12.5).color(p.ok),
                                    );
                                }
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui::tone(ui, "Открыть", None, 34.0).clicked()
                                && let Some(d) = &resolved
                            {
                                (self.hooks.open)(d);
                            }
                            if ui::tone(ui, "Выбрать…", None, 34.0).clicked()
                                && let Some(d) = (self.hooks.pick_folder)()
                            {
                                self.cfg.save.dir = d.clone();
                                self.store.set("save", "dir", d);
                            }
                        });
                    });
                });
        });
    }

    fn capture(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui::heading(ui, "Снимки", 24.0);
        caption(ui, "Папка для сохранения");
        self.folder_card(ui);
        let store = &mut self.store;
        let save = &mut self.cfg.save;
        card(ui, |ui| {
            row(ui, "Формат", "", |ui| {
                let now = match save.format {
                    Format::Png => 0,
                    Format::Webp => 1,
                    Format::Jpeg => 2,
                };
                if let Some(i) = segmented(ui, &["PNG", "WebP", "JPEG"], now, 30.0) {
                    let (f, key) = [
                        (Format::Png, "png"),
                        (Format::Webp, "webp"),
                        (Format::Jpeg, "jpeg"),
                    ][i];
                    save.format = f;
                    store.set("save", "format", key);
                }
            });
            if save.format == Format::Jpeg {
                divider(ui);
                row(ui, "Качество JPEG", "", |ui| {
                    ui.spacing_mut().slider_width = 200.0;
                    let r = ui.add(egui::Slider::new(&mut save.jpeg_quality, 1..=100));
                    if r.drag_stopped() || r.lost_focus() {
                        store.set("save", "jpeg_quality", i64::from(save.jpeg_quality));
                    }
                });
            }
            divider(ui);
            Frame::new()
                .inner_margin(Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label("Имя файла");
                    let r = ui.add(
                        TextEdit::singleline(&mut save.name)
                            .font(ui::mono(13.0))
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(12, 9)),
                    );
                    if r.lost_focus() {
                        store.set("save", "name", save.name.clone());
                    }
                    let sample = storage::Stamp {
                        year: 2026,
                        month: 10,
                        day: 29,
                        hour: 14,
                        minute: 21,
                        second: 7,
                    };
                    let name = storage::render_name(&save.name, sample, "Discord");
                    let shown = format!("{}.{}", name.display(), save.format.ext());
                    ui.label(RichText::new(shown).font(ui::mono(12.5)).color(p.muted));
                });
        });
        caption(ui, "После снимка");
        card(ui, |ui| {
            let c = &mut self.cfg.capture;
            Self::flag(
                ui,
                store,
                "Копировать в буфер",
                "",
                &mut c.copy,
                ("capture", "copy"),
            );
            divider(ui);
            let key = ("save", "auto_save");
            Self::flag(ui, store, "Сохранять в папку", "", &mut save.auto_save, key);
            divider(ui);
            let key = ("capture", "open_editor");
            Self::flag(ui, store, "Открывать редактор", "", &mut c.open_editor, key);
            divider(ui);
            let key = ("capture", "instant_copy");
            let hint = "Выделил — скопировано и закрыто, без панели действий.";
            Self::flag(
                ui,
                store,
                "Сразу копировать",
                hint,
                &mut c.instant_copy,
                key,
            );
        });
    }

    fn stop_recording(&mut self) {
        if self.recording.take().is_some() {
            (self.hooks.pause_hotkeys)(false);
        }
    }

    fn with_combo(hotkeys: &Hotkeys, a: Action, combo: &str) -> Hotkeys {
        let mut h = hotkeys.clone();
        *match a {
            Action::CaptureRegion => &mut h.capture_region,
            Action::CaptureScreen => &mut h.capture_screen,
            Action::CaptureWindow => &mut h.capture_window,
            Action::RepeatRegion => &mut h.repeat_region,
        } = combo.to_string();
        h
    }

    fn poll_recording(&mut self, ctx: &egui::Context) {
        let Some(action) = self.recording else {
            return;
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        if !ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            return self.stop_recording();
        }
        let Some(combo) = (self.hooks.pressed_combo)() else {
            return;
        };
        self.stop_recording();
        if combo == "Escape" {
            return;
        }
        let key = match action {
            Action::CaptureRegion => "capture_region",
            Action::CaptureScreen => "capture_screen",
            Action::CaptureWindow => "capture_window",
            Action::RepeatRegion => "repeat_region",
        };
        self.cfg.hotkeys = Self::with_combo(&self.cfg.hotkeys, action, &combo);
        self.store.set("hotkeys", key, combo);
    }

    fn hotkeys(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui::heading(ui, "Горячие клавиши", 24.0);
        let (_, problems) = hotkey::check(&self.cfg.hotkeys, self.hooks.reserved);
        let mut click = None;
        let mut clear = None;
        card(ui, |ui| {
            for (i, a) in Action::ALL.into_iter().enumerate() {
                if i > 0 {
                    divider(ui);
                }
                let problem = problems
                    .iter()
                    .find(|m| m.starts_with(&format!("«{}»", a.title())));
                let recording = self.recording == Some(a);
                let combo = self.cfg.hotkeys.combo(a);
                let label = match (recording, combo.is_empty()) {
                    (true, _) => "Нажми сочетание…",
                    (_, true) => "Не задано",
                    _ => combo,
                };
                let (fill, text) = if recording {
                    (p.tone, p.tone_text)
                } else if problem.is_some() {
                    (p.warn_bg, p.warn)
                } else {
                    (p.ghost, p.text)
                };
                Frame::new()
                    .inner_margin(Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        let size = vec2(ui.available_width(), 30.0);
                        let center = Layout::left_to_right(Align::Center);
                        ui.allocate_ui_with_layout(size, center, |ui| {
                            ui.label(a.title());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if !combo.is_empty()
                                    && ui::ghost_icon(ui, icon::X, 30.0, "Убрать сочетание")
                                        .clicked()
                                {
                                    clear = Some(a);
                                }
                                let g = ui.painter().layout_no_wrap(
                                    label.to_string(),
                                    ui::mono(13.0),
                                    text,
                                );
                                let size = vec2(g.size().x + 24.0, 30.0);
                                let (rect, r) = ui.allocate_exact_size(size, Sense::click());
                                ui.painter().rect_filled(rect, 15.0, fill);
                                if recording {
                                    ui::outline(ui, rect, 15.0);
                                }
                                ui.painter().galley(rect.center() - g.size() / 2.0, g, text);
                                if r.clicked() {
                                    click = Some(a);
                                }
                            });
                        });
                        if let Some(m) = problem {
                            let text = m.split_once(": ").map_or(m.as_str(), |(_, t)| t);
                            ui.label(RichText::new(text).size(12.5).color(p.warn));
                        }
                    });
            }
        });
        if let Some(a) = click {
            self.stop_recording();
            (self.hooks.pause_hotkeys)(true);
            (self.hooks.pressed_combo)();
            self.recording = Some(a);
        }
        if let Some(a) = clear {
            self.stop_recording();
            self.cfg.hotkeys = Self::with_combo(&self.cfg.hotkeys, a, "");
            let key = match a {
                Action::CaptureRegion => "capture_region",
                Action::CaptureScreen => "capture_screen",
                Action::CaptureWindow => "capture_window",
                Action::RepeatRegion => "repeat_region",
            };
            self.store.set("hotkeys", key, "");
        }
        ui.label(
            RichText::new(
                "Нажми на сочетание, чтобы записать новое, Esc — отмена. Какая программа заняла \
                 сочетание, Windows не сообщает — Snapling покажет сам конфликт.",
            )
            .size(13.0)
            .color(p.muted),
        );
    }

    fn text(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui::heading(ui, "Текст и перевод", 24.0);
        let store = &mut self.store;
        card(ui, |ui| {
            row(ui, "Распознавание", "", |ui| {
                ui.label(RichText::new("Windows OCR, встроенный").color(p.muted));
            });
            divider(ui);
            row(ui, "Языки текста", "", |ui| {
                let lang = &mut self.cfg.ocr.language;
                let now = match lang.as_str() {
                    "ru" => 1,
                    "en" => 2,
                    _ => 0,
                };
                let items = ["Как в Windows", "Русский", "Английский"];
                if let Some(i) = segmented(ui, &items, now, 30.0) {
                    *lang = ["auto", "ru", "en"][i].to_string();
                    store.set("ocr", "language", lang.clone());
                }
            });
        });
        ui.columns(2, |cols| {
            let online = self.cfg.translate.online;
            let opts = [
                (
                    Online::Off,
                    "На устройстве",
                    "OPUS-MT, без интернета. Модели ru↔en по ~117 МБ, скачиваются с вопросом.",
                    p.soft,
                    "off",
                ),
                (
                    Online::Deepl,
                    "DeepL, онлайн",
                    "Появится кнопка в окне перевода. Текст уходит только по ней, первый раз \
                     Snapling спросит.",
                    p.warn,
                    "deepl",
                ),
            ];
            for (ui, (v, title, note, color, key)) in cols.iter_mut().zip(opts) {
                let on = online == v;
                let r = Frame::new()
                    .fill(if on { p.tone } else { p.card })
                    .stroke(egui::Stroke::new(
                        if on { 1.5 } else { 1.0 },
                        if on { ui::ACCENT } else { p.line },
                    ))
                    .corner_radius(14)
                    .inner_margin(16)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(76.0);
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let dot = if on { icon::RADIO_BUTTON } else { icon::CIRCLE };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(dot).font(ui::icon_font(18.0)));
                            ui.label(RichText::new(title).font(ui::semi(14.0)));
                        });
                        ui.label(RichText::new(note).size(12.5).color(color));
                    })
                    .response;
                if r.interact(Sense::click()).clicked() && !on {
                    self.cfg.translate.online = v;
                    store.set("translate", "online", key);
                }
            }
        });
        ui.label(
            RichText::new(
                "Ключ DeepL вводится в окне перевода. Хранится в диспетчере учётных данных \
                 Windows, не попадает в логи и config.",
            )
            .size(13.0)
            .color(p.muted),
        );
    }

    fn privacy(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui::heading(ui, "Приватность и сеть", 24.0);
        Frame::new()
            .fill(p.ok.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, p.ok.gamma_multiply(0.35)))
            .corner_radius(14)
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(icon::SHIELD_CHECK)
                            .font(ui::icon_font(22.0))
                            .color(p.ok),
                    );
                    ui.label(
                        "Snapling выходит в сеть только для скачивания модели перевода и \
                         DeepL — и только когда ты сам это подтвердил.",
                    );
                });
            });
        let store = &mut self.store;
        card(ui, |ui| {
            let v = &mut self.cfg.privacy.hide_from_capture;
            let title = "Скрывать выделение в демонстрации экрана";
            let hint = "Discord, OBS и Teams не увидят окно выделения.";
            Self::flag(ui, store, title, hint, v, ("privacy", "hide_from_capture"));
        });
    }

    fn about(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui::draw_logo(ui, 88.0);
            ui.label(RichText::new("Snapling").font(ui::semi(26.0)));
            let version = format!("Версия {} · MIT", snapling_core::VERSION);
            ui.label(RichText::new(version).size(13.0).color(p.muted));
            ui.add_space(4.0);
            ui.set_max_width(440.0);
            ui.label(
                RichText::new(
                    "Основано на идеях и коде EiSen (MIT, © 2026 Tran Trong Thuc). \
                     Шрифты Inter и JetBrains Mono — OFL. Иконки Phosphor — MIT.",
                )
                .size(13.5)
                .color(p.soft),
            );
            ui.add_space(4.0);
            if ui::tone(ui, "Открыть лог", None, 38.0).clicked() {
                (self.hooks.open)(&self.hooks.log);
            }
        });
    }
}

impl eframe::App for Settings {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ui::preview(&ctx);
        self.poll_recording(&ctx);
        if self.recording.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let p = ui::pal(&ctx);
        ui::title_bar(ui, "Snapling — настройки");
        egui::Panel::left("nav")
            .exact_size(236.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.bg).inner_margin(Margin {
                left: 12,
                right: 12,
                top: 8,
                bottom: 16,
            }))
            .show(ui, |ui| self.nav(ui));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin {
                left: 0,
                right: 8,
                top: 0,
                bottom: 8,
            }))
            .show(ui, |ui| {
                Frame::new()
                    .fill(p.main)
                    .stroke(egui::Stroke::new(1.0, p.line))
                    .corner_radius(14)
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            Frame::new()
                                .inner_margin(Margin::symmetric(32, 26))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.spacing_mut().item_spacing.y = 14.0;
                                    match self.tab {
                                        Tab::General => self.general(ui),
                                        Tab::Capture => self.capture(ui),
                                        Tab::Hotkeys => self.hotkeys(ui),
                                        Tab::Text => self.text(ui),
                                        Tab::Privacy => self.privacy(ui),
                                        Tab::About => self.about(ui),
                                    }
                                    if !self.store.error.is_empty() {
                                        let e = &self.store.error;
                                        ui.label(RichText::new(e).size(13.0).color(p.warn));
                                    }
                                });
                        });
                    });
            });
    }
}
