//! Annotation editor window (egui), `assets/design/Editor.dc.html`. Runs as its own process
//! (`snapling --edit <file>`): winit allows one event loop per process, and closing the editor
//! returns its memory. Drawing is previewed with egui shapes and committed through `annotate`
//! (tiny-skia), so what is saved is what the core renders on every OS.

use crate::ui::{self, ACCENT};
use eframe::egui::{
    self, Align2, Color32, ColorImage, Frame, Key, KeyboardShortcut, Margin, Modifiers, Pos2, Rect,
    RichText, Sense, Shape, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2, epaint, vec2,
};
use egui_phosphor::regular as icon;
use snapling_core::annotate::Pixmap;
use snapling_core::annotate::{self, BlurStyle, Element, Head, History, Kind, Pt};
use snapling_core::config::Config;
use snapling_core::i18n::{n, t, tf};
use std::path::{Path, PathBuf};

type SaveAs = fn(&[u8], u32, u32, &str) -> Result<Option<PathBuf>, String>;

/// Platform side: clipboard, the configured save folder, OCR.
pub struct Hooks {
    pub copy: fn(&[u8], u32, u32) -> Result<(), String>,
    /// bgra, width, height, source app → saved path.
    pub save: fn(&[u8], u32, u32, &str) -> Result<PathBuf, String>,
    /// Same, after the native dialog; `None` = cancelled.
    pub save_as: SaveAs,
    /// OCR, then the translation window.
    pub text: fn(&[u8], u32, u32),
    pub warn: fn(&str),
}

const COLORS: [[u8; 3]; 8] = [
    [255, 59, 48],
    [255, 138, 31],
    [255, 204, 0],
    [52, 199, 89],
    [10, 132, 255],
    [175, 82, 222],
    [17, 17, 17],
    [255, 255, 255],
];
const RAIL_H: f32 = 582.0;
const ZOOM_STEP: f32 = 1.25;
const ZOOM_RANGE: (f32, f32) = (0.1, 8.0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Marker,
    Text,
    Step,
    Pixelate,
    Crop,
}

const TOOLS: [(Tool, &str, &str, Key); 11] = [
    (Tool::Select, icon::CURSOR, n("Выбор"), Key::V),
    (Tool::Arrow, icon::ARROW_UP_RIGHT, n("Стрелка"), Key::A),
    (Tool::Line, icon::LINE_SEGMENT, n("Линия"), Key::L),
    (Tool::Rect, icon::SQUARE, n("Прямоугольник"), Key::R),
    (Tool::Ellipse, icon::CIRCLE, n("Эллипс"), Key::O),
    (Tool::Pen, icon::PENCIL_SIMPLE, n("Перо"), Key::P),
    (Tool::Marker, icon::HIGHLIGHTER, n("Маркер"), Key::M),
    (Tool::Text, icon::TEXT_T, n("Текст"), Key::T),
    (Tool::Step, icon::NUMBER_CIRCLE_ONE, n("Нумерация"), Key::N),
    (Tool::Pixelate, icon::CHECKERBOARD, n("Размытие"), Key::B),
    (Tool::Crop, icon::CROP, n("Обрезка"), Key::C),
];

pub fn run(file: &Path, hooks: Hooks) {
    let raw = match annotate::read_raw(file) {
        Ok(r) => r,
        Err(e) => return (hooks.warn)(&tf("Не удалось открыть снимок.\n{e}", &[("e", &e)])),
    };
    let _ = std::fs::remove_file(file);
    let Some(base) = annotate::pixmap_from_bgra(&raw.bgra, raw.width, raw.height) else {
        return (hooks.warn)(t("Не удалось открыть снимок: неверный размер."));
    };
    let (w, h) = (raw.width as f32, raw.height as f32);
    let big = w > 1040.0 || h > 700.0;
    let size = [
        (w + 380.0).clamp(1000.0, 1440.0),
        (h + 180.0).clamp(700.0, 900.0),
    ];
    let mut viewport =
        ui::viewport(t("Snapling — редактор"), size, [1000.0, 700.0]).with_maximized(big);
    viewport.drag_and_drop = Some(false);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let warn = hooks.warn;
    let res = eframe::run_native(
        "Snapling editor",
        options,
        Box::new(move |cc| {
            ui::init(cc);
            Ok(Box::new(Editor::new(
                base,
                raw.app,
                hooks,
                cc.egui_ctx.clone(),
            )))
        }),
    );
    if let Err(e) = res {
        warn(&tf("Редактор не запустился.\n{e}", &[("e", &e)]));
    }
}

