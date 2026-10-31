use super::*;

pub fn layout(sc: &Scene<'_>) -> Layout {
    let s = sc.scale;
    let f = fonts();
    let mut out = Layout::default();
    let blur = (24.0 * s).ceil() as i32;
    if let Some(sel) = sc.selection {
        let label = format!("{} × {}", sel.width, sel.height);
        let (w, h) = (text_w(&f.mono, &label, 11.0 * s) + 16.0 * s, 20.0 * s);
        let pill = above_or_inside(sel, sc.monitor, w, h, 8.0 * s, s);
        out.bounds = union(out.bounds, inflate(pill, blur));
        out.size_pill = Some((pill, label));
        out.bounds = union(out.bounds, inflate(sel, (10.0 * s) as i32));
        if sc.confirmed && !sc.dragging {
            let lasso = sc.tools.as_ref().is_some_and(|t| t.lasso.is_some());
            let items: Vec<Option<Button>> = BAR
                .into_iter()
                .filter(|b| {
                    !lasso
                        || !matches!(
                            b,
                            Some(
                                Button::Text | Button::Translate | Button::Record | Button::Scroll
                            )
                        )
                })
                .collect();
            let width = |b: Option<Button>| match b {
                Some(Button::Copy) => primary_w(s),
                Some(_) => BTN * s,
                None => 9.0 * s,
            };
            let tw = 2.0 * BAR_PAD * s
                + items.iter().map(|b| width(*b)).sum::<f32>()
                + (items.len() - 1) as f32 * GAP * s;
            let th = (BTN + 2.0 * BAR_PAD) * s;
            let gap = 14.0 * s;
            let mon = sc.monitor;
            let x = (sel.left as f32)
                .min(mon.right() as f32 - tw - gap)
                .max(mon.left as f32 + gap);
            let below = sel.bottom() as f32 + gap;
            let above = sel.top as f32 - gap - th - 38.0 * s;
            let y = if below + th + gap <= mon.bottom() as f32 {
                below
            } else if above >= mon.top as f32 + gap {
                sel.top as f32 - gap - th
            } else {
                sel.bottom() as f32 - th - gap
            };
            let bar = r(x, y, tw, th);
            let mut bx = x + BAR_PAD * s;
            let by = y + BAR_PAD * s;
            for b in items {
                let w = width(b);
                match b {
                    Some(b) => out
                        .buttons
                        .push((r(bx, by, w, BTN * s), Control::Action(b))),
                    None => out
                        .dividers
                        .push(r(bx + 4.0 * s, by + 8.0 * s, s, 20.0 * s)),
                }
                bx += w + GAP * s;
            }
            out.bounds = union(out.bounds, inflate(bar, (40.0 * s) as i32));
            out.toolbar = Some(bar);
        }
    } else if let Some(hv) = &sc.hover {
        let tag = text_w(&f.semibold, hv.target.label(), 13.0 * s) + 18.0 * s;
        let rest = text_w(&f.regular, &hover_text(hv), 13.0 * s);
        let (w, h) = (6.0 * s + tag + 8.0 * s + rest + 12.0 * s, 32.0 * s);
        let mon = sc.monitor;
        let x = (hv.rect.left as f32)
            .min(mon.right() as f32 - w)
            .max(mon.left as f32);
        let below = hv.rect.bottom() as f32 + 14.0 * s;
        let y = if below + h <= mon.bottom() as f32 {
            below
        } else {
            (hv.rect.bottom() as f32 - h - 14.0 * s).max(mon.top as f32)
        };
        let pill = r(x, y, w, h);
        out.bounds = union(out.bounds, inflate(pill, blur));
        out.bounds = union(out.bounds, inflate(hv.rect, (8.0 * s) as i32));
        out.hover_pill = Some(pill);
    }
    if let Some(pts) = sc.tools.as_ref().and_then(|t| t.lasso) {
        for p in pts {
            let dot = r(p.x - 4.0, p.y - 4.0, 8.0, 8.0);
            out.bounds = union(out.bounds, dot);
        }
    }
    if let Some(t) = sc.tools.as_ref().filter(|t| !sc.dragging && !t.rail_hidden) {
        rail_layout(&mut out, sc, t);
    }
    if !sc.dragging {
        let fs = 12.5 * s;
        let hints = hints(sc);
        let mut w = 32.0 * s + 18.0 * s * (hints.len() - 1) as f32;
        for &(k, v) in hints {
            w += text_w(&f.semibold, t(k), fs) + text_w(&f.regular, &format!(" {}", t(v)), fs);
        }
        let h = 34.0 * s;
        let m = sc.cursor_monitor;
        // Hints are a help, not a control: on a narrow monitor they stay out of the way.
        if w + 48.0 * s <= m.width as f32 {
            let bar = r(
                m.left as f32 + (m.width as f32 - w) / 2.0,
                m.bottom() as f32 - 24.0 * s - h,
                w,
                h,
            );
            out.bounds = union(out.bounds, inflate(bar, blur));
            out.hint_bar = Some(bar);
        }
    }
    if let Some(l) = &sc.loupe {
        let (side, mon) = (loupe_side(s), sc.cursor_monitor);
        let (w, h) = (side + 12.0 * s, side + 38.0 * s);
        let off = 22.0 * s;
        let (x, y) = (l.at.x as f32, l.at.y as f32);
        let px = if x + off + w > mon.right() as f32 {
            x - off - w
        } else {
            x + off
        };
        let py = if y + off + h > mon.bottom() as f32 {
            y - off - h
        } else {
            y + off
        };
        let panel = r(px, py, w, h);
        out.bounds = union(out.bounds, inflate(panel, 2));
        out.loupe = Some(panel);
    }
    out
}

