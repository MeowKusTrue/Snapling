//! Shared look of the egui windows, from `assets/design/*.dc.html`: tokens, fonts, controls and
//! the borderless window frame (own title bar, drag, resize).

use eframe::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId,
    Frame, Id, Layout, Margin, Pos2, Rect, ResizeDirection, Response, RichText, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Theme, ThemePreference, Ui, Vec2, ViewportBuilder,
    ViewportCommand, Visuals, vec2,
};
use egui_phosphor::regular as icon;
use snapling_core::config::{self, Config};
use snapling_core::i18n::t;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const ACCENT: Color32 = Color32::from_rgb(0x29, 0x5B, 0xE8);
const ACCENT_HOVER: Color32 = Color32::from_rgb(0x3B, 0x6C, 0xF5);
const SWITCH_ON: Color32 = Color32::from_rgb(0x2F, 0x63, 0xF0);
const CLOSE_HOVER: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);
pub const RESIZE_EDGE: f32 = 6.0;

/// Colors of one theme.
#[derive(Clone, Copy)]
pub struct Pal {
    /// Window behind the panels.
    pub bg: Color32,
    /// Content panel.
    pub main: Color32,
    /// Cards and fields.
    pub card: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    /// Icons and quiet text on buttons.
    pub soft: Color32,
    /// Tinted button: fill and text.
    pub tone: Color32,
    pub tone_text: Color32,
    pub ghost: Color32,
    pub well: Color32,
    pub warn: Color32,
    pub warn_bg: Color32,
    pub ok: Color32,
}

const DARK: Pal = Pal {
    bg: Color32::from_rgb(0x10, 0x14, 0x1D),
    main: Color32::from_rgb(0x15, 0x1A, 0x25),
    card: Color32::from_rgb(0x1C, 0x22, 0x30),
    line: Color32::from_rgb(0x2A, 0x30, 0x3D),
    text: Color32::from_rgb(0xF3, 0xF6, 0xFC),
    muted: Color32::from_rgb(0xA3, 0xAE, 0xC2),
    soft: Color32::from_rgb(0xC9, 0xD2, 0xE3),
    tone: Color32::from_rgb(0x1E, 0x2A, 0x4C),
    tone_text: Color32::from_rgb(0xB4, 0xC8, 0xFF),
    ghost: Color32::from_rgb(0x26, 0x2C, 0x39),
    well: Color32::from_rgb(0x0F, 0x13, 0x1B),
    warn: Color32::from_rgb(0xFF, 0xB7, 0x74),
    warn_bg: Color32::from_rgb(0x3A, 0x2A, 0x20),
    ok: Color32::from_rgb(0x5B, 0xD3, 0xA8),
};

const LIGHT: Pal = Pal {
    bg: Color32::from_rgb(0xED, 0xF1, 0xF7),
    main: Color32::from_rgb(0xF7, 0xF9, 0xFC),
    card: Color32::WHITE,
    line: Color32::from_rgb(0xD6, 0xDF, 0xEB),
    text: Color32::from_rgb(0x17, 0x20, 0x33),
    muted: Color32::from_rgb(0x53, 0x61, 0x79),
    soft: Color32::from_rgb(0x3A, 0x47, 0x5E),
    tone: Color32::from_rgb(0xDF, 0xE7, 0xFB),
    tone_text: Color32::from_rgb(0x20, 0x49, 0xC7),
    ghost: Color32::from_rgb(0xE3, 0xE8, 0xF0),
    well: Color32::from_rgb(0xE3, 0xE8, 0xF0),
    warn: Color32::from_rgb(0xA8, 0x5A, 0x00),
    warn_bg: Color32::from_rgb(0xFD, 0xEB, 0xD8),
    ok: Color32::from_rgb(0x17, 0x7A, 0x5B),
};

const BLACK: Pal = Pal {
    bg: Color32::BLACK,
    main: Color32::BLACK,
    card: Color32::from_rgb(0x0C, 0x0C, 0x0E),
    line: Color32::from_rgb(0x24, 0x24, 0x28),
    tone: Color32::from_rgb(0x16, 0x1E, 0x38),
    ghost: Color32::from_rgb(0x1A, 0x1A, 0x1E),
    well: Color32::from_rgb(0x08, 0x08, 0x0A),
    warn_bg: Color32::from_rgb(0x2A, 0x1C, 0x12),
    ..DARK
};

