use super::*;

/// Decorations over the already blitted frame. `lay` must come from [`layout`] of `sc`.
pub fn draw(pm: &mut PixmapMut<'_>, sc: &Scene<'_>, lay: &Layout) {
    let s = sc.scale;
    let f = fonts();
    let id = Transform::identity();
    if let Some(t) = &sc.tools {
        if let Some(el) = t.live {
            draw_element(pm, el);
        }
        if let Some((at, typed)) = t.caret {
            let el = annotate::Element {
                kind: annotate::Kind::Text(at, format!("{typed}|"), t.style.font),
                color: t.color,
                size: t.size,
            };
            draw_element(pm, &el);
        }
    }
    if let Some(hv) = &sc.hover
        && sc.selection.is_none()
    {
        let rad = if hv.target == Target::Element {
            6.0
        } else {
            12.0
        } * s;
        let mon = sc.monitor;
        if let Some(p) = rr(
            fit(inflate(hv.rect, (4.0 * s) as i32), mon, 2.0 * s),
            rad + 4.0 * s,
        ) {
            pm.stroke_path(
                &p,
                &paint([0xFF, 0x8A, 0x1F, 46]),
                &stroke(4.0 * s),
                id,
                None,
            );
        }
        if let Some(p) = rr(fit(inflate(hv.rect, (1.0 * s) as i32), mon, s), rad + s) {
            pm.stroke_path(&p, &paint(ORANGE), &stroke(2.0 * s), id, None);
        }
    }
    let lasso = sc.tools.as_ref().and_then(|t| t.lasso);
    if let Some(p) = lasso.and_then(annotate::polygon) {
        let c = [255, 255, 255, 178];
        pm.stroke_path(&p, &paint(c), &stroke(1.5 * s), id, None);
    }
    if let Some(sel) = sc.selection.filter(|_| lasso.is_none()) {
        // Design «А»: a hairline, with corners that show where to pull.
        let a = fit(sel, sc.monitor, 1.0 * s);
        if let Some(p) = rr(a, 3.0 * s) {
            pm.stroke_path(&p, &paint([255, 255, 255, 128]), &stroke(s), id, None);
        }
        let (x0, y0) = (a.left as f32, a.top as f32);
        let (x1, y1) = (a.right() as f32, a.bottom() as f32);
        let len = (14.0 * s)
            .min(a.width as f32 / 2.0)
            .min(a.height as f32 / 2.0);
        let rad = (4.0 * s).min(len);
        for (cx, cy, dx, dy) in [
            (x0, y0, 1.0, 1.0),
            (x1, y0, -1.0, 1.0),
            (x0, y1, 1.0, -1.0),
            (x1, y1, -1.0, -1.0),
        ] {
            let mut pb = PathBuilder::new();
            pb.move_to(cx, cy + dy * len);
            pb.line_to(cx, cy + dy * rad);
            pb.quad_to(cx, cy, cx + dx * rad, cy);
            pb.line_to(cx + dx * len, cy);
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &paint([255, 255, 255, 255]), &stroke(2.0 * s), id, None);
            }
        }
    }
    if let Some((pill, label)) = &lay.size_pill {
        glass(pm, *pill, 7.0 * s, GLASS_HINT, 20.0 * s, false);
        let cy = pill.top as f32 + pill.height as f32 / 2.0;
        text(
            pm,
            &f.mono,
            label,
            pill.left as f32 + 8.0 * s,
            cy,
            11.0 * s,
            [0xC9, 0xD2, 0xE3, 255],
        );
    }
    if let (Some(pill), Some(hv)) = (lay.hover_pill, &sc.hover) {
        glass(pm, pill, pill.height as f32 / 2.0, GLASS, 20.0 * s, false);
        let cy = pill.top as f32 + pill.height as f32 / 2.0;
        let tag_w = text_w(&f.semibold, hv.target.label(), 13.0 * s) + 18.0 * s;
        let tag = r(pill.left as f32 + 6.0 * s, cy - 12.0 * s, tag_w, 24.0 * s);
        if let Some(p) = rr(tag, 12.0 * s) {
            pm.fill_path(
                &p,
                &paint([0xFF, 0x8A, 0x1F, 41]),
                FillRule::Winding,
                id,
                None,
            );
        }
        text(
            pm,
            &f.semibold,
            hv.target.label(),
            tag.left as f32 + 9.0 * s,
            cy,
            13.0 * s,
            [0xFF, 0xB7, 0x74, 255],
        );
        text(
            pm,
            &f.regular,
            &hover_text(hv),
            tag.right() as f32 + 8.0 * s,
            cy,
            13.0 * s,
            [0xC9, 0xD2, 0xE3, 255],
        );
    }
    for (panel, rad) in [(lay.toolbar, 14.0), (lay.strip, 16.0), (lay.options, 14.0)] {
        let Some(bar) = panel else { continue };
        glass(pm, bar, rad * s, GLASS, 24.0 * s, true);
        // Top highlight of the mockup's inset shadow.
        let mut pb = PathBuilder::new();
        pb.move_to(bar.left as f32 + 16.0 * s, bar.top as f32 + 1.0);
        pb.line_to(bar.right() as f32 - 16.0 * s, bar.top as f32 + 1.0);
        if let Some(top) = pb.finish() {
            pm.stroke_path(&top, &paint([255, 255, 255, 20]), &stroke(1.0), id, None);
        }
    }
    if let Some(g) = lay.grip {
        let (cx, cy) = (
            g.left as f32 + g.width as f32 / 2.0,
            g.bottom() as f32 - 7.0 * s,
        );
        for i in -1..=1 {
            let dot = r(cx + i as f32 * 5.0 * s - s, cy - s, 2.0 * s, 2.0 * s);
            if let Some(p) = rr(dot, s) {
                pm.fill_path(
                    &p,
                    &paint([255, 255, 255, 110]),
                    FillRule::Winding,
                    id,
                    None,
                );
            }
        }
    }
    for d in &lay.dividers {
        if let Some(p) = rr(*d, 0.0) {
            pm.fill_path(&p, &paint([255, 255, 255, 31]), FillRule::Winding, id, None);
        }
    }
    let tools = sc.tools.as_ref();
    for &(b_rect, c) in &lay.buttons {
        let hovered = sc.hover_button == Some(c);
        let (bx, by, bw, bh) = (
            b_rect.left as f32,
            b_rect.top as f32,
            b_rect.width as f32,
            b_rect.height as f32,
        );
        let (cx, cy) = (bx + bw / 2.0, by + bh / 2.0);
        let selected = tools.is_some_and(|t| match c {
            Control::Tool(x) => t.tool == Some(x),
            Control::Select => t.tool.is_none() && !t.lasso_mode,
            Control::Lasso => t.tool.is_none() && t.lasso_mode,
            Control::Picker => t.picker,
            Control::ColorFormat(i) => t.color_format.index() == i,
            Control::Action(Button::Blur) => t.tool == Some(Tool::Blur),
            Control::Color(i) => t.color == COLORS[i],
            Control::Size(i) => t.size == SIZES[i],
            Control::Gauss(g) => t.blur.gauss == g,
            Control::Strength(i) => t.blur.strength == i,
            Control::Head(i) => t.style.arrow.head == Head::ALL[i],
            Control::Free(f) => t.style.free == f,
            Control::ToStart(b) => t.style.arrow.to_start == b,
            Control::StepStyle(i) => t.style.step == StepStyle::ALL[i],
            _ => false,
        });
        let disabled = tools.is_some_and(|t| {
            (c == Control::Undo && !t.can_undo) || (c == Control::Redo && !t.can_redo)
        });
        if c == Control::Action(Button::Copy) {
            let fill = if hovered {
                [0x37, 0x67, 0xEE, 255]
            } else {
                BLUE
            };
            if let Some(p) = rr(b_rect, 10.0 * s) {
                pm.fill_path(&p, &paint(fill), FillRule::Winding, id, None);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(bx + 10.0 * s, by + 1.0);
            pb.line_to(bx + bw - 10.0 * s, by + 1.0);
            if let Some(top) = pb.finish() {
                pm.stroke_path(&top, &paint([255, 255, 255, 46]), &stroke(1.0), id, None);
            }
            icon(
                pm,
                icon_data(Button::Copy),
                bx + 18.5 * s,
                cy,
                17.0 * s,
                [255; 4],
            );
            text(
                pm,
                &f.semibold,
                t("Копировать"),
                bx + 35.0 * s,
                cy,
                13.5 * s,
                [255; 4],
            );
            continue;
        }
        let filled = matches!(
            c,
            Control::Tool(_)
                | Control::Gauss(_)
                | Control::Select
                | Control::Lasso
                | Control::Picker
                | Control::ColorFormat(_)
                | Control::Action(Button::Blur)
                | Control::Head(_)
                | Control::Free(_)
                | Control::ToStart(_)
                | Control::StepStyle(_)
        );
        let fill = if selected && filled {
            Some(BLUE)
        } else if hovered && !disabled {
            Some([255, 255, 255, 20])
        } else {
            None
        };
        if let Some(fill) = fill
            && let Some(p) = rr(b_rect, 11.0 * s)
        {
            pm.fill_path(&p, &paint(fill), FillRule::Winding, id, None);
        }
        let on = if selected { [255; 4] } else { ICON_MUTED };
        match c {
            Control::Action(b) => {
                let (size, col) = if b == Button::Cancel {
                    (17.0, ICON_MUTED)
                } else {
                    (18.0, ICON)
                };
                icon(pm, icon_data(b), cx, cy, size * s, col);
            }
            Control::Tool(t) => {
                let col = if selected { [255; 4] } else { ICON };
                icon(pm, tool_icon(t), cx, cy, 19.0 * s, col);
            }
            Control::Picker => {
                let col = if selected { [255; 4] } else { ICON };
                icon(pm, &PIPETTE, cx, cy, 19.0 * s, col);
            }
            Control::ColorFormat(i) => {
                let label = ColorFormat::ALL[i].label();
                let col = if selected { [255; 4] } else { ICON };
                let w = text_w(&f.semibold, label, 13.0 * s);
                text(pm, &f.semibold, label, cx - w / 2.0, cy, 13.0 * s, col);
            }
            Control::Select | Control::Lasso => {
                let col = if selected { [255; 4] } else { ICON };
                let d: &[&str] = if c == Control::Select {
                    &SELECT
                } else {
                    &LASSO
                };
                icon(pm, d, cx, cy, 19.0 * s, col);
            }
            Control::Undo | Control::Redo => {
                let col = if disabled {
                    [0xE6, 0xEB, 0xF5, 70]
                } else {
                    ICON
                };
                let d: &[&str] = if c == Control::Undo { &UNDO } else { &REDO };
                icon(pm, d, cx, cy, 19.0 * s, col);
            }
            Control::Color(i) => {
                let [r0, g0, b0] = COLORS[i];
                if let Some(p) = PathBuilder::from_circle(cx, cy, 12.0 * s) {
                    pm.fill_path(&p, &paint([r0, g0, b0, 255]), FillRule::Winding, id, None);
                    pm.stroke_path(&p, &paint([255, 255, 255, 40]), &stroke(1.0), id, None);
                }
                if (selected || hovered)
                    && let Some(p) = PathBuilder::from_circle(cx, cy, 15.0 * s)
                {
                    let a = if selected { 255 } else { 110 };
                    pm.stroke_path(&p, &paint([255, 255, 255, a]), &stroke(1.5 * s), id, None);
                }
            }
            Control::Size(i) => {
                if let Some(p) = PathBuilder::from_circle(cx, cy, (2.5 + i as f32 * 2.0) * s) {
                    pm.fill_path(&p, &paint(on), FillRule::Winding, id, None);
                }
            }
            Control::Strength(i) => {
                for k in 0..=i {
                    let h = (6.0 + 4.0 * f32::from(k)) * s;
                    let x = cx - f32::from(i) * 3.0 * s + f32::from(k) * 6.0 * s;
                    if let Some(p) = rrect(x - 1.5 * s, cy + 7.0 * s - h, 3.0 * s, h, 1.5 * s) {
                        pm.fill_path(&p, &paint(on), FillRule::Winding, id, None);
                    }
                }
            }
            Control::Gauss(g) => {
                let col = if selected { [255; 4] } else { ICON };
                let d: &[&str] = if g { &DROP } else { &CUBES };
                icon(pm, d, cx, cy, 19.0 * s, col);
            }
            Control::Head(_) | Control::Free(_) | Control::ToStart(_) => {
                let col = if selected { [255; 4] } else { ICON };
                let d: &[&str] = match c {
                    Control::Head(0) => &HEAD_SHARP,
                    Control::Head(1) => &HEAD_SOFT,
                    Control::Head(_) => &HEAD_DOUBLE,
                    Control::Free(false) => &SHAPE_LINE,
                    Control::Free(true) => &SHAPE_HAND,
                    Control::ToStart(false) => &TO_END,
                    _ => &TO_START,
                };
                icon(pm, d, cx, cy, 18.0 * s, col);
            }
            Control::StepStyle(i) => {
                let style = StepStyle::ALL[i];
                // Radius 8 at scale 1; the pin's point hangs below, so it sits higher.
                let lift = if style == StepStyle::Pin {
                    3.0 * s
                } else {
                    0.0
                };
                let color = tools.map_or(COLORS[0], |t| t.color);
                draw_element(
                    pm,
                    &annotate::Element {
                        kind: annotate::Kind::Step(
                            Pt {
                                x: cx,
                                y: cy - lift,
                            },
                            1,
                            style,
                        ),
                        color,
                        size: (8.0 * s - 10.0) / 2.5,
                    },
                );
            }
            Control::Font(forward) => {
                let d: &[&str] = if forward { &NEXT } else { &PREV };
                icon(pm, d, cx, cy, 17.0 * s, ICON);
                if forward && let (Some(panel), Some(t)) = (lay.options, tools) {
                    let name = if t.style.font.is_empty() {
                        crate::t("Встроенный")
                    } else {
                        t.style.font
                    };
                    let max = panel.width as f32 - 72.0 * s;
                    let fs = 12.0 * s;
                    let shown = clip_text(&f.semibold, name, max, fs);
                    let w = text_w(&f.semibold, &shown, fs);
                    let mid = panel.left as f32 + panel.width as f32 / 2.0;
                    text(pm, &f.semibold, &shown, mid - w / 2.0, cy, fs, TEXT);
                }
            }
        }
    }
    if let Some(bar) = lay.hint_bar {
        glass(
            pm,
            bar,
            bar.height as f32 / 2.0,
            GLASS_HINT,
            20.0 * s,
            false,
        );
        let fs = 12.5 * s;
        let cy = bar.top as f32 + bar.height as f32 / 2.0;
        let mut x = bar.left as f32 + 16.0 * s;
        for &(k, v) in hints(sc) {
            x += text(pm, &f.semibold, t(k), x, cy, fs, TEXT);
            x += text(pm, &f.regular, &format!(" {}", t(v)), x, cy, fs, ICON_MUTED);
            x += 18.0 * s;
        }
    }
    if let (Some(panel), Some(l)) = (lay.loupe, &sc.loupe) {
        draw_loupe(pm, panel, l, s);
    }
}