struct Editor {
    base: Pixmap,
    app: String,
    hooks: Hooks,
    history: History,
    tex: Option<TextureHandle>,
    dirty: bool,
    tool: Tool,
    color: [u8; 3],
    size: f32,
    head: Head,
    blur: BlurStyle,
    drag: Option<Element>,
    /// Text being typed: image position and content.
    typing: Option<(Pt, String)>,
    /// Element under the «Выбор» tool.
    picked: Option<usize>,
    /// Picked element is being dragged, image position of the last step.
    moving: Option<Pt>,
    /// Display points per image pixel; `None` = fit the window.
    zoom: Option<f32>,
    pan: Vec2,
    status: String,
    /// A save running on its thread.
    saving: Option<std::thread::JoinHandle<Result<PathBuf, String>>>,
    ctx: egui::Context,
}

impl Editor {
    fn new(base: Pixmap, app: String, hooks: Hooks, ctx: egui::Context) -> Self {
        Self {
            base,
            app,
            hooks,
            history: History::default(),
            tex: None,
            dirty: true,
            tool: Tool::Arrow,
            color: COLORS[0],
            size: 5.0,
            head: Head::default(),
            blur: {
                let b = Config::load_or_create(&ui::config_path())
                    .unwrap_or_default()
                    .blur;
                BlurStyle {
                    gauss: b.gauss,
                    strength: b.strength.min(2),
                }
            },
            drag: None,
            typing: None,
            picked: None,
            moving: None,
            zoom: None,
            pan: Vec2::ZERO,
            status: String::new(),
            saving: None,
            ctx,
        }
    }

    fn el(&self, kind: Kind) -> Element {
        Element {
            kind,
            color: self.color,
            size: self.size,
        }
    }

    fn commit_text(&mut self) {
        if let Some((at, text)) = self.typing.take() {
            let el = self.el(Kind::Text(at, text.trim_end().to_string()));
            self.history.push(el);
            self.dirty = true;
        }
    }

    fn undo(&mut self) {
        self.typing = None;
        self.picked = None;
        self.dirty |= self.history.undo();
    }

    fn redo(&mut self) {
        self.picked = None;
        self.dirty |= self.history.redo();
    }

    fn export(&mut self) -> (Vec<u8>, u32, u32) {
        self.commit_text();
        annotate::export(&self.base, &self.history.doc)
    }

    fn copy(&mut self) -> bool {
        let (bgra, w, h) = self.export();
        match (self.hooks.copy)(&bgra, w, h) {
            Ok(()) => {
                self.status = tf("Скопировано в буфер · {w}×{h}", &[("w", &w), ("h", &h)]);
                true
            }
            Err(e) => {
                self.status = tf("Не удалось скопировать: {e}", &[("e", &e)]);
                false
            }
        }
    }

    /// Encodes on its own thread: a 4K PNG takes ~0.3 s. A press during a save is ignored.
    fn save(&mut self) {
        if self.saving.is_some() {
            return;
        }
        let (bgra, w, h) = self.export();
        let (save, app, ctx) = (self.hooks.save, self.app.clone(), self.ctx.clone());
        self.saving = Some(std::thread::spawn(move || {
            let done = save(&bgra, w, h, &app);
            ctx.request_repaint();
            done
        }));
        self.status = t("Сохраняю…").into();
    }

    fn poll_save(&mut self) {
        if self.saving.as_ref().is_some_and(|h| h.is_finished()) {
            self.status = self.save_result();
        }
    }

    /// Waits for the running save: what it says for the status line.
    fn save_result(&mut self) -> String {
        match self.saving.take().map(std::thread::JoinHandle::join) {
            Some(Ok(Ok(p))) => tf("Сохранено: {0}", &[("0", &p.display())]),
            Some(Ok(Err(e))) => tf("Не удалось сохранить: {e}", &[("e", &e)]),
            Some(Err(_)) => tf("Не удалось сохранить: {e}", &[("e", &"panic")]),
            None => String::new(),
        }
    }

    fn save_as(&mut self) {
        let (bgra, w, h) = self.export();
        self.status = match (self.hooks.save_as)(&bgra, w, h, &self.app) {
            Ok(Some(p)) => tf("Сохранено: {0}", &[("0", &p.display())]),
            Ok(None) => String::new(),
            Err(e) => tf("Не удалось сохранить: {e}", &[("e", &e)]),
        };
    }

    fn delete_picked(&mut self) {
        if let Some(i) = self.picked.take() {
            self.history.change(|d| {
                d.elements.remove(i);
            });
            self.dirty = true;
        }
    }