/// `Theme::Black` is egui's dark theme with these colors.
static PURE_BLACK: AtomicBool = AtomicBool::new(false);

pub fn pal(ctx: &egui::Context) -> Pal {
    match ctx.theme() {
        Theme::Dark if PURE_BLACK.load(Ordering::Relaxed) => BLACK,
        Theme::Dark => DARK,
        Theme::Light => LIGHT,
    }
}

/// The user's extra language files.
pub fn language_dir() -> std::path::PathBuf {
    crate::app_dir("APPDATA").join("i18n")
}

/// The family of an installed font for egui. A family added now can be drawn from the next frame
/// on, so the first call answers with the bundled one.
// ponytail: only the font's own glyphs while typing; the saved shape falls back per glyph.
pub fn shot_family(ctx: &egui::Context, family: &str) -> FontFamily {
    use egui::epaint::text::{FontData, FontInsert, FontPriority, InsertFontFamily};
    let bundled = FontFamily::Name("shot".into());
    let Some((bytes, index)) = snapling_core::fonts::data(family) else {
        return bundled;
    };
    let own = FontFamily::Name(family.into());
    let known = ctx.fonts(|f| f.families().contains(&own));
    if known {
        return own;
    }
    let mut data = FontData::from_static(bytes);
    data.index = index;
    ctx.add_font(FontInsert::new(
        family,
        data,
        vec![InsertFontFamily {
            family: own,
            priority: FontPriority::Highest,
        }],
    ));
    ctx.request_repaint();
    bundled
}

pub fn apply_language(code: &str) {
    #[cfg(windows)]
    let system = snapling_win::ui_language();
    #[cfg(not(windows))]
    let system = snapling_unix::ui_language();
    snapling_core::i18n::set(code, system, &language_dir());
}

pub fn config_path() -> std::path::PathBuf {
    crate::app_dir("APPDATA").join("config.toml")
}

pub fn set_theme(ctx: &egui::Context, theme: config::Theme) {
    let black = theme == config::Theme::Black;
    PURE_BLACK.store(black, Ordering::Relaxed);
    ctx.set_visuals_of(
        Theme::Dark,
        visuals(Theme::Dark, if black { BLACK } else { DARK }),
    );
    ctx.set_theme(match theme {
        config::Theme::System => ThemePreference::System,
        config::Theme::Dark | config::Theme::Black => ThemePreference::Dark,
        config::Theme::Light => ThemePreference::Light,
    });
}

fn visuals(theme: Theme, p: Pal) -> Visuals {
    let mut v = match theme {
        Theme::Dark => Visuals::dark(),
        Theme::Light => Visuals::light(),
    };
    v.panel_fill = p.main;
    v.window_fill = p.card;
    v.extreme_bg_color = p.well;
    v.faint_bg_color = p.card;
    v.override_text_color = Some(p.text);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_stroke = Stroke::new(1.0, p.line);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.55);
    v.selection.stroke = Stroke::new(1.5, ACCENT_HOVER);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    for (w, fill) in [
        (&mut v.widgets.inactive, p.ghost),
        (&mut v.widgets.hovered, p.tone),
        (&mut v.widgets.active, p.tone),
        (&mut v.widgets.open, p.tone),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, p.text);
        w.corner_radius = CornerRadius::same(9);
        w.expansion = 0.0;
    }
    v
}

/// Fonts, tokens, saved theme, native corners. Called once per window.
pub fn init(cc: &eframe::CreationContext<'_>) {
    let ctx = &cc.egui_ctx;
    let mut fonts = FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    for (name, bytes) in [
        ("Inter", snapling_scene::FONT_REGULAR),
        ("InterSemi", snapling_scene::FONT_SEMIBOLD),
        ("Mono", snapling_scene::FONT_MONO),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // Icons get a family of their own: Inter has other glyphs at the same private-use codes,
    // and Phosphor has empty Latin letters, so neither can go first for both.
    let list = fonts.families.entry(FontFamily::Proportional).or_default();
    list.retain(|f| f != "phosphor");
    list.insert(0, "Inter".into());
    let fallback = list.clone();
    for (name, first) in [("semi", "InterSemi"), ("mono", "Mono")] {
        let mut l = vec![first.to_string()];
        l.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(name.into()), l);
    }
    fonts
        .families
        .insert(FontFamily::Name("icon".into()), vec!["phosphor".into()]);
    fonts
        .families
        .insert(FontFamily::Name("shot".into()), vec!["Ubuntu-Light".into()]);
    ctx.set_fonts(fonts);
    ctx.set_visuals_of(Theme::Light, visuals(Theme::Light, LIGHT));
    ctx.all_styles_mut(|s| {
        s.spacing.button_padding = vec2(12.0, 7.0);
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.interaction.selectable_labels = false;
        for (t, size) in [
            (egui::TextStyle::Body, 14.0),
            (egui::TextStyle::Button, 14.0),
            (egui::TextStyle::Small, 12.5),
        ] {
            if let Some(f) = s.text_styles.get_mut(&t) {
                f.size = size;
            }
        }
    });
    let cfg = Config::load_or_create(&config_path()).unwrap_or_default();
    set_theme(ctx, cfg.ui.theme);
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = cc.window_handle()
            && let RawWindowHandle::Win32(w) = h.as_raw()
        {
            snapling_win::style_window(w.hwnd.get());
        }
    }
}

