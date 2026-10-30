//! Settings window, its own process (`snapling --settings`). Every change goes straight into
//! config.toml, comments kept; the tray picks it up by mtime, like a hand edit.

use crate::ui::{self, caption, card, divider, row, segmented, toggle};
use eframe::egui::{self, Align, Color32, Frame, Layout, Margin, RichText, Sense, TextEdit, vec2};
use egui_phosphor::regular as icon;
use snapling_core::config::{
    Action, Config, Engine, Format, Online, RecordFormat, SoundCfg, SoundPack, Theme, TrayIcon,
};
use snapling_core::hotkey::{self, Hotkey};
use snapling_core::i18n;
use snapling_core::i18n::{n, t, tf};
use snapling_core::sound::Sound;
use snapling_core::storage;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

pub struct Hooks {
    pub warn: fn(&str),
    pub autostart: fn() -> bool,
    pub set_autostart: fn(bool) -> Result<(), String>,
    pub pick_folder: fn() -> Option<String>,
    /// Catches the keyboard while a combo is recorded.
    pub record_keys: fn(bool),
    pub recorded_combo: fn() -> Option<String>,
    pub hotkey_free: fn(&Hotkey) -> bool,
    pub autostart_blocked: fn() -> bool,
    /// Windows gives PrtScn to its own screen capture.
    pub snipping_owns: fn() -> bool,
    pub free_printscreen: fn() -> Result<(), String>,
    pub models: PathBuf,
    /// An installed copy can replace itself; portable and Unix ones only show the page.
    pub can_update: bool,
    pub ask: fn(&str) -> bool,
    /// Starts the downloaded installer and closes Snapling.
    pub run_update: fn(&Path) -> Result<(), String>,
    /// Proxy of online translation, kept in Credential Manager: it may hold a password.
    pub get_proxy: fn() -> Option<String>,
    pub set_proxy: fn(&str) -> Result<(), String>,
    pub reserved: fn(&Hotkey) -> Option<&'static str>,
    pub open: fn(&Path),
    pub log: PathBuf,
    /// Folder used when `[save] dir` is empty.
    pub default_dir: fn() -> Option<PathBuf>,
    pub pick_wav: fn() -> Option<String>,
    pub pick_toml: fn() -> Option<String>,
    pub save_toml: fn() -> Option<PathBuf>,
    /// Plays one sound as these settings would.
    pub preview: fn(&SoundCfg, Sound) -> Result<(), String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    General,
    Capture,
    Hotkeys,
    Text,
    Models,
    Privacy,
    About,
}

const TABS: [(Tab, &str, &str); 7] = [
    (Tab::General, icon::GEAR_SIX, n("Основные")),
    (Tab::Capture, icon::CORNERS_OUT, n("Снимки")),
    (Tab::Hotkeys, icon::KEYBOARD, n("Горячие клавиши")),
    (Tab::Text, icon::TRANSLATE, n("Текст и перевод")),
    (Tab::Models, icon::DOWNLOAD_SIMPLE, n("Модели")),
    (Tab::Privacy, icon::SHIELD_CHECK, n("Приватность и сеть")),
    (Tab::About, icon::INFO, n("О программе")),
];

const RECORD_LIMIT: std::time::Duration = std::time::Duration::from_secs(20);

/// `welcome`: the first-run screen instead of the tabs.
pub fn run(hooks: Hooks, welcome: bool) {
    let path = ui::config_path();
    let cfg = match Config::load_or_create(&path) {
        Ok(c) => c,
        Err(e) => return (hooks.warn)(&tf("Ошибка в config.toml.\n{e}", &[("e", &e)])),
    };
    let options = eframe::NativeOptions {
        viewport: if welcome {
            ui::viewport("Snapling", [600.0, 640.0], [480.0, 480.0])
        } else {
            ui::viewport(t("Snapling — настройки"), [1040.0, 700.0], [760.0, 480.0])
        },
        ..Default::default()
    };
    let warn = hooks.warn;
    let res = eframe::run_native(
        "Snapling settings",
        options,
        Box::new(move |cc| {
            ui::init(cc);
            Ok(Box::new(Settings::new(path, cfg, hooks, welcome)))
        }),
    );
    if let Err(e) = res {
        warn(&tf("Окно настроек не запустилось.\n{e}", &[("e", &e)]));
    }
}

/// `SNAP_TAB=<0-6>` opens that tab in debug builds, for `ui::preview`.
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
    welcome: bool,
    autostart: bool,
    recording: Option<(Action, std::time::Instant)>,
    /// Why the last pressed combo was refused.
    refused: Option<(Action, String)>,
    proxy: String,
    /// Row of the model being downloaded and its result.
    fetching: Option<(String, Receiver<Result<(), String>>)>,
    /// Update check under way and what it said.
    update: Option<Receiver<UpdateReply>>,
    update_text: String,
    update_page: Option<String>,
    /// Installer to fetch, and the running download.
    update_setup: Option<(String, String)>,
    installing: Option<Receiver<Result<PathBuf, String>>>,
}