    fn set_tool(&mut self, tool: Tool) {
        self.commit_text();
        self.tool = tool;
        self.picked = None;
    }

    /// The element shown in the inspector, if «Выбор» holds one.
    fn picked_mut(&mut self) -> Option<&mut Element> {
        self.history.doc.elements.get_mut(self.picked?)
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        let cmd_shift = |k| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, k);
        let typing = ctx.memory(|m| m.focused().is_some());
        let (undo, redo, copy, save) = ctx.input_mut(|i| {
            (
                !typing && i.consume_shortcut(&cmd(Key::Z)),
                !typing
                    && (i.consume_shortcut(&cmd(Key::Y)) || i.consume_shortcut(&cmd_shift(Key::Z))),
                !typing && i.consume_shortcut(&cmd(Key::C)),
                i.consume_shortcut(&cmd(Key::S)),
            )
        });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        if copy {
            self.copy();
        }
        if save {
            self.save();
        }
        if !typing {
            for (tool, _, _, key) in TOOLS {
                if ctx.input(|i| i.key_pressed(key) && i.modifiers.is_none()) {
                    self.set_tool(tool);
                }
            }
            let del = ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace));
            if del {
                self.delete_picked();
            }
        }
    }

    /// Floating panel of the design: glass tint, hairline, soft shadow.
    fn panel(ctx: &egui::Context, radius: u8, margin: Margin) -> Frame {
        let p = ui::pal(ctx);
        Frame::new()
            .fill(p.main.gamma_multiply(0.94))
            .stroke(Stroke::new(1.0, p.line))
            .corner_radius(radius)
            .inner_margin(margin)
            .shadow(epaint::Shadow {
                offset: [0, 12],
                blur: 32,
                spread: 0,
                color: Color32::from_black_alpha(90),
            })
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        let p = ui::pal(ctx);
        let width = ctx.content_rect().width() - 32.0;
        let (mut copy, mut save_as, mut text) = (false, false, false);
        let (w, h) = (self.base.width(), self.base.height());
        egui::Area::new(egui::Id::new("top"))
            .anchor(Align2::LEFT_TOP, vec2(16.0, 16.0))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                Self::panel(ctx, 16, Margin::symmetric(0, 0)).show(ui, |ui| {
                    ui.set_width(width);
                    ui.set_height(56.0);
                    ui::drag_area(ui, ui.max_rect());
                    let size = vec2(width, 56.0);
                    let row = egui::Layout::left_to_right(egui::Align::Center);
                    ui.allocate_ui_with_layout(size, row, |ui| {
                        ui.add_space(16.0);
                        ui::draw_logo(ui, 22.0);
                        let right = egui::Layout::right_to_left(egui::Align::Center);
                        ui.with_layout(right, |ui| {
                            ui.add_space(10.0);
                            ui::window_buttons(ui, 38.0);
                            let (d, _) = ui.allocate_exact_size(vec2(9.0, 22.0), Sense::hover());
                            ui.painter()
                                .vline(d.center().x, d.y_range(), Stroke::new(1.0, p.line));
                            copy = ui::primary(ui, t("Копировать и закрыть"), None, 38.0).clicked();
                            save_as = ui::tone(ui, t("Сохранить как…"), None, 38.0).clicked();
                            text = ui::ghost_text(ui, t("Текст и перевод"), 38.0).clicked();
                            let left = egui::Layout::left_to_right(egui::Align::Center);
                            ui.with_layout(left, |ui| {
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    ui.add_space(9.0);
                                    let name = if self.app.is_empty() {
                                        t("Снимок").to_string()
                                    } else {
                                        tf("Снимок · {0}", &[("0", &self.app)])
                                    };
                                    ui::short(ui, RichText::new(name).font(ui::semi(14.0)));
                                    let sub = format!("{w} × {h}");
                                    ui::short(ui, RichText::new(sub).size(12.0).color(p.muted));
                                });
                            });
                        });
                    });
                });
            });
        if copy && self.copy() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if save_as {
            self.save_as();
        }
        if text {
            let (bgra, w, h) = self.export();
            (self.hooks.text)(&bgra, w, h);
            self.status = t("Распознаю текст…").into();
        }
    }

    fn rail(&mut self, ctx: &egui::Context) {
        let p = ui::pal(ctx);
        let top = ((ctx.content_rect().height() - RAIL_H) / 2.0).max(84.0);
        let mut pick = None;
        let (mut undo, mut redo) = (false, false);
        egui::Area::new(egui::Id::new("rail"))
            .fixed_pos(Pos2::new(16.0, top))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                Self::panel(ctx, 16, Margin::same(6)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for (tool, glyph, tip, _) in TOOLS {
                        let name = format!("{} ({})", t(tip), key_name(tool));
                        if ui::rail_button(ui, glyph, self.tool == tool, true, &name).clicked() {
                            pick = Some(tool);
                        }
                    }
                    let (d, _) = ui.allocate_exact_size(vec2(40.0, 9.0), Sense::hover());
                    ui.painter().hline(
                        (d.left() + 6.0)..=(d.right() - 6.0),
                        d.center().y,
                        Stroke::new(1.0, p.line),
                    );
                    undo = ui::rail_button(
                        ui,
                        icon::ARROW_COUNTER_CLOCKWISE,
                        false,
                        self.history.can_undo(),
                        t("Отменить (Ctrl+Z)"),
                    )
                    .clicked();
                    redo = ui::rail_button(
                        ui,
                        icon::ARROW_CLOCKWISE,
                        false,
                        self.history.can_redo(),
                        t("Повторить (Ctrl+Y)"),
                    )
                    .clicked();
                });
            });
        if let Some(t) = pick {
            self.set_tool(t);
        }
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
    }

    fn inspector_title(&self) -> &'static str {
        if let Some(e) = self.picked.and_then(|i| self.history.doc.elements.get(i)) {
            return match e.kind {
                Kind::Arrow(..) => t("Стрелка"),
                Kind::Line(..) => t("Линия"),
                Kind::Rect(..) => t("Прямоугольник"),
                Kind::Ellipse(..) => t("Эллипс"),
                Kind::Pen(..) => t("Перо"),
                Kind::Marker(..) => t("Маркер"),
                Kind::Text(..) => t("Текст"),
                Kind::Step(..) => t("Нумерация"),
                Kind::Pixelate(..) | Kind::Blur(..) => t("Размытие"),
            };
        }
        TOOLS
            .iter()
            .find(|x| x.0 == self.tool)
            .map_or("", |x| t(x.2))
    }

    fn inspector(&mut self, ctx: &egui::Context) {
        let p = ui::pal(ctx);
        let picked = self.picked.and_then(|i| self.history.doc.elements.get(i));
        let arrow = picked.map_or(self.tool == Tool::Arrow, |e| {
            matches!(e.kind, Kind::Arrow(..))
        });
        let blur = picked.map_or(self.tool == Tool::Pixelate, |e| {
            matches!(e.kind, Kind::Blur(..))
        });
        let bare = picked.is_none() && matches!(self.tool, Tool::Select | Tool::Crop);
        let color = picked.map_or(self.color, |e| e.color);
        let mut size = picked.map_or(self.size, |e| e.size);
        let head = match picked.map(|e| &e.kind) {
            Some(Kind::Arrow(_, _, h)) => *h,
            _ => self.head,
        };
        let mut new_color = None;
        let mut size_step = None;
        let mut new_head = None;
        let mut new_blur = None;
        let title = self.inspector_title();
        egui::Area::new(egui::Id::new("inspector"))
            .anchor(Align2::RIGHT_TOP, vec2(-16.0, 88.0))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                Self::panel(ctx, 16, Margin::same(16)).show(ui, |ui| {
                    ui.set_width(216.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(RichText::new(title).font(ui::semi(15.0)));
                    if bare {
                        let hint = if self.tool == Tool::Crop {
                            t("Обведи рамкой. Клик — сбросить обрезку.")
                        } else {
                            t("Кликни по фигуре: двигай, меняй цвет и толщину, Delete — удалить.")
                        };
                        ui.label(RichText::new(hint).size(12.5).color(p.muted));
                        return;
                    }
                    if !blur {
                        ui.add_space(6.0);
                        ui.label(RichText::new(t("Цвет")).size(12.5).color(p.muted));
                        for row in COLORS.chunks(4) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 16.0;
                                for &c in row {
                                    if swatch(ui, c, c == color).clicked() {
                                        new_color = Some(c);
                                    }
                                }
                            });
                        }
                    } else {
                        ui.add_space(6.0);
                        ui.label(RichText::new(t("Вид")).size(12.5).color(p.muted));
                        let items = [t("Кубики"), t("Гаусс")];
                        let now = usize::from(self.blur.gauss);
                        if let Some(i) = ui::segmented_w(ui, &items, now, 30.0, Some(216.0)) {
                            new_blur = Some(BlurStyle {
                                gauss: i == 1,
                                ..self.blur
                            });
                        }
                        ui.label(RichText::new(t("Сила")).size(12.5).color(p.muted));
                        let items = [t("Слабая"), t("Средняя"), t("Сильная")];
                        let now = usize::from(self.blur.strength);
                        if let Some(i) = ui::segmented_w(ui, &items, now, 30.0, Some(216.0)) {
                            new_blur = Some(BlurStyle {
                                strength: i as u8,
                                ..self.blur
                            });
                        }
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new(t("Толщина")).size(12.5).color(p.muted));
                    ui.spacing_mut().slider_width = 216.0;
                    let r = ui.add(egui::Slider::new(&mut size, 1.0..=16.0).show_value(false));
                    if r.drag_started() || r.clicked() {
                        size_step = Some((size, true));
                    } else if r.changed() {
                        size_step = Some((size, false));
                    }
                    if arrow {
                        ui.add_space(6.0);
                        ui.label(RichText::new(t("Наконечник")).size(12.5).color(p.muted));
                        let heads = [Head::Sharp, Head::Soft, Head::Double];
                        let now = heads.iter().position(|h| *h == head).unwrap_or(0);
                        let items = [t("Острый"), t("Мягкий"), t("Двойной")];
                        if let Some(i) = ui::segmented_w(ui, &items, now, 30.0, Some(216.0)) {
                            new_head = Some(heads[i]);
                        }
                    }
                });
            });
        if let Some(c) = new_color {
            self.color = c;
            if let Some(i) = self.picked {
                self.history.change(|d| d.elements[i].color = c);
                self.dirty = true;
            }
        }
        if let Some((v, first)) = size_step {
            self.size = v;
            if self.picked.is_some() && first {
                self.history.snapshot();
            }
            if let Some(e) = self.picked_mut() {
                e.size = v;
                self.dirty = true;
            }
        }
        if let Some(h) = new_head {
            self.head = h;
            if let Some(i) = self.picked {
                self.history.change(|d| {
                    if let Kind::Arrow(_, _, old) = &mut d.elements[i].kind {
                        *old = h;
                    }
                });
                self.dirty = true;
            }
        }
        if let Some(b) = new_blur {
            self.blur = b;
            if let Some(i) = self.picked {
                self.history.change(|d| {
                    if let Kind::Blur(_, old) = &mut d.elements[i].kind {
                        *old = b;
                    }
                });
                self.dirty = true;
            }
        }
    }

    fn zoom_pill(&mut self, ctx: &egui::Context, k: f32) {
        let p = ui::pal(ctx);
        let percent = (k * ctx.pixels_per_point() * 100.0).round();
        let (mut out, mut fit, mut into) = (false, false, false);
        egui::Area::new(egui::Id::new("zoom"))
            .anchor(Align2::LEFT_BOTTOM, vec2(112.0, -24.0))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                Self::panel(ctx, 12, Margin::same(4)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        out = ui::ghost_icon(ui, icon::MINUS, 30.0, t("Уменьшить")).clicked();
                        let label = RichText::new(format!("{percent}%"))
                            .font(ui::mono(13.0))
                            .color(p.soft);
                        let b = egui::Button::new(label)
                            .frame(false)
                            .min_size(vec2(52.0, 30.0));
                        fit = ui.add(b).on_hover_text(t("По размеру окна")).clicked();
                        into = ui::ghost_icon(ui, icon::PLUS, 30.0, t("Увеличить")).clicked();
                    });
                });
            });
        if out || into {
            let step = if into { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
            self.zoom = Some((k * step).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1));
        }
        if fit {
            self.zoom = None;
            self.pan = Vec2::ZERO;
        }
        if !self.status.is_empty() {
            egui::Area::new(egui::Id::new("status"))
                .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -28.0))
                .order(egui::Order::Middle)
                .interactable(false)
                .show(ctx, |ui| {
                    Self::panel(ctx, 12, Margin::symmetric(14, 8)).show(ui, |ui| {
                        let t = RichText::new(&self.status).size(13.0).color(p.soft);
                        ui.add(egui::Label::new(t).extend());
                    });
                });
        }
    }

    /// Fit-to-window scale, never above one image pixel per physical pixel.
    fn fit(&self, view: Rect, ppp: f32) -> f32 {
        let (iw, ih) = (self.base.width() as f32, self.base.height() as f32);
        (view.width() / iw).min(view.height() / ih).min(1.0 / ppp)
    }

    fn canvas(&mut self, ui: &mut egui::Ui) -> f32 {
        let (iw, ih) = (self.base.width() as f32, self.base.height() as f32);
        if self.dirty || self.tex.is_none() {
            let pm = annotate::compose(&self.base, &self.history.doc);
            let img = ColorImage::from_rgba_unmultiplied(
                [pm.width() as usize, pm.height() as usize],
                pm.data(),
            );
            match &mut self.tex {
                Some(t) => t.set(img, TextureOptions::LINEAR),
                None => self.tex = Some(ui.ctx().load_texture("shot", img, TextureOptions::LINEAR)),
            }
            self.dirty = false;
        }
        let all = ui.max_rect();
        let p = ui::pal(ui.ctx());
        let dot = p.text.gamma_multiply(0.07);
        let mut y = all.top() + 10.0;
        while y < all.bottom() {
            let mut x = all.left() + 10.0;
            while x < all.right() {
                ui.painter().circle_filled(Pos2::new(x, y), 1.0, dot);
                x += 20.0;
            }
            y += 20.0;
        }
        // Space the floating panels leave free.
        let view = Rect::from_min_max(
            Pos2::new(all.left() + 88.0, all.top() + 88.0),
            Pos2::new(all.right() - 280.0, all.bottom() - 72.0),
        );
        let ppp = ui.ctx().pixels_per_point();
        let mut k = self.zoom.unwrap_or_else(|| self.fit(view, ppp));

        let back = ui.interact(all, ui.id().with("pan"), Sense::drag());
        let (scroll, pinch, hover, space) = ui.input(|i| {
            (
                i.smooth_scroll_delta.y,
                i.zoom_delta(),
                i.pointer.hover_pos(),
                i.key_down(Key::Space),
            )
        });
        if let Some(at) = hover.filter(|h| all.contains(*h)) {
            let factor = pinch * (scroll * 0.002).exp();
            if factor != 1.0 {
                let next = (k * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
                let center = view.center() + self.pan;
                self.pan += (at - center) * (1.0 - next / k);
                self.zoom = Some(next);
                k = next;
            }
        }
        let panning = space || ui.input(|i| i.pointer.middle_down());
        if back.dragged() || (panning && ui.input(|i| i.pointer.any_down())) {
            self.pan += ui.input(|i| i.pointer.delta());
        }

        let rect = Rect::from_center_size(view.center() + self.pan, vec2(iw * k, ih * k));
        let shadow = epaint::Shadow {
            offset: [0, 30],
            blur: 80,
            spread: 0,
            color: Color32::from_black_alpha(130),
        };
        ui.painter().add(shadow.as_shape(rect, 10));
        let resp = ui.interact(rect, ui.id().with("shot"), Sense::click_and_drag());
        let to_img = |p: Pos2| Pt {
            x: ((p.x - rect.min.x) / k).clamp(0.0, iw),
            y: ((p.y - rect.min.y) / k).clamp(0.0, ih),
        };
        let to_scr = |p: Pt| Pos2::new(rect.min.x + p.x * k, rect.min.y + p.y * k);
        let shift = ui.input(|i| i.modifiers.shift);

        if let Some(t) = &self.tex {
            egui::Image::new((t.id(), rect.size()))
                .corner_radius(10)
                .paint_at(ui, rect);
        }
        if resp.hovered() && !panning {
            ui.ctx().set_cursor_icon(match self.tool {
                Tool::Text => egui::CursorIcon::Text,
                Tool::Select => egui::CursorIcon::Default,
                _ => egui::CursorIcon::Crosshair,
            });
        }

        if resp.drag_started() && !panning {
            self.commit_text();
            if let Some(start) = ui.input(|i| i.pointer.press_origin()).map(to_img) {
                if self.tool == Tool::Select {
                    self.picked = annotate::pick(&self.history.doc, start);
                    if self.picked.is_some() {
                        self.history.snapshot();
                        self.moving = Some(start);
                    }
                }
                let kind = match self.tool {
                    Tool::Arrow => Some(Kind::Arrow(start, start, self.head)),
                    Tool::Line => Some(Kind::Line(start, start)),
                    Tool::Rect => Some(Kind::Rect(start, start)),
                    Tool::Ellipse => Some(Kind::Ellipse(start, start)),
                    Tool::Pen => Some(Kind::Pen(vec![start])),
                    Tool::Marker => Some(Kind::Marker(vec![start])),
                    Tool::Pixelate => Some(Kind::Blur(vec![start], self.blur)),
                    Tool::Crop => Some(Kind::Pixelate(start, start)),
                    Tool::Text | Tool::Step | Tool::Select => None,
                };
                self.drag = kind.map(|k| self.el(k));
            }
        }
        if let (Some(last), Some(at)) = (self.moving, resp.interact_pointer_pos().map(to_img)) {
            if let Some(e) = self.picked_mut() {
                annotate::translate(&mut e.kind, at.x - last.x, at.y - last.y);
            }
            self.moving = Some(at);
            self.dirty = true;
        }
        if let (Some(el), Some(p)) = (&mut self.drag, resp.interact_pointer_pos()) {
            let p = to_img(p);
            annotate::drag_to(&mut el.kind, p, shift, 1.5 / k);
        }
        if resp.drag_stopped() {
            self.moving = None;
            if let Some(el) = self.drag.take() {
                match (self.tool, el.kind) {
                    (Tool::Crop, Kind::Pixelate(a, b)) => {
                        if (a.x - b.x).abs() >= 3.0 && (a.y - b.y).abs() >= 3.0 {
                            self.history.change(|d| d.crop = Some((a, b)));
                        }
                    }
                    (_, kind) => self.history.push(Element { kind, ..el }),
                }
                self.dirty = true;
            }
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos().map(to_img)
        {
            match self.tool {
                Tool::Select => {
                    self.picked = annotate::pick(&self.history.doc, p);
                    if let Some(e) = self.picked_mut() {
                        let (c, s) = (e.color, e.size);
                        self.color = c;
                        self.size = s;
                    }
                }
                Tool::Step => {
                    self.commit_text();
                    let el = self.el(Kind::Step(p, self.history.next_step()));
                    self.history.push(el);
                    self.dirty = true;
                }
                Tool::Text => {
                    self.commit_text();
                    self.typing = Some((p, String::new()));
                }
                Tool::Crop => self.history.change(|d| d.crop = None),
                _ => {}
            }
            if self.tool == Tool::Crop {
                self.dirty = true;
            }
        }

        let painter = ui.painter_at(all);
        if let Some((a, b)) = self.history.doc.crop {
            dim_outside(&painter, rect, Rect::from_two_pos(to_scr(a), to_scr(b)));
        }
        if let Some(el) = &self.drag {
            preview(&painter, el, self.tool, k, to_scr, rect);
        }
        if let Some(e) = self.picked.and_then(|i| self.history.doc.elements.get(i)) {
            let (a, b) = annotate::bounds(e);
            let r = Rect::from_two_pos(to_scr(a), to_scr(b)).expand(3.0);
            let s = Stroke::new(1.5, ACCENT);
            painter.rect_stroke(r, 4.0, s, StrokeKind::Outside);
        }
        self.text_box(ui, k, to_scr);
        k
    }

    fn text_box(&mut self, ui: &mut egui::Ui, k: f32, to_scr: impl Fn(Pt) -> Pos2) {
        let Some((at, text)) = &mut self.typing else {
            return;
        };
        let px = annotate::text_px(self.size) * k;
        let color = rgb(self.color);
        let mut done = false;
        let mut cancel = false;
        egui::Area::new(egui::Id::new("typing"))
            .fixed_pos(to_scr(*at))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let width = annotate::text_size(text, px).0 + px;
                let edit = egui::TextEdit::multiline(text)
                    .font(egui::FontId::new(px, egui::FontFamily::Name("shot".into())))
                    .text_color(color)
                    .frame(egui::Frame::NONE)
                    .desired_rows(1)
                    .desired_width(width)
                    .hint_text(t("Текст"));
                let r = egui::Frame::new()
                    .stroke(Stroke::new(1.0, ACCENT))
                    .corner_radius(4)
                    .show(ui, |ui| ui.add(edit))
                    .inner;
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                ui.input_mut(|i| {
                    if i.consume_key(Modifiers::NONE, Key::Enter) {
                        done = true;
                    }
                    if i.consume_key(Modifiers::NONE, Key::Escape) {
                        cancel = true;
                    }
                });
                if r.lost_focus() {
                    done = true;
                }
            });
        if cancel {
            self.typing = None;
        } else if done {
            self.commit_text();
        }
    }
}