/// Debug builds: `SNAP_PREVIEW=<dir>/<name>` saves the window to `<name>.png` after two seconds
/// and exits. Needs no focus and sends no input.
#[cfg(debug_assertions)]
pub fn preview(ctx: &egui::Context) {
    let Some(path) = std::env::var_os("SNAP_PREVIEW") else {
        return;
    };
    ctx.request_repaint();
    let id = Id::new("preview-asked");
    if ctx.input(|i| i.time) > 2.0 && !ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        ctx.data_mut(|d| d.insert_temp(id, true));
        ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
    }
    let shot = ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    });
    if let Some(img) = shot {
        let path = std::path::PathBuf::from(path);
        let bgra: Vec<u8> = img
            .pixels
            .iter()
            .flat_map(|c| [c.b(), c.g(), c.r(), 255])
            .collect();
        let [w, h] = img.size.map(|v| v as u32);
        let dir = path.parent().unwrap_or(std::path::Path::new("."));
        let name = std::path::PathBuf::from(path.file_name().unwrap_or_default());
        let res = snapling_core::storage::save_bgra(
            &bgra,
            w,
            h,
            dir,
            &name,
            snapling_core::config::Format::Png,
            90,
        );
        eprintln!("preview: {res:?}");
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}

#[cfg(not(debug_assertions))]
pub fn preview(_: &egui::Context) {}

pub fn semi(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semi".into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("mono".into()))
}

/// Phosphor glyphs (`egui_phosphor::regular`).
pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icon".into()))
}

/// Borderless window with the Frame Spark icon.
pub fn viewport(title: &str, size: [f32; 2], min: [f32; 2]) -> ViewportBuilder {
    ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_inner_size(size)
        .with_min_inner_size(min)
        .with_icon(egui::IconData {
            rgba: crate::icon::frame_spark(64, [0xFF, 0xFF, 0xFF], [0xFF, 0x8A, 0x1F]),
            width: 64,
            height: 64,
        })
}

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized == Some(true))
}

/// Window edges work no matter what is drawn under the pointer.
fn resize_edges(ctx: &egui::Context) {
    if maximized(ctx) {
        return;
    }
    let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let r = ctx.content_rect();
    let e = RESIZE_EDGE;
    let (w, n) = (p.x < r.min.x + e, p.y < r.min.y + e);
    let (east, s) = (p.x > r.max.x - e, p.y > r.max.y - e);
    let (dir, cursor) = match (w, east, n, s) {
        (true, _, true, _) => (ResizeDirection::NorthWest, CursorIcon::ResizeNwSe),
        (_, true, _, true) => (ResizeDirection::SouthEast, CursorIcon::ResizeNwSe),
        (true, _, _, true) => (ResizeDirection::SouthWest, CursorIcon::ResizeNeSw),
        (_, true, true, _) => (ResizeDirection::NorthEast, CursorIcon::ResizeNeSw),
        (true, ..) => (ResizeDirection::West, CursorIcon::ResizeHorizontal),
        (_, true, ..) => (ResizeDirection::East, CursorIcon::ResizeHorizontal),
        (_, _, true, _) => (ResizeDirection::North, CursorIcon::ResizeVertical),
        (_, _, _, true) => (ResizeDirection::South, CursorIcon::ResizeVertical),
        _ => return,
    };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
    }
}

/// Drag = move the window, double click = maximize. Call before the widgets on top of `rect`.
pub fn drag_area(ui: &mut Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let bar = ui.interact(rect, Id::new("window-drag"), Sense::click_and_drag());
    if bar.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(&ctx)));
    } else if bar.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