pub(crate) const LOUPE_CELLS: i32 = 11;

pub(crate) fn loupe_cell(s: f32) -> f32 {
    (13.0 * s).round()
}

pub(crate) fn loupe_side(s: f32) -> f32 {
    loupe_cell(s) * LOUPE_CELLS as f32
}

pub(crate) fn hints(sc: &Scene<'_>) -> &'static [(&'static str, &'static str)] {
    let t = sc.tools.as_ref();
    let painting = t.is_some_and(|t| t.tool.is_some());
    match (t.is_some_and(|t| t.lasso_mode), sc.confirmed) {
        (_, false) if painting => &HINTS_PAINT,
        (true, true) if !painting => &HINTS_LASSO_DONE,
        (true, false) => &HINTS_LASSO,
        (_, true) => &HINTS_SELECTED,
        _ => &HINTS,
    }
}

/// Tool rail at the monitor's edge and the options of the active tool beside it.
/// The rail moves to the other edge when a selection would sit under it.
pub(crate) fn rail_layout(out: &mut Layout, sc: &Scene<'_>, t: &Draw<'_>) {
    let s = sc.scale;
    let mon = t.monitor;
    let (bw, bh) = (RAIL_BTN.0 * s, RAIL_BTN.1 * s);
    let (pad, gap, sep) = (6.0 * s, 2.0 * s, 13.0 * s);
    let items: Vec<Option<Control>> = [Some(Control::Select), Some(Control::Lasso), None]
        .into_iter()
        .chain(TOOLS.iter().map(|&t| Some(Control::Tool(t))))
        .chain([None, Some(Control::Picker)])
        .chain([None, Some(Control::Undo), Some(Control::Redo)])
        .collect();
    let sw = RAIL_W * s;
    let grip = 14.0 * s;
    let sh = 2.0 * pad
        + grip
        + items
            .iter()
            .map(|c| if c.is_some() { bh } else { sep })
            .sum::<f32>()
        + (items.len() - 1) as f32 * gap;
    let edge = 24.0 * s;
    let left = mon.left as f32 + edge;
    let reach = left + sw + 12.0 * s + OPTS_W * s;
    let covered = sc.selection.is_some_and(|sel| (sel.left as f32) < reach);
    let x = if covered {
        mon.right() as f32 - edge - sw
    } else {
        left
    };
    let y = mon.top as f32 + (mon.height as f32 - sh) / 2.0;
    let (x, y) = t.rail_at.map_or((x, y), |p| {
        let right = (mon.right() as f32 - sw).max(mon.left as f32);
        let bottom = (mon.bottom() as f32 - sh).max(mon.top as f32);
        (
            (p.x as f32).clamp(mon.left as f32, right),
            (p.y as f32).clamp(mon.top as f32, bottom),
        )
    });
    let covered = match t.rail_at {
        Some(_) => x + sw + 12.0 * s + OPTS_W * s > mon.right() as f32,
        None => covered,
    };
    let rail = r(x, y, sw, sh);
    out.grip = Some(r(x, y, sw, pad + grip));
    let bx = x + pad;
    let mut by = y + pad + grip;
    let mut first_tool = y;
    let mut picker_y = y;
    for c in items {
        match c {
            Some(c) => {
                if c == Control::Tool(TOOLS[0]) {
                    first_tool = by;
                }
                if c == Control::Picker {
                    picker_y = by;
                }
                out.buttons.push((r(bx, by, bw, bh), c));
                by += bh + gap;
            }
            None => {
                out.dividers
                    .push(r(bx + 8.0 * s, by + 6.0 * s, 28.0 * s, s));
                by += sep + gap;
            }
        }
    }
    out.bounds = union(out.bounds, inflate(rail, (40.0 * s) as i32));
    out.strip = Some(rail);

    let ow = OPTS_W * s;
    let ox = if covered {
        x - 12.0 * s - ow
    } else {
        x + sw + 12.0 * s
    };
    if t.picker {
        let row = 30.0 * s;
        let rows = ColorFormat::ALL.len() as f32;
        let h = rows * row + 16.0 * s;
        let top = (picker_y - h / 2.0).min(mon.bottom() as f32 - h - 12.0 * s);
        for i in 0..ColorFormat::ALL.len() {
            let cell = r(
                ox + 8.0 * s,
                top + 8.0 * s + i as f32 * row,
                ow - 16.0 * s,
                row,
            );
            out.buttons.push((cell, Control::ColorFormat(i)));
        }
        let opts = r(ox, top, ow, h);
        out.bounds = union(out.bounds, inflate(opts, (40.0 * s) as i32));
        out.options = Some(opts);
        return;
    }
    let Some(tool) = t.tool else { return };
    let rows: Vec<(Vec<Control>, f32)> = if tool == Tool::Blur {
        vec![
            (vec![Control::Gauss(false), Control::Gauss(true)], 36.0),
            ((0..3).map(Control::Strength).collect(), 32.0),
            ((0..3).map(Control::Size).collect(), 28.0),
        ]
    } else {
        vec![
            ((0..4).map(Control::Color).collect(), 26.0),
            ((4..8).map(Control::Color).collect(), 26.0),
            ((0..3).map(Control::Size).collect(), 28.0),
        ]
    };
    let inner = ow - 24.0 * s;
    let mut cy = first_tool + 6.0 * s;
    let top = first_tool - 6.0 * s;
    for (i, (row, cell)) in rows.iter().enumerate() {
        let cell = cell * s;
        let step = (inner - cell) / (row.len() - 1) as f32;
        for (k, c) in row.iter().enumerate() {
            let cx = ox + 12.0 * s + k as f32 * step;
            out.buttons.push((r(cx, cy, cell, cell), *c));
        }
        cy += cell;
        let next = rows.get(i + 1);
        if next.is_some_and(|(n, _)| !matches!(n[0], Control::Color(_))) {
            out.dividers.push(r(ox + 12.0 * s, cy + 9.0 * s, inner, s));
            cy += 19.0 * s;
        } else if next.is_some() {
            cy += 8.0 * s;
        }
    }
    let opts = r(ox, top, ow, cy + 12.0 * s - top);
    out.bounds = union(out.bounds, inflate(opts, (40.0 * s) as i32));
    out.options = Some(opts);
}

pub(crate) fn hover_text(hv: &Hover<'_>) -> String {
    let tail = t("колесо — уровень · клик — выбрать");
    if hv.app.is_empty() {
        tail.into()
    } else {
        format!("{} · {tail}", hv.app)
    }
}