/// `name` cut with an ellipsis to fit `max`.
fn clip_text(font: &FontRef<'_>, name: &str, max: f32, px: f32) -> String {
    if text_w(font, name, px) <= max {
        return name.to_string();
    }
    let mut out = String::new();
    for c in name.chars() {
        out.push(c);
        if text_w(font, &format!("{out}…"), px) > max {
            out.pop();
            break;
        }
    }
    out + "…"
}

pub(crate) fn draw_loupe(pm: &mut PixmapMut<'_>, panel: Rect, l: &Loupe<'_>, s: f32) {
    let f = fonts();
    let id = Transform::identity();
    if let Some(p) = rr(panel, 12.0 * s) {
        pm.fill_path(&p, &paint([22, 27, 39, 245]), FillRule::Winding, id, None);
        pm.stroke_path(&p, &paint([255, 255, 255, 40]), &stroke(1.0), id, None);
    }
    let c = loupe_cell(s);
    let (x0, y0) = (panel.left as f32 + 6.0 * s, panel.top as f32 + 6.0 * s);
    let half = LOUPE_CELLS / 2;
    for dy in -half..=half {
        for dx in -half..=half {
            let Some(px) = loupe_px(l, l.at.x + dx, l.at.y + dy) else {
                continue;
            };
            let cell = tiny_skia::Rect::from_xywh(
                x0 + (dx + half) as f32 * c,
                y0 + (dy + half) as f32 * c,
                c,
                c,
            );
            if let Some(cell) = cell {
                pm.fill_rect(cell, &paint([px[2], px[1], px[0], 255]), id, None);
            }
        }
    }
    let mid = half as f32 * c;
    if let Some(p) = rrect(x0 + mid, y0 + mid, c, c, 0.0) {
        pm.stroke_path(&p, &paint([0, 0, 0, 200]), &stroke(3.0), id, None);
        pm.stroke_path(&p, &paint([255, 255, 255, 255]), &stroke(1.0), id, None);
    }
    if let Some(px) = loupe_px(l, l.at.x, l.at.y) {
        let cy = panel.bottom() as f32 - 16.0 * s;
        if let Some(p) = PathBuilder::from_circle(x0 + 8.0 * s, cy, 7.0 * s) {
            pm.fill_path(
                &p,
                &paint([px[2], px[1], px[0], 255]),
                FillRule::Winding,
                id,
                None,
            );
            pm.stroke_path(&p, &paint([255, 255, 255, 90]), &stroke(1.0), id, None);
        }
        let label = color_text(px, l.format);
        let room = panel.right() as f32 - x0 - 30.0 * s;
        let label = fit_text(&f.mono, &label, 11.5 * s, room);
        text(pm, &f.mono, &label, x0 + 22.0 * s, cy, 11.5 * s, TEXT);
    }
}