fn key_name(tool: Tool) -> char {
    match tool {
        Tool::Select => 'V',
        Tool::Arrow => 'A',
        Tool::Line => 'L',
        Tool::Rect => 'R',
        Tool::Ellipse => 'O',
        Tool::Pen => 'P',
        Tool::Marker => 'M',
        Tool::Text => 'T',
        Tool::Step => 'N',
        Tool::Pixelate => 'B',
        Tool::Crop => 'C',
    }
}

/// Round color button, 32 px; the selected one gets a ring.
fn swatch(ui: &mut egui::Ui, c: [u8; 3], on: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
    let center = rect.center();
    ui.painter().circle_filled(center, 16.0, rgb(c));
    if on {
        let ring = Stroke::new(2.0, rgb(c));
        ui.painter().circle_stroke(center, 18.5, ring);
    } else if r.hovered() {
        let ring = Stroke::new(1.5, ui::pal(ui.ctx()).soft);
        ui.painter().circle_stroke(center, 18.0, ring);
    }
    r
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui::preview(ui.ctx());
        self.poll_save();
        self.shortcuts(ui.ctx());
        let ctx = ui.ctx().clone();
        ui::frame_edges(&ctx);
        let fill = ui::pal(&ctx).well;
        let mut k = 1.0;
        egui::CentralPanel::default()
            .frame(Frame::new().fill(fill))
            .show(ui, |ui| k = self.canvas(ui));
        self.top_bar(&ctx);
        self.rail(&ctx);
        self.inspector(&ctx);
        self.zoom_pill(&ctx, k);
    }

    /// A save still running would lose its file with the process.
    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        self.save_result();
    }
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn dim_outside(painter: &egui::Painter, all: Rect, keep: Rect) {
    let shade = Color32::from_black_alpha(150);
    for r in [
        Rect::from_min_max(all.min, Pos2::new(all.max.x, keep.min.y)),
        Rect::from_min_max(Pos2::new(all.min.x, keep.max.y), all.max),
        Rect::from_min_max(
            Pos2::new(all.min.x, keep.min.y),
            Pos2::new(keep.min.x, keep.max.y),
        ),
        Rect::from_min_max(
            Pos2::new(keep.max.x, keep.min.y),
            Pos2::new(all.max.x, keep.max.y),
        ),
    ] {
        painter.rect_filled(r, 0.0, shade);
    }
    painter.rect_stroke(
        keep,
        0.0,
        Stroke::new(1.5, Color32::WHITE),
        StrokeKind::Outside,
    );
}