fn logo(ctx: &egui::Context) -> TextureHandle {
    let id = Id::new("frame-spark");
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return t;
    }
    let rgba = crate::icon::frame_spark(64, [0x3B, 0x6C, 0xF5], [0xFF, 0x8A, 0x1F]);
    let img = egui::ColorImage::from_rgba_unmultiplied([64, 64], &rgba);
    let tex = ctx.load_texture("frame-spark", img, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

pub fn draw_logo(ui: &mut Ui, size: f32) {
    let tex = logo(ui.ctx());
    ui.image((tex.id(), Vec2::splat(size)));
}

fn caption_button(ui: &mut Ui, glyph: &str, hover: Color32, tip: &str) -> Response {
    let height = ui.available_height();
    let (rect, r) = ui.allocate_exact_size(vec2(46.0, height), Sense::click());
    let p = pal(ui.ctx());
    if r.hovered() {
        ui.painter().rect_filled(rect, 0.0, hover);
    }
    let color = if r.hovered() && hover == CLOSE_HOVER {
        Color32::WHITE
    } else {
        p.soft
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        icon_font(15.0),
        color,
    );
    r.on_hover_text(tip)
}

/// The same three as quiet icon buttons, for a floating top bar.
pub fn window_buttons(ui: &mut Ui, size: f32) {
    let ctx = ui.ctx().clone();
    if ghost_icon(ui, icon::X, size, t("Закрыть")).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let max = maximized(&ctx);
    let (glyph, tip) = if max {
        (icon::CORNERS_IN, t("Восстановить"))
    } else {
        (icon::SQUARE, t("Развернуть"))
    };
    if ghost_icon(ui, glyph, size, tip).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
    if ghost_icon(ui, icon::MINUS, size, t("Свернуть")).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// Minimize, maximize, close, right to left. Works inside a right-to-left layout.
pub fn caption_buttons(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let p = pal(&ctx);
    ui.spacing_mut().item_spacing.x = 0.0;
    if caption_button(ui, icon::X, CLOSE_HOVER, t("Закрыть")).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let max = maximized(&ctx);
    let (glyph, tip) = if max {
        (icon::CORNERS_IN, t("Восстановить"))
    } else {
        (icon::SQUARE, t("Развернуть"))
    };
    if caption_button(ui, glyph, p.ghost, tip).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
    if caption_button(ui, icon::MINUS, p.ghost, t("Свернуть")).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// Window title bar: logo, title, window buttons (settings).
pub fn title_bar(ui: &mut Ui, title: &str) {
    let ctx = ui.ctx().clone();
    resize_edges(&ctx);
    let p = pal(&ctx);
    egui::Panel::top("title-bar")
        .exact_size(44.0)
        .show_separator_line(false)
        .frame(Frame::new().fill(p.bg))
        .show(ui, |ui| {
            drag_area(ui, ui.max_rect());
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add_space(16.0);
                draw_logo(ui, 16.0);
                ui.label(RichText::new(title).size(13.0).color(p.soft));
                ui.with_layout(Layout::right_to_left(Align::Center), caption_buttons);
            });
        });
}

/// For windows whose header is part of the content (history, text): only the resize edges.
pub fn frame_edges(ctx: &egui::Context) {
    resize_edges(ctx);
}

/// Accent button.
pub fn primary(ui: &mut Ui, text: &str, width: Option<f32>, height: f32) -> Response {
    filled(ui, text, width, height, true)
}

/// Blue-tinted secondary button.
pub fn tone(ui: &mut Ui, text: &str, width: Option<f32>, height: f32) -> Response {
    filled(ui, text, width, height, false)
}

fn filled(ui: &mut Ui, text: &str, width: Option<f32>, height: f32, main: bool) -> Response {
    let p = pal(ui.ctx());
    let font = semi(if height < 36.0 { 13.0 } else { 14.0 });
    let color = if main { Color32::WHITE } else { p.tone_text };
    let galley = ui.painter().layout_no_wrap(text.into(), font, color);
    let w = width.unwrap_or(galley.size().x + 28.0);
    let (rect, r) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    let on = ui.is_enabled();
    let base = if main { ACCENT } else { p.tone };
    let fill = match (on, r.hovered()) {
        (false, _) => base.gamma_multiply(0.45),
        (true, true) if main => ACCENT_HOVER,
        (true, true) => p.ghost,
        _ => base,
    };
    // Pressed buttons shrink a little: scale(.97) in the design.
    let t = ui
        .ctx()
        .animate_bool_with_time(r.id, r.is_pointer_button_down_on(), 0.1);
    let rect = rect.shrink2(rect.size() * 0.015 * t);
    ui.painter().rect_filled(rect, 10.0, fill);
    let tint = if on { color } else { color.gamma_multiply(0.5) };
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, tint);
    r
}