type UpdateReply = Result<Option<snapling_translate::Release>, String>;

impl Settings {
    fn new(path: PathBuf, cfg: Config, hooks: Hooks, welcome: bool) -> Self {
        Self {
            cfg,
            store: Store {
                path,
                error: String::new(),
            },
            autostart: (hooks.autostart)(),
            proxy: (hooks.get_proxy)().unwrap_or_default(),
            hooks,
            tab: first_tab(),
            welcome,
            recording: None,
            refused: None,
            fetching: None,
            update: None,
            update_text: String::new(),
            update_page: None,
            update_setup: None,
            installing: None,
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
                t(label),
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

    fn autostart_row(ui: &mut egui::Ui, hooks: &Hooks, on: &mut bool, store: &mut Store) {
        let hint = if (hooks.autostart_blocked)() {
            t("Выключено в диспетчере задач. Этот переключатель вернёт автозапуск.")
        } else {
            ""
        };
        row(
            ui,
            t("Запускать при входе в систему"),
            hint,
            |ui| {
                if toggle(ui, on).changed()
                    && let Err(e) = (hooks.set_autostart)(*on)
                {
                    store.error = e;
                    *on = (hooks.autostart)();
                }
            },
        );
    }

    fn language_row(ui: &mut egui::Ui, now: &mut String, store: &mut Store) {
        row(ui, t("Язык"), "", |ui| {
            let dir = ui::language_dir();
            let mut all = vec![("auto".to_string(), t("Как в системе").to_string())];
            all.extend(i18n::languages(&dir));
            let shown = all
                .iter()
                .find(|l| l.0 == *now)
                .map_or("", |l| l.1.as_str());
            let mut picked = None;
            egui::ComboBox::from_id_salt("language")
                .selected_text(shown)
                .width(190.0)
                .show_ui(ui, |ui| {
                    for (code, name) in &all {
                        if ui.selectable_label(code == now, name).clicked() {
                            picked = Some(code.clone());
                        }
                    }
                });
            if let Some(code) = picked {
                ui::apply_language(&code);
                store.set("ui", "language", code.clone());
                *now = code;
            }
        });
    }

    fn general(&mut self, ui: &mut egui::Ui) {
        let store = &mut self.store;
        ui::heading(ui, t("Основные"), 24.0);
        card(ui, |ui| {
            Self::autostart_row(ui, &self.hooks, &mut self.autostart, store);
            divider(ui);
            row(ui, t("Тема"), "", |ui| {
                let now = match self.cfg.ui.theme {
                    Theme::System => 0,
                    Theme::Dark => 1,
                    Theme::Light => 2,
                    Theme::Black => 3,
                };
                let items = [t("Системная"), t("Тёмная"), t("Светлая"), t("Чёрная")];
                if let Some(i) = segmented(ui, &items, now, 30.0) {
                    let (t, key) = [
                        (Theme::System, "system"),
                        (Theme::Dark, "dark"),
                        (Theme::Light, "light"),
                        (Theme::Black, "black"),
                    ][i];
                    self.cfg.ui.theme = t;
                    store.set("ui", "theme", key);
                    ui::set_theme(ui.ctx(), t);
                }
            });
        });
        card(ui, |ui| {
            Self::language_row(ui, &mut self.cfg.ui.language, store);
            divider(ui);
            row(ui, t("Значок в трее"), "", |ui| {
                let modes = [
                    (TrayIcon::Auto, "auto"),
                    (TrayIcon::White, "white"),
                    (TrayIcon::Black, "black"),
                ];
                let mode = &mut self.cfg.ui.tray_icon;
                let now = modes.iter().position(|m| m.0 == *mode).unwrap_or(0);
                let items = [t("Авто"), t("Белый"), t("Чёрный")];
                if let Some(i) = segmented(ui, &items, now, 30.0) {
                    *mode = modes[i].0;
                    store.set("ui", "tray_icon", modes[i].1);
                }
            });
        });
        let store = &mut self.store;
        let mut imported = None;
        card(ui, |ui| {
            let hint = t("Ключи API в файл не попадают.");
            row(ui, t("Файл настроек"), hint, |ui| {
                if ui::tone(ui, t("Импорт…"), None, 30.0).clicked()
                    && let Some(file) = (self.hooks.pick_toml)()
                {
                    let file = Path::new(&file);
                    let done = std::fs::read_to_string(file)
                        .map_err(|e| e.to_string())
                        .and_then(|text| {
                            let cfg: Config = toml::from_str(&text).map_err(|e| e.to_string())?;
                            std::fs::write(&store.path, text).map_err(|e| e.to_string())?;
                            Ok(cfg)
                        });
                    match done {
                        Ok(cfg) => imported = Some(cfg),
                        Err(e) => store.error = tf("Файл не подошёл: {e}", &[("e", &e)]),
                    }
                }
                if ui::tone(ui, t("Экспорт…"), None, 30.0).clicked()
                    && let Some(to) = (self.hooks.save_toml)()
                    && let Err(e) = std::fs::copy(&store.path, to)
                {
                    store.error = e.to_string();
                }
            });
        });
        if let Some(cfg) = imported {
            ui::apply_language(&cfg.ui.language);
            ui::set_theme(ui.ctx(), cfg.ui.theme);
            self.cfg = cfg;
        }
        let store = &mut self.store;
        caption(ui, t("Отклик"));
        card(ui, |ui| {
            let title = t("Уведомление после снимка");
            let key = ("capture", "toast");
            Self::flag(ui, store, title, "", &mut self.cfg.capture.toast, key);
            divider(ui);
            let s = &mut self.cfg.sound;
            Self::flag(
                ui,
                store,
                t("Звуки"),
                "",
                &mut s.enabled,
                ("sound", "enabled"),
            );
            divider(ui);
            row(ui, t("Набор звуков"), "", |ui| {
                let packs = [
                    (SoundPack::Soft, "soft"),
                    (SoundPack::Real, "real"),
                    (SoundPack::System, "system"),
                ];
                let now = packs.iter().position(|p| p.0 == s.pack).unwrap_or(0);
                let items = [t("Мягкий"), t("Реалистичный"), t("Системный")];
                if let Some(i) = segmented(ui, &items, now, 30.0) {
                    s.pack = packs[i].0;
                    store.set("sound", "pack", packs[i].1);
                    store.error = (self.hooks.preview)(s, Sound::Capture)
                        .err()
                        .unwrap_or_default();
                }
            });
            divider(ui);
            row(ui, t("Громкость"), "", |ui| {
                ui.spacing_mut().slider_width = 200.0;
                let r = ui.add(egui::Slider::new(&mut s.volume, 0..=100).show_value(false));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    store.set("sound", "volume", i64::from(s.volume));
                }
            });
            divider(ui);
            let title = t("Без звука в играх на весь экран");
            let key = ("sound", "mute_in_games");
            Self::flag(ui, store, title, "", &mut s.mute_in_games, key);
        });
        caption(ui, t("Свои звуки, файлы WAV"));
        let events = [
            (Sound::Capture, t("Снимок"), "capture_file"),
            (Sound::Copy, t("Копирование"), "copy_file"),
            (Sound::Error, t("Ошибка"), "error_file"),
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
                    .map_or(t("Из набора").to_string(), |n| {
                        n.to_string_lossy().into()
                    });
                let (mut clear, mut pick, mut play) = (false, false, false);
                let has_file = !file.is_empty();
                row(ui, title, &name, |ui| {
                    if has_file {
                        clear = ui::ghost_icon(ui, icon::X, 34.0, t("Вернуть звук из набора"))
                            .clicked();
                    }
                    pick = ui::tone(ui, t("Выбрать…"), None, 34.0).clicked();
                    play = ui::ghost_icon(ui, icon::PLAY, 34.0, t("Послушать")).clicked();
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
            None => Some(t("Папка «Изображения» не найдена").to_string()),
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
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui::tone(ui, t("Открыть"), None, 34.0).clicked()
                                && let Some(d) = &resolved
                            {
                                (self.hooks.open)(d);
                            }
                            if ui::tone(ui, t("Выбрать…"), None, 34.0).clicked()
                                && let Some(d) = (self.hooks.pick_folder)()
                            {
                                self.cfg.save.dir = d.clone();
                                self.store.set("save", "dir", d);
                            }
                            let center = Layout::left_to_right(Align::Center);
                            ui.with_layout(center, |ui| {
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    let shown = resolved
                                        .as_ref()
                                        .map_or(String::new(), |d| d.display().to_string());
                                    ui::short(ui, RichText::new(shown).font(ui::mono(14.0)));
                                    let (text, color) = match &problem {
                                        Some(e) => (e.as_str(), p.warn),
                                        None => (t("Папка подходит"), p.ok),
                                    };
                                    ui::short(ui, RichText::new(text).size(12.5).color(color));
                                });
                            });
                        });
                    });
                });
        });
    }

    fn capture(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui::heading(ui, t("Снимки"), 24.0);
        caption(ui, t("Папка для сохранения"));
        self.folder_card(ui);
        let store = &mut self.store;
        let save = &mut self.cfg.save;
        card(ui, |ui| {
            row(ui, t("Формат"), "", |ui| {
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
                row(ui, t("Качество JPEG"), "", |ui| {
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
                    ui.label(t("Имя файла"));
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
        caption(ui, t("Размытие кистью"));
        card(ui, |ui| {
            let b = &mut self.cfg.blur;
            row(ui, t("Вид"), "", |ui| {
                let items = [t("Кубики"), t("Гаусс")];
                if let Some(i) = segmented(ui, &items, usize::from(b.gauss), 30.0) {
                    b.gauss = i == 1;
                    store.set("blur", "gauss", b.gauss);
                }
            });
            divider(ui);
            row(ui, t("Сила"), "", |ui| {
                let items = [t("Слабая"), t("Средняя"), t("Сильная")];
                if let Some(i) = segmented(ui, &items, usize::from(b.strength.min(2)), 30.0) {
                    b.strength = i as u8;
                    store.set("blur", "strength", i as i64);
                }
            });
        });
        caption(ui, t("После снимка"));
        card(ui, |ui| {
            let c = &mut self.cfg.capture;
            Self::flag(
                ui,
                store,
                t("Копировать в буфер"),
                "",
                &mut c.copy,
                ("capture", "copy"),
            );
            divider(ui);
            let key = ("save", "auto_save");
            Self::flag(
                ui,
                store,
                t("Сохранять в папку"),
                "",
                &mut save.auto_save,
                key,
            );
            divider(ui);
            let key = ("capture", "open_editor");
            Self::flag(
                ui,
                store,
                t("Открывать редактор"),
                "",
                &mut c.open_editor,
                key,
            );
            divider(ui);
            let key = ("capture", "instant_copy");
            let hint = t("Выделил — скопировано и закрыто, без панели действий.");
            Self::flag(
                ui,
                store,
                t("Сразу копировать"),
                hint,
                &mut c.instant_copy,
                key,
            );
            divider(ui);
            row(
                ui,
                t("Снимок с задержкой"),
                t("Пункт в меню трея и своё сочетание"),
                |ui| {
                    let delays = [3, 5, 10];
                    let now = delays.iter().position(|d| *d == c.delay).unwrap_or(0);
                    let items = [t("3 с"), t("5 с"), t("10 с")];
                    if let Some(i) = segmented(ui, &items, now, 30.0) {
                        c.delay = delays[i];
                        store.set("capture", "delay", i64::from(delays[i]));
                    }
                },
            );
            divider(ui);
            let key = ("capture", "loupe");
            let hint = t("Пока выбираешь область. Клавиша I копирует цвет под курсором.");
            Self::flag(ui, store, t("Лупа у курсора"), hint, &mut c.loupe, key);
        });
    }

    fn stop_recording(&mut self) {
        if self.recording.take().is_some() {
            (self.hooks.record_keys)(false);
        }
    }

    fn start_recording(&mut self, a: Action) {
        self.stop_recording();
        (self.hooks.record_keys)(true);
        self.recording = Some((a, std::time::Instant::now()));
    }

    fn poll_recording(&mut self, ctx: &egui::Context) {
        let Some((action, started)) = self.recording else {
            return;
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        // The whole keyboard is ours while recording, so it never lasts long.
        if !focused || started.elapsed() > RECORD_LIMIT {
            return self.stop_recording();
        }
        let Some(combo) = (self.hooks.recorded_combo)() else {
            return;
        };
        self.stop_recording();
        self.refused = None;
        if combo == "Escape" {
            return;
        }
        let Ok(hk) = Hotkey::parse(&combo) else {
            return;
        };
        let same = Hotkey::parse(self.cfg.hotkeys.combo(action)).is_ok_and(|h| h == hk);
        if same {
            return;
        }
        let why = if let Some(other) = hotkey::taken_by(&self.cfg.hotkeys, action, &hk) {
            Some(tf(
                "{combo} уже на действии «{0}»",
                &[("combo", &combo), ("0", &other.title())],
            ))
        } else if !(self.hooks.hotkey_free)(&hk) {
            Some(tf("{combo} занято другой программой", &[("combo", &combo)]))
        } else {
            None
        };
        if let Some(why) = why {
            self.refused = Some((
                action,
                tf("{why}. Нажми другое, Esc — отмена", &[("why", &why)]),
            ));
            return self.start_recording(action);
        }
        self.cfg.hotkeys.set_combo(action, combo.clone());
        self.store.set("hotkeys", action.key(), combo);
    }

    /// One action with its combo; says whether the combo or its cross was pressed.
    fn hotkey_row(&self, ui: &mut egui::Ui, a: Action, problems: &[String]) -> (bool, bool) {
        let p = ui::pal(ui.ctx());
        let (mut clicked, mut cleared) = (false, false);
        let problem = problems
            .iter()
            .find(|m| m.starts_with(&format!("«{}»", a.title())));
        let recording = self.recording.is_some_and(|r| r.0 == a);
        let combo = self.cfg.hotkeys.combo(a);
        let label = match (recording, combo.is_empty()) {
            (true, _) => t("Нажми сочетание…"),
            (_, true) => t("Не задано"),
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
                            && ui::ghost_icon(ui, icon::X, 30.0, t("Убрать сочетание")).clicked()
                        {
                            cleared = true;
                        }
                        let g =
                            ui.painter()
                                .layout_no_wrap(label.to_string(), ui::mono(13.0), text);
                        let size = vec2(g.size().x + 24.0, 30.0);
                        let (rect, r) = ui.allocate_exact_size(size, Sense::click());
                        ui.painter().rect_filled(rect, 15.0, fill);
                        if recording {
                            ui::outline(ui, rect, 15.0);
                        }
                        ui.painter().galley(rect.center() - g.size() / 2.0, g, text);
                        if r.clicked() {
                            clicked = true;
                        }
                    });
                });
                if let Some((_, why)) = self.refused.as_ref().filter(|r| r.0 == a) {
                    ui.label(RichText::new(why).size(12.5).color(p.warn));
                } else if let Some(m) = problem {
                    let text = m.split_once(": ").map_or(m.as_str(), |(_, t)| t);
                    ui.label(RichText::new(text).size(12.5).color(p.warn));
                }
            });
        (clicked, cleared)
    }

    fn record_card(&mut self, ui: &mut egui::Ui) {
        let store = &mut self.store;
        let rec = &mut self.cfg.record;
        caption(ui, t("Запись экрана"));
        card(ui, |ui| {
            let hint = t("GIF сжимается до 960 пикселей по длинной стороне.");
            row(ui, t("Формат"), hint, |ui| {
                let now = usize::from(rec.format == RecordFormat::Mp4);
                if let Some(i) = segmented(ui, &["GIF", "MP4"], now, 30.0) {
                    rec.format = [RecordFormat::Gif, RecordFormat::Mp4][i];
                    store.set("record", "format", rec.format.extension());
                }
            });
            divider(ui);
            row(ui, t("Кадров в секунду"), "", |ui| {
                let all = [10, 15, 30];
                let now = all.iter().position(|v| *v == rec.fps).unwrap_or(1);
                if let Some(i) = segmented(ui, &["10", "15", "30"], now, 30.0) {
                    rec.fps = all[i];
                    store.set("record", "fps", i64::from(all[i]));
                }
            });
            divider(ui);
            row(ui, t("Останавливать через"), "", |ui| {
                let all = [60, 120, 300];
                let now = all.iter().position(|v| *v == rec.max_seconds).unwrap_or(1);
                let items = [t("1 мин"), t("2 мин"), t("5 мин")];
                if let Some(i) = segmented(ui, &items, now, 30.0) {
                    rec.max_seconds = all[i];
                    store.set("record", "max_seconds", i64::from(all[i]));
                }
            });
        });
    }

    fn hotkey_pressed(&mut self, click: Option<Action>, clear: Option<Action>) {
        if let Some(a) = click {
            self.refused = None;
            self.start_recording(a);
        }
        if let Some(a) = clear {
            self.stop_recording();
            self.cfg.hotkeys.set_combo(a, String::new());
            self.store.set("hotkeys", a.key(), "");
        }
    }

    fn hotkeys(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui::heading(ui, t("Горячие клавиши"), 24.0);
        self.snipping_card(ui);
        if cfg!(not(windows)) {
            ui.label(RichText::new(t(
                "Здесь сочетания задаются в самой системе: привяжи клавишу к команде. Для                  Hyprland: bind = , Print, exec, snapling --region",
            )).color(p.muted));
            for flag in [
                "--region",
                "--screen",
                "--delay",
                "--record",
                "--long",
                "--ocr",
                "--ocr-translate",
            ] {
                ui.label(RichText::new(format!("snapling {flag}")).font(ui::mono(13.0)));
            }
            return;
        }
        let (_, problems) = hotkey::check(&self.cfg.hotkeys, self.hooks.reserved);
        let mut click = None;
        let mut clear = None;
        card(ui, |ui| {
            for (i, a) in Action::ALL.into_iter().enumerate() {
                if i > 0 {
                    divider(ui);
                }
                let (clicked, cleared) = self.hotkey_row(ui, a, &problems);
                if clicked {
                    click = Some(a);
                }
                if cleared {
                    clear = Some(a);
                }
            }
        });
        self.hotkey_pressed(click, clear);
        ui.label(
            RichText::new(t(
                "Нажми на сочетание, затем нужные клавиши: Windows и другие программы на них \
                 не среагируют. Esc — отмена. Какая программа заняла сочетание, Windows не \
                 сообщает — Snapling покажет сам конфликт.",
            ))
            .size(13.0)
            .color(p.muted),
        );
    }

    /// Shown while Windows keeps PrtScn for itself and an action wants it.
    fn snipping_card(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        let wanted = Action::ALL
            .into_iter()
            .any(|a| Hotkey::parse(self.cfg.hotkeys.combo(a)) == Hotkey::parse("PrintScreen"));
        if !wanted || !(self.hooks.snipping_owns)() {
            return;
        }
        Frame::new()
            .fill(p.warn_bg)
            .corner_radius(14)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.label(
                    RichText::new(t(
                        "PrtScn сейчас открывает «Ножницы» Windows, снимок по нему не сработает.",
                    ))
                    .color(p.warn),
                );
                if ui::tone(ui, t("Исправить"), None, 34.0).clicked()
                    && let Err(e) = (self.hooks.free_printscreen)()
                {
                    self.store.error = e;
                }
            });
    }

    /// First run: what matters before the first shot, on one screen.
    fn welcome(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui::heading(ui, t("Snapling уже работает"), 24.0);
        ui.label(
            RichText::new(t(
                "Он живёт в трее, рядом с часами. Проверь три вещи, остальное есть в настройках.",
            ))
            .color(p.soft),
        );
        card(ui, |ui| {
            Self::language_row(ui, &mut self.cfg.ui.language, &mut self.store);
        });
        let a = Action::CaptureRegion;
        let (_, problems) = hotkey::check(&self.cfg.hotkeys, self.hooks.reserved);
        caption(ui, t("Сочетание для снимка"));
        let (clicked, cleared) = card(ui, |ui| self.hotkey_row(ui, a, &problems));
        self.hotkey_pressed(clicked.then_some(a), cleared.then_some(a));
        self.snipping_card(ui);
        caption(ui, t("Папка снимков"));
        self.folder_card(ui);
        card(ui, |ui| {
            Self::autostart_row(ui, &self.hooks, &mut self.autostart, &mut self.store);
        });
        if ui::primary(ui, t("Готово"), Some(ui.available_width()), 42.0).clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn text(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui::heading(ui, t("Текст и перевод"), 24.0);
        let store = &mut self.store;
        card(ui, |ui| {
            if cfg!(windows) {
                let hint = t("PaddleOCR медленнее, модель ~11 МБ скачивается с вопросом.");
                row(ui, t("Распознавание"), hint, |ui| {
                    let e = &mut self.cfg.ocr.engine;
                    let items = [Engine::Windows.name(), Engine::Paddle.name()];
                    if let Some(i) = segmented(ui, &items, usize::from(*e == Engine::Paddle), 30.0)
                    {
                        *e = [Engine::Windows, Engine::Paddle][i];
                        store.set("ocr", "engine", ["windows", "paddle"][i]);
                    }
                });
                divider(ui);
            }
            let v = &mut self.cfg.ocr.panel;
            let title = t("Показывать окно с текстом");
            let hint = t("После распознавания. Текст в любом случае попадает в буфер.");
            Self::flag(ui, store, title, hint, v, ("ocr", "panel"));
            divider(ui);
            row(ui, t("Языки текста"), "", |ui| {
                let lang = &mut self.cfg.ocr.language;
                let now = match lang.as_str() {
                    "ru" => 1,
                    "en" => 2,
                    _ => 0,
                };
                let items = [t("Как в Windows"), t("Русский"), t("Английский")];
                if let Some(i) = segmented(ui, &items, now, 30.0) {
                    *lang = ["auto", "ru", "en"][i].to_string();
                    store.set("ocr", "language", lang.clone());
                }
            });
        });
        let online = self.cfg.translate.online;
        let by_button = t(
            "Появится кнопка в окне перевода. Текст уходит только по ней, первый раз \
                     Snapling спросит.",
        );
        let paid = t(
            "Платно, свой API-ключ. Текст уходит только по кнопке, первый раз Snapling \
                     спросит.",
        );
        let opts = [
            (
                Online::Off,
                t("На устройстве"),
                t("OPUS-MT, без интернета. Модели ru↔en по ~117 МБ, скачиваются с вопросом."),
                p.soft,
                "off",
            ),
            (
                Online::Deepl,
                t("DeepL, онлайн"),
                by_button,
                p.warn,
                "deepl",
            ),
            (Online::Yandex, t("Яндекс, онлайн"), paid, p.warn, "yandex"),
            (Online::Google, t("Google, онлайн"), paid, p.warn, "google"),
        ];
        for pair in opts.chunks(2) {
            ui.columns(2, |cols| {
                for (ui, &(v, title, note, color, key)) in cols.iter_mut().zip(pair) {
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
                            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                                ui.label(RichText::new(note).size(12.5).color(color));
                            });
                        })
                        .response;
                    if r.interact(Sense::click()).clicked() && !on {
                        self.cfg.translate.online = v;
                        store.set("translate", "online", key);
                    }
                }
            });
        }
        ui.label(
            RichText::new(t(
                "Ключ вводится в окне перевода. Хранится в диспетчере учётных данных Windows, \
                 не попадает в логи и config.",
            ))
            .size(13.0)
            .color(p.muted),
        );
        {
            caption(ui, t("Прокси"));
            let r = ui.add(
                TextEdit::singleline(&mut self.proxy)
                    .font(ui::mono(13.0))
                    .password(true)
                    .hint_text("socks5://127.0.0.1:1080")
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(12, 9)),
            );
            if r.lost_focus() {
                self.store.error = (self.hooks.set_proxy)(self.proxy.trim())
                    .err()
                    .unwrap_or_default();
            }
            ui.label(
                RichText::new(t(
                    "Для онлайн-перевода и скачивания моделей: http, https, socks4, socks5, можно \
                     с логином. Хранится там же, где ключ. Пусто — без прокси.",
                ))
                .size(13.0)
                .color(p.muted),
            );
        }
    }

    fn privacy(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui::heading(ui, t("Приватность и сеть"), 24.0);
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
                    ui.label(t(
                        "Snapling выходит в сеть только для скачивания моделей, онлайн-перевода и \
                         проверки обновлений — и только по твоей кнопке.",
                    ));
                });
            });
        let store = &mut self.store;
        card(ui, |ui| {
            let v = &mut self.cfg.privacy.hide_from_capture;
            row(
                ui,
                t("Хранить записи галереи"),
                "",
                |ui| {
                    let days = [0, 30, 90];
                    let keep = &mut self.cfg.history.keep_days;
                    let now = days.iter().position(|d| d == keep).unwrap_or(0);
                    let items = [t("Всегда"), t("30 дней"), t("90 дней")];
                    if let Some(i) = segmented(ui, &items, now, 30.0) {
                        *keep = days[i];
                        store.set("history", "keep_days", i64::from(days[i]));
                    }
                },
            );
            if self.cfg.history.keep_days > 0 {
                divider(ui);
                let title = t("Удалять и файлы снимков");
                let hint =
                    t("Когда срок вышел, файл уходит в корзину. Иначе он остаётся на диске.");
                let v = &mut self.cfg.history.delete_files;
                Self::flag(ui, store, title, hint, v, ("history", "delete_files"));
            }
            divider(ui);
            let title = t("Скрывать выделение в демонстрации экрана");
            let hint = t("Discord, OBS и Teams не увидят окно выделения.");
            Self::flag(ui, store, title, hint, v, ("privacy", "hide_from_capture"));
        });
    }

    fn about(&mut self, ui: &mut egui::Ui) {
        let p = ui::pal(ui.ctx());
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui::draw_logo(ui, 88.0);
            ui.label(RichText::new("Snapling").font(ui::semi(26.0)));
            let version = tf("Версия {0} · MIT", &[("0", &snapling_core::VERSION)]);
            ui.label(RichText::new(version).size(13.0).color(p.muted));
            ui.add_space(4.0);
            ui.set_max_width(440.0);
            ui.label(
                RichText::new(t(
                    "Основано на идеях и коде EiSen (MIT, © 2026 Tran Trong Thuc). \
                     Шрифты Inter и JetBrains Mono — OFL. Иконки Phosphor — MIT.",
                ))
                .size(13.5)
                .color(p.soft),
            );
            ui.add_space(4.0);
            if ui::tone(ui, t("Открыть лог"), None, 38.0).clicked() {
                (self.hooks.open)(&self.hooks.log);
            }
            let busy = self.update.is_some();
            let label = if busy {
                t("Проверяю…")
            } else {
                t("Проверить обновления")
            };
            if ui
                .add_enabled_ui(!busy, |ui| ui::tone(ui, label, None, 38.0))
                .inner
                .clicked()
            {
                let (tx, rx) = channel();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let _ = tx.send(snapling_translate::latest_release());
                    ctx.request_repaint();
                });
                self.update = Some(rx);
                self.update_text.clear();
                self.update_page = None;
            }
            if !self.update_text.is_empty() {
                ui.label(RichText::new(&self.update_text).size(13.0).color(p.muted));
            }
            if self.hooks.can_update
                && let Some(setup) = self.update_setup.clone()
                && self.installing.is_none()
                && ui::primary(ui, t("Скачать и установить"), None, 38.0).clicked()
                && (self.hooks.ask)(t(
                    "Все окна Snapling закроются, версия обновится и он запустится снова. \
                     Несохранённые правки в редакторе пропадут.",
                ))
            {
                let (tx, rx) = channel();
                let ctx = ui.ctx().clone();
                let dir = self.hooks.models.join("update");
                std::thread::spawn(move || {
                    let _ = tx.send(snapling_translate::download_setup(&setup, &dir));
                    ctx.request_repaint();
                });
                self.installing = Some(rx);
                self.update_text = t("Скачиваю…").into();
            }
            if let Some(page) = &self.update_page
                && ui::tone(ui, t("Открыть страницу выпуска"), None, 38.0).clicked()
            {
                (self.hooks.open)(Path::new(page));
            }
        });
    }

    fn poll_jobs(&mut self) {
        if let Some(reply) = self.update.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.update = None;
            self.update_text = match reply {
                Ok(Some(r)) if snapling_translate::is_newer(&r.tag, snapling_core::VERSION) => {
                    self.update_page = r.page.starts_with("https://github.com/").then_some(r.page);
                    self.update_setup = r.setup;
                    tf("Есть версия {0}", &[("0", &r.tag)])
                }
                Ok(Some(_)) => t("Установлена последняя версия").into(),
                Ok(None) => t("Выпусков пока нет").into(),
                Err(e) => e,
            };
        }
        if let Some(done) = self.installing.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.installing = None;
            let run = self.hooks.run_update;
            self.update_text = match done.and_then(|path| run(&path)) {
                Ok(()) => String::new(),
                Err(e) => tf("Не получилось: {e}", &[("e", &e)]),
            };
        }
        if let Some(done) = self.fetching.as_ref().and_then(|f| f.1.try_recv().ok()) {
            self.fetching = None;
            if let Err(e) = done {
                self.store.error = e;
            }
        }
    }

    fn start_fetch(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        work: impl FnOnce(&Path) -> Result<(), String> + Send + 'static,
    ) {
        let (tx, rx) = channel();
        let models = self.hooks.models.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work(&models));
            ctx.request_repaint();
        });
        self.fetching = Some((id.to_string(), rx));
        self.store.error.clear();
    }

    /// Download or Remove of one row, by whether its files are there.
    fn model_button(&self, ui: &mut egui::Ui, id: &str, installed: bool) -> Option<bool> {
        if self.fetching.as_ref().is_some_and(|f| f.0 == id) {
            ui.label(RichText::new(t("Скачиваю…")).color(ui::pal(ui.ctx()).muted));
            return None;
        }
        let label = if installed {
            t("Удалить")
        } else {
            t("Скачать")
        };
        let free = self.fetching.is_none();
        let r = ui.add_enabled_ui(free, |ui| ui::tone(ui, label, None, 30.0));
        r.inner.clicked().then_some(installed)
    }

    fn models_tab(&mut self, ui: &mut egui::Ui) {
        use snapling_translate::{DIRECTIONS, LANGS, Lang, lang_name, paddle};
        let p = ui::pal(ui.ctx());
        ui::heading(ui, t("Модели"), 24.0);
        let root = self.hooks.models.clone();
        let mut action: Option<(Option<Lang>, bool)> = None;
        caption(ui, t("Распознавание"));
        card(ui, |ui| {
            let hint = tf("Офлайн, ~{0} МБ", &[("0", &paddle::DOWNLOAD_MB)]);
            row(ui, "PaddleOCR", &hint, |ui| {
                if let Some(remove) =
                    self.model_button(ui, paddle::FOLDER, paddle::installed(&root))
                {
                    action = Some((None, remove));
                }
            });
        });
        caption(ui, t("Перевод без интернета"));
        card(ui, |ui| {
            let langs = LANGS
                .iter()
                .filter(|l| l.0 != "en")
                .filter(|l| DIRECTIONS.iter().any(|d| d.from == l.0 || d.to == l.0));
            for (i, (code, _)) in langs.enumerate() {
                if i > 0 {
                    divider(ui);
                }
                let dirs: Vec<_> = DIRECTIONS
                    .iter()
                    .filter(|d| d.from == *code || d.to == *code)
                    .collect();
                let mb: u32 = dirs.iter().map(|d| d.mb).sum();
                let hint = if dirs.len() == 2 {
                    tf("С английским в обе стороны, ~{0} МБ", &[("0", &mb)])
                } else {
                    tf("Только на английский, ~{0} МБ", &[("0", &mb)])
                };
                let installed = dirs.iter().all(|d| d.installed(&root));
                row(ui, lang_name(code), &hint, |ui| {
                    if let Some(remove) = self.model_button(ui, code, installed) {
                        action = Some((Some(code), remove));
                    }
                });
            }
        });
        ui.label(
            RichText::new(t(
                "Между двумя языками перевод идёт через английский: нужны модели обоих. \
                 Остальные пары работают только через онлайн-сервис.",
            ))
            .size(13.0)
            .color(p.muted),
        );
        let ctx = ui.ctx().clone();
        match action {
            Some((None, false)) => {
                let via = (self.hooks.get_proxy)();
                self.start_fetch(&ctx, paddle::FOLDER, move |models| {
                    paddle::download(models, via.as_deref())
                });
            }
            Some((None, true)) => {
                let _ = std::fs::remove_dir_all(root.join(paddle::FOLDER));
            }
            Some((Some(code), false)) => {
                let via = (self.hooks.get_proxy)();
                self.start_fetch(&ctx, code, move |models| {
                    DIRECTIONS
                        .iter()
                        .filter(|d| d.from == code || d.to == code)
                        .try_for_each(|d| snapling_translate::download(models, d, via.as_deref()))
                });
            }
            Some((Some(code), true)) => {
                for d in DIRECTIONS.iter().filter(|d| d.from == code || d.to == code) {
                    let _ = std::fs::remove_dir_all(root.join(d.model()));
                }
            }
            None => {}
        }
    }
}

impl eframe::App for Settings {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ui::preview(&ctx);
        self.poll_recording(&ctx);
        self.poll_jobs();
        if self.recording.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let p = ui::pal(&ctx);
        if self.welcome {
            ui::title_bar(ui, "Snapling");
            egui::CentralPanel::default()
                .frame(Frame::new().fill(p.main))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        Frame::new()
                            .inner_margin(Margin {
                                left: 28,
                                right: 28,
                                top: 12,
                                bottom: 24,
                            })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 12.0;
                                self.welcome(ui);
                                if !self.store.error.is_empty() {
                                    let e = &self.store.error;
                                    ui.label(RichText::new(e).size(13.0).color(p.warn));
                                }
                            });
                    });
                });
            return;
        }
        ui::title_bar(ui, t("Snapling — настройки"));
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
                                        Tab::Capture => {
                                            self.capture(ui);
                                            self.record_card(ui);
                                        }
                                        Tab::Hotkeys => self.hotkeys(ui),
                                        Tab::Text => self.text(ui),
                                        Tab::Models => self.models_tab(ui),
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