/// Live shape while dragging; the committed one is rendered by `annotate`.
fn preview(
    painter: &egui::Painter,
    el: &Element,
    tool: Tool,
    k: f32,
    to_scr: impl Fn(Pt) -> Pos2,
    image: Rect,
) {
    let color = rgb(el.color);
    let stroke = Stroke::new(el.size * k, color);
    match &el.kind {
        Kind::Line(a, b) => {
            painter.line_segment([to_scr(*a), to_scr(*b)], stroke);
        }
        Kind::Arrow(a, b, _) => {
            let [tip, l, r] = annotate::arrow_head(*a, *b, el.size);
            let base = Pt {
                x: (l.x + r.x) / 2.0,
                y: (l.y + r.y) / 2.0,
            };
            painter.line_segment([to_scr(*a), to_scr(base)], stroke);
            painter.add(Shape::convex_polygon(
                vec![to_scr(tip), to_scr(l), to_scr(r)],
                color,
                Stroke::NONE,
            ));
        }
        Kind::Rect(a, b) => {
            painter.rect_stroke(
                Rect::from_two_pos(to_scr(*a), to_scr(*b)),
                0.0,
                stroke,
                StrokeKind::Middle,
            );
        }
        Kind::Ellipse(a, b) => {
            let r = Rect::from_two_pos(to_scr(*a), to_scr(*b));
            painter.add(Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
        }
        Kind::Pen(pts) => {
            painter.add(Shape::line(
                pts.iter().map(|p| to_scr(*p)).collect(),
                stroke,
            ));
        }
        Kind::Blur(pts, _) => {
            let s = Stroke::new(
                annotate::blur_width(el.size) * k,
                Color32::from_white_alpha(60),
            );
            painter.add(Shape::line(pts.iter().map(|p| to_scr(*p)).collect(), s));
        }
        Kind::Marker(pts) => {
            let s = Stroke::new(
                annotate::marker_width(el.size) * k,
                color.gamma_multiply(0.45),
            );
            painter.add(Shape::line(pts.iter().map(|p| to_scr(*p)).collect(), s));
        }
        Kind::Pixelate(a, b) => {
            let r = Rect::from_two_pos(to_scr(*a), to_scr(*b));
            if tool == Tool::Crop {
                dim_outside(painter, image, r);
            } else {
                painter.rect_filled(r, 0.0, Color32::from_white_alpha(40));
                painter.add(Shape::dashed_line(
                    &[
                        r.left_top(),
                        r.right_top(),
                        r.right_bottom(),
                        r.left_bottom(),
                        r.left_top(),
                    ],
                    Stroke::new(1.0, Color32::WHITE),
                    6.0,
                    4.0,
                ));
            }
        }
        Kind::Text(..) | Kind::Step(..) => {}
    }
}