/// Copies `src` into the buffer inside the outline: the freeform selection stays undimmed.
/// A hard edge, the same one the exported image gets.
pub fn paint_region(pm: &mut PixmapMut<'_>, src: PixmapRef<'_>, pts: &[Pt]) {
    let Some(path) = annotate::polygon(pts) else {
        return;
    };
    let id = Transform::identity();
    let shader = tiny_skia::Pattern::new(
        src,
        tiny_skia::SpreadMode::Pad,
        tiny_skia::FilterQuality::Nearest,
        1.0,
        id,
    );
    let p = Paint {
        shader,
        blend_mode: BlendMode::Source,
        anti_alias: false,
        ..Paint::default()
    };
    pm.fill_path(&path, &p, FillRule::Winding, id, None);
}

pub fn button_at(lay: &Layout, p: Point) -> Option<Control> {
    lay.buttons
        .iter()
        .find(|(r, _)| r.contains(p))
        .map(|(_, c)| *c)
}

/// An annotation in the BGRA buffer: same element with its color channels swapped.
pub fn draw_element(pm: &mut PixmapMut<'_>, e: &annotate::Element) {
    let [r0, g0, b0] = e.color;
    let swapped = annotate::Element {
        color: [b0, g0, r0],
        ..e.clone()
    };
    annotate::draw(pm, &swapped);
}