/// Text button with no fill until hovered.
/// Quiet button of a given width.
pub fn ghost_text_w(ui: &mut Ui, text: &str, width: f32, height: f32) -> Response {
    let p = pal(ui.ctx());
    let (rect, r) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    ui.painter()
        .rect_filled(rect, 10.0, if r.hovered() { p.line } else { p.ghost });
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::proportional(13.5),
        p.text,
    );
    r
}

pub fn ghost_text(ui: &mut Ui, text: &str, height: f32) -> Response {
    let p = pal(ui.ctx());
    let g = ui
        .painter()
        .layout_no_wrap(text.into(), FontId::proportional(14.0), p.text);
    let (rect, r) = ui.allocate_exact_size(vec2(g.size().x + 28.0, height), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect, 10.0, p.ghost);
    }
    ui.painter()
        .galley(rect.center() - g.size() / 2.0, g, p.text);
    r
}

/// Tool button of a vertical rail: blue when active, dim when disabled.
pub fn rail_button(ui: &mut Ui, glyph: &str, on: bool, enabled: bool, tip: &str) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(40.0), sense);
    let p = pal(ui.ctx());
    if on {
        ui.painter().rect_filled(rect, 11.0, ACCENT);
    } else if r.hovered() && enabled {
        ui.painter().rect_filled(rect, 11.0, p.ghost);
    }
    let color = match (on, enabled) {
        (true, _) => Color32::WHITE,
        (_, true) => p.text,
        _ => p.muted.gamma_multiply(0.5),
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        icon_font(19.0),
        color,
    );
    r.on_hover_text(tip)
}

/// Quiet round-cornered icon button.
pub fn ghost_icon(ui: &mut Ui, glyph: &str, size: f32, tip: &str) -> Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let p = pal(ui.ctx());
    if r.hovered() {
        ui.painter().rect_filled(rect, 10.0, p.ghost);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        icon_font(16.0),
        p.muted,
    );
    r.on_hover_text(tip)
}

/// iOS-style switch, 42×26 with a 20 px knob.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut r) = ui.allocate_exact_size(vec2(42.0, 26.0), Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(r.id, *on);
    let p = pal(ui.ctx());
    let off = p.ghost.lerp_to_gamma(p.muted, 0.35);
    ui.painter()
        .rect_filled(rect, 13.0, off.lerp_to_gamma(SWITCH_ON, t));
    let x = egui::lerp((rect.left() + 13.0)..=(rect.right() - 13.0), t);
    let c = Pos2::new(x, rect.center().y);
    ui.painter()
        .circle_filled(c + vec2(0.0, 1.0), 10.0, Color32::from_black_alpha(70));
    ui.painter().circle_filled(c, 10.0, Color32::WHITE);
    r
}

/// Segmented control. Returns the picked index when it changes.
pub fn segmented(ui: &mut Ui, items: &[&str], selected: usize, height: f32) -> Option<usize> {
    segmented_w(ui, items, selected, height, None)
}

/// Same, stretched to a total `width`.
pub fn segmented_w(
    ui: &mut Ui,
    items: &[&str],
    selected: usize,
    height: f32,
    width: Option<f32>,
) -> Option<usize> {
    let p = pal(ui.ctx());
    let galleys: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let (font, color) = if i == selected {
                (semi(13.0), p.text)
            } else {
                (FontId::proportional(13.0), p.muted)
            };
            ui.painter().layout_no_wrap(item.to_string(), font, color)
        })
        .collect();
    let text: f32 = galleys.iter().map(|g| g.size().x).sum();
    let inner = width.map_or(text + 28.0 * items.len() as f32, |w| w - 6.0);
    let extra = (inner - text) / items.len() as f32;
    let (rect, whole) = ui.allocate_exact_size(vec2(inner + 6.0, height + 6.0), Sense::hover());
    ui.painter().rect_filled(rect, 10.0, p.well);
    let mut picked = None;
    let mut x = rect.left() + 3.0;
    for (i, g) in galleys.into_iter().enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(x, rect.top() + 3.0),
            vec2(g.size().x + extra, height),
        );
        x = cell.right();
        let r = ui.interact(cell, whole.id.with(i), Sense::click());
        if i == selected {
            ui.painter().rect_filled(cell, 8.0, p.ghost);
        }
        let color = if i == selected { p.text } else { p.muted };
        ui.painter()
            .galley(cell.center() - g.size() / 2.0, g, color);
        if r.clicked() && i != selected {
            picked = Some(i);
        }
    }
    picked
}

