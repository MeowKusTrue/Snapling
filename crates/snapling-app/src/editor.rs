//! Annotation editor window (egui). Runs as its own process (`snapling --edit <file>`):
//! winit allows one event loop per process, and closing the editor returns its memory.
//! Drawing is previewed with egui shapes and committed through `annotate` (tiny-skia),
//! so what is saved is what the core renders on every OS.

use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, Key, KeyboardShortcut, Modifiers, Pos2, Rect, Sense,
    Shape, Stroke, StrokeKind, TextureHandle, TextureOptions,
};
use egui_phosphor::regular as icon;
use snapling_core::annotate::Pixmap;
use snapling_core::annotate::{self, Element, History, Kind, Pt};
use std::path::{Path, PathBuf};

/// Platform side: clipboard and the configured save folder.
pub struct Hooks {
    pub copy: fn(&[u8], u32, u32) -> Result<(), String>,
    /// bgra, width, height, source app → saved path.
    pub save: fn(&[u8], u32, u32, &str) -> Result<PathBuf, String>,
    pub warn: fn(&str),
}

const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x1F);
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
const SIZES: [f32; 3] = [2.0, 4.0, 8.0];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
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

const TOOLS: [(Tool, &str, &str, Key); 10] = [
    (Tool::Arrow, icon::ARROW_UP_RIGHT, "Стрелка (A)", Key::A),
    (Tool::Line, icon::LINE_SEGMENT, "Линия (L)", Key::L),
    (Tool::Rect, icon::SQUARE, "Прямоугольник (R)", Key::R),
    (Tool::Ellipse, icon::CIRCLE, "Эллипс (O)", Key::O),
    (Tool::Pen, icon::PENCIL_SIMPLE, "Перо (P)", Key::P),
    (Tool::Marker, icon::HIGHLIGHTER, "Маркер (M)", Key::M),
    (Tool::Text, icon::TEXT_T, "Текст (T)", Key::T),
    (Tool::Step, icon::NUMBER_CIRCLE_ONE, "Нумерация (N)", Key::N),
    (
        Tool::Pixelate,
        icon::CHECKERBOARD,
        "Скрыть пикселями (B)",
        Key::B,
    ),
    (Tool::Crop, icon::CROP, "Обрезка (C)", Key::C),
];

pub fn run(file: &Path, hooks: Hooks) {
    let raw = match annotate::read_raw(file) {
        Ok(r) => r,
        Err(e) => return (hooks.warn)(&format!("Не удалось открыть снимок.\n{e}")),
    };
    let _ = std::fs::remove_file(file);
    let Some(base) = annotate::pixmap_from_bgra(&raw.bgra, raw.width, raw.height) else {
        return (hooks.warn)("Не удалось открыть снимок: неверный размер.");
    };
    let (w, h) = (raw.width as f32, raw.height as f32);
    let big = w > 1400.0 || h > 850.0;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Snapling — редактор")
        .with_inner_size([w.clamp(640.0, 1400.0), (h + 96.0).clamp(420.0, 950.0)])
        .with_min_inner_size([560.0, 360.0])
        .with_maximized(big)
        .with_icon(egui::IconData {
            rgba: crate::icon::frame_spark(64, [0xFF, 0xFF, 0xFF], [0xFF, 0x8A, 0x1F]),
            width: 64,
            height: 64,
        });
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
            setup_style(&cc.egui_ctx);
            Ok(Box::new(Editor::new(base, raw.app, hooks)))
        }),
    );
    if let Err(e) = res {
        warn(&format!("Редактор не запустился.\n{e}"));
    }
}

pub(crate) fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    ctx.set_visuals(egui::Visuals::dark());
    ctx.all_styles_mut(|s| {
        s.spacing.button_padding = egui::vec2(8.0, 5.0);
        s.spacing.item_spacing = egui::vec2(6.0, 6.0);
        s.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.85);
        s.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        for w in [
            &mut s.visuals.widgets.inactive,
            &mut s.visuals.widgets.hovered,
            &mut s.visuals.widgets.active,
        ] {
            w.corner_radius = CornerRadius::same(8);
        }
    });
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
    drag: Option<Element>,
    /// Text being typed: image position and content.
    typing: Option<(Pt, String)>,
    status: String,
}