/// Card with a hairline, like every group of rows in the design.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = pal(ui.ctx());
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

/// A row of a card: text on the left, control on the right.
pub fn row(ui: &mut Ui, title: &str, hint: &str, add: impl FnOnce(&mut Ui)) {
    let p = pal(ui.ctx());
    Frame::new()
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            let height = if hint.is_empty() { 34.0 } else { 40.0 };
            let size = vec2(ui.available_width(), height);
            // Controls first: the text gets what is left and is cut short, not pushed out.
            ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
                add(ui);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if hint.is_empty() {
                        short(ui, RichText::new(title));
                    } else {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            short(ui, RichText::new(title));
                            short(ui, RichText::new(hint).size(12.5).color(p.muted));
                        });
                    }
                });
            });
        });
}

/// One line of text, cut with an ellipsis when it does not fit.
pub fn short(ui: &mut Ui, text: RichText) -> Response {
    ui.add(egui::Label::new(text).truncate())
}

/// Hairline between rows, indented like the design.
pub fn divider(ui: &mut Ui) {
    let p = pal(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        (r.left() + 16.0)..=r.right(),
        r.center().y,
        Stroke::new(1.0, p.line),
    );
}

pub fn heading(ui: &mut Ui, text: &str, size: f32) {
    ui.label(RichText::new(text).font(semi(size)));
}

/// Small grey caption above a card.
pub fn caption(ui: &mut Ui, text: &str) {
    let p = pal(ui.ctx());
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(text).size(13.0).color(p.muted));
    });
}

/// Round icon button that comes alive under the pointer: it turns by `turn` radians and grows
/// by `grow` points.
pub fn live_icon(ui: &mut Ui, glyph: &str, tip: &str, turn: f32, grow: f32) -> Response {
    let (rect, r) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
    let p = pal(ui.ctx());
    let t = ui
        .ctx()
        .animate_bool_with_time(r.id.with("live"), r.hovered(), 0.2);
    let eased = 1.0 - (1.0 - t).powi(3);
    ui.painter()
        .rect_filled(rect, 19.0, p.ghost.gamma_multiply(eased));
    let color = p.soft.lerp_to_gamma(p.text, eased);
    let galley =
        ui.painter()
            .layout_no_wrap(glyph.to_string(), icon_font(20.0 + grow * eased), color);
    let at = rect.center() - galley.size() / 2.0;
    let shape = egui::epaint::TextShape::new(at, galley, color)
        .with_angle_and_anchor(turn * eased, egui::Align2::CENTER_CENTER);
    ui.painter().add(shape);
    r.on_hover_text(tip)
}

/// Bar and words of a running download; keeps the window repainting while it lasts.
pub fn download_bar(ui: &mut Ui, progress: &snapling_translate::Progress) {
    let p = pal(ui.ctx());
    ui.spacing_mut().item_spacing.y = 4.0;
    if let Some(part) = progress.ratio() {
        ui.add(
            egui::ProgressBar::new(part)
                .desired_height(6.0)
                .fill(ACCENT),
        );
    }
    short(ui, RichText::new(progress.text()).size(12.5).color(p.muted));
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(200));
}

/// Placeholder of something still loading: a calm pulse, the frames only while it is on screen.
pub fn skeleton(ui: &Ui, rect: Rect, radius: f32) {
    let ctx = ui.ctx();
    let p = pal(ctx);
    let phase = (ctx.input(|i| i.time) * 2.4).sin() as f32 * 0.5 + 0.5;
    let fill = p.ghost.gamma_multiply(0.45 + 0.55 * phase);
    ui.painter().rect_filled(rect, radius, fill);
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
}

/// Hairline on top of an image or a field.
pub fn outline(ui: &Ui, rect: Rect, radius: f32) {
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, pal(ui.ctx()).line),
        StrokeKind::Inside,
    );
}