impl Editor {
    fn new(base: Pixmap, app: String, hooks: Hooks) -> Self {
        Self {
            base,
            app,
            hooks,
            history: History::default(),
            tex: None,
            dirty: true,
            tool: Tool::Arrow,
            color: COLORS[0],
            size: SIZES[1],
            drag: None,
            typing: None,
            status: String::new(),
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
        self.dirty |= self.history.undo();
    }

    fn redo(&mut self) {
        self.dirty |= self.history.redo();
    }

    fn copy(&mut self) {
        self.commit_text();
        let (bgra, w, h) = annotate::export(&self.base, &self.history.doc);
        self.status = match (self.hooks.copy)(&bgra, w, h) {
            Ok(()) => format!("Скопировано в буфер · {w}×{h}"),
            Err(e) => format!("Не удалось скопировать: {e}"),
        };
    }

    fn save(&mut self) {
        self.commit_text();
        let (bgra, w, h) = annotate::export(&self.base, &self.history.doc);
        // ponytail: encodes on the UI thread (a 4K PNG freezes ~0.3 s); move to a thread if it bothers.
        self.status = match (self.hooks.save)(&bgra, w, h, &self.app) {
            Ok(p) => format!("Сохранено: {}", p.display()),
            Err(e) => format!("Не удалось сохранить: {e}"),
        };
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
                    self.tool = tool;
                }
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tool, glyph, tip, _) in TOOLS {
                let b = ui
                    .selectable_label(self.tool == tool, egui::RichText::new(glyph).size(18.0))
                    .on_hover_text(tip);
                if b.clicked() {
                    self.commit_text();
                    self.tool = tool;
                }
            }
            ui.separator();
            for c in COLORS {
                let (r, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::click());
                let selected = self.color == c;
                ui.painter()
                    .circle_filled(r.center(), if selected { 9.0 } else { 7.0 }, rgb(c));
                if selected || resp.hovered() {
                    ui.painter().circle_stroke(
                        r.center(),
                        10.5,
                        Stroke::new(1.5, Color32::from_gray(220)),
                    );
                }
                if resp.clicked() {
                    self.color = c;
                }
            }
            ui.separator();
            for (i, s) in SIZES.into_iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), Sense::click());
                let fill = if self.size == s {
                    ACCENT
                } else if resp.hovered() {
                    Color32::from_gray(230)
                } else {
                    Color32::from_gray(150)
                };
                ui.painter()
                    .circle_filled(r.center(), 2.5 + i as f32 * 2.0, fill);
                if resp
                    .on_hover_text(["Тонкая", "Средняя", "Толстая"][i])
                    .clicked()
                {
                    self.size = s;
                }
            }
            ui.separator();
            let undo = ui.add_enabled(
                self.history.can_undo(),
                egui::Button::new(icon::ARROW_COUNTER_CLOCKWISE),
            );
            if undo.on_hover_text("Отменить (Ctrl+Z)").clicked() {
                self.undo();
            }
            let redo = ui.add_enabled(
                self.history.can_redo(),
                egui::Button::new(icon::ARROW_CLOCKWISE),
            );
            if redo.on_hover_text("Повторить (Ctrl+Y)").clicked() {
                self.redo();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = egui::Button::new(
                    egui::RichText::new(format!("{} Сохранить", icon::FLOPPY_DISK))
                        .color(Color32::BLACK),
                )
                .fill(ACCENT);
                if ui.add(save).on_hover_text("Ctrl+S").clicked() {
                    self.save();
                }
                if ui
                    .button(format!("{} Копировать", icon::COPY))
                    .on_hover_text("Ctrl+C")
                    .clicked()
                {
                    self.copy();
                }
            });
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
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
        let avail = ui.available_rect_before_wrap();
        // 1 image pixel = 1 physical pixel at most; shrink to fit otherwise.
        let k = (avail.width() / iw)
            .min(avail.height() / ih)
            .min(1.0 / ui.ctx().pixels_per_point());
        let rect = Rect::from_center_size(avail.center(), egui::vec2(iw * k, ih * k));
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let to_img = |p: Pos2| Pt {
            x: ((p.x - rect.min.x) / k).clamp(0.0, iw),
            y: ((p.y - rect.min.y) / k).clamp(0.0, ih),
        };
        let to_scr = |p: Pt| Pos2::new(rect.min.x + p.x * k, rect.min.y + p.y * k);
        let shift = ui.input(|i| i.modifiers.shift);

        if let Some(t) = &self.tex {
            ui.painter().image(
                t.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(match self.tool {
                Tool::Text => egui::CursorIcon::Text,
                _ => egui::CursorIcon::Crosshair,
            });
        }

        if resp.drag_started() {
            self.commit_text();
            if let Some(start) = ui.input(|i| i.pointer.press_origin()).map(to_img) {
                let kind = match self.tool {
                    Tool::Arrow => Some(Kind::Arrow(start, start)),
                    Tool::Line => Some(Kind::Line(start, start)),
                    Tool::Rect => Some(Kind::Rect(start, start)),
                    Tool::Ellipse => Some(Kind::Ellipse(start, start)),
                    Tool::Pen => Some(Kind::Pen(vec![start])),
                    Tool::Marker => Some(Kind::Marker(vec![start])),
                    Tool::Pixelate | Tool::Crop => Some(Kind::Pixelate(start, start)),
                    Tool::Text | Tool::Step => None,
                };
                self.drag = kind.map(|k| self.el(k));
            }
        }
        if let (Some(el), Some(p)) = (&mut self.drag, resp.interact_pointer_pos()) {
            let p = to_img(p);
            match &mut el.kind {
                Kind::Arrow(a, b) | Kind::Line(a, b) => *b = if shift { snap45(*a, p) } else { p },
                Kind::Rect(a, b) | Kind::Ellipse(a, b) | Kind::Pixelate(a, b) => {
                    *b = if shift { square(*a, p) } else { p }
                }
                Kind::Pen(pts) | Kind::Marker(pts)
                    if pts
                        .last()
                        .is_none_or(|l| (l.x - p.x).hypot(l.y - p.y) > 1.5 / k) =>
                {
                    pts.push(p);
                }
                _ => {}
            }
        }
        if resp.drag_stopped()
            && let Some(el) = self.drag.take()
        {
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
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos().map(to_img)
        {
            match self.tool {
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

        let painter = ui.painter_at(rect);
        if let Some((a, b)) = self.history.doc.crop {
            dim_outside(&painter, rect, Rect::from_two_pos(to_scr(a), to_scr(b)));
        }
        if let Some(el) = &self.drag {
            preview(&painter, el, self.tool, k, to_scr);
        }
        self.text_box(ui, k, to_scr);
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
                    .font(egui::FontId::proportional(px))
                    .text_color(color)
                    .frame(egui::Frame::NONE)
                    .desired_rows(1)
                    .desired_width(width)
                    .hint_text("Текст");
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

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shortcuts(ui.ctx());
        egui::Panel::top("tools")
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .fill(Color32::from_rgb(0x16, 0x1B, 0x26)),
            )
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .fill(Color32::from_rgb(0x16, 0x1B, 0x26)),
            )
            .show(ui, |ui| {
                let hint = "Shift — ровно · Ctrl+Z / Ctrl+Y · клик с обрезкой — сбросить обрезку";
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
            .show(ui, |ui| self.canvas(ui));
    }
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn snap45(a: Pt, p: Pt) -> Pt {
    let (dx, dy) = (p.x - a.x, p.y - a.y);
    let step = std::f32::consts::FRAC_PI_4;
    let ang = (dy.atan2(dx) / step).round() * step;
    let len = dx.hypot(dy);
    Pt {
        x: a.x + len * ang.cos(),
        y: a.y + len * ang.sin(),
    }
}

fn square(a: Pt, p: Pt) -> Pt {
    let s = (p.x - a.x).abs().max((p.y - a.y).abs());
    Pt {
        x: a.x + s * (p.x - a.x).signum(),
        y: a.y + s * (p.y - a.y).signum(),
    }
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
fn preview(painter: &egui::Painter, el: &Element, tool: Tool, k: f32, to_scr: impl Fn(Pt) -> Pos2) {
    let color = rgb(el.color);
    let stroke = Stroke::new(el.size * k, color);
    match &el.kind {
        Kind::Line(a, b) => {
            painter.line_segment([to_scr(*a), to_scr(*b)], stroke);
        }
        Kind::Arrow(a, b) => {
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
                dim_outside(painter, painter.clip_rect(), r);
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
