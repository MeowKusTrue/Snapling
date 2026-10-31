use super::*;

/// Three box passes ≈ a Gaussian (CSS `blur(sigma)`), on premultiplied BGRA.
/// Copy of `area` (clamped) from the buffer.
pub(crate) fn grab(pm: &PixmapMut<'_>, area: Rect) -> Option<(Pixmap, i32, i32)> {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (x0, y0) = (area.left.max(0), area.top.max(0));
    let (x1, y1) = (area.right().min(w), area.bottom().min(h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut out = Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32)?;
    let src = pm.as_ref().data();
    let row = ((x1 - x0) * 4) as usize;
    for y in y0..y1 {
        let s = ((y * w + x0) * 4) as usize;
        let d = ((y - y0) as usize) * row;
        out.data_mut()[d..d + row].copy_from_slice(&src[s..s + row]);
    }
    Some((out, x0, y0))
}

/// Writes a [`grab`] copy back.
pub(crate) fn put(pm: &mut PixmapMut<'_>, part: &Pixmap, x0: i32, y0: i32) {
    let w = pm.width() as usize;
    let row = part.width() as usize * 4;
    let dst = pm.data_mut();
    for (y, line) in part.data().chunks_exact(row).enumerate() {
        let at = ((y0 as usize + y) * w + x0 as usize) * 4;
        dst[at..at + row].copy_from_slice(line);
    }
}

/// Painted glass, reusable while the pixels under it stay the same.
pub(crate) struct Pane {
    pub(crate) key: (Rect, u32, [u8; 4], u32, bool),
    pub(crate) under: Pixmap,
    pub(crate) done: Pixmap,
}

/// Panels on screen at once are a handful; moved ones age out.
pub(crate) const PANES: usize = 8;

thread_local! {
    pub(crate) static CACHE: RefCell<Vec<Pane>> = const { RefCell::new(Vec::new()) };
}

/// Forgets the cached glass: it holds pieces of the screenshot.
pub fn drop_cache() {
    CACHE.with(|c| *c.borrow_mut() = Vec::new());
}

/// Frosted panel, cached: blurring is most of a frame, and most frames change nothing under it.
pub(crate) fn glass(
    pm: &mut PixmapMut<'_>,
    at: Rect,
    rad: f32,
    tint: [u8; 4],
    blur: f32,
    shadow: bool,
) {
    let reach = if shadow { 2.4 * blur } else { 1.5 * blur };
    let area = inflate(at, reach.ceil() as i32);
    let Some((under, x0, y0)) = grab(pm, area) else {
        return glass_now(pm, at, rad, tint, blur, shadow);
    };
    let key = (at, rad.to_bits(), tint, blur.to_bits(), shadow);
    let hit = CACHE.with(|c| {
        let c = c.borrow();
        let same = c.iter().find(|p| p.key == key && p.under == under);
        same.map(|p| put(pm, &p.done, x0, y0)).is_some()
    });
    if hit {
        return;
    }
    glass_now(pm, at, rad, tint, blur, shadow);
    if let Some((done, ..)) = grab(pm, area) {
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            c.retain(|p| p.key != key);
            if c.len() == PANES {
                c.remove(0);
            }
            c.push(Pane { key, under, done });
        });
    }
}

/// Blurred backdrop, tint, hairline, optional drop shadow.
pub(crate) fn glass_now(
    pm: &mut PixmapMut<'_>,
    at: Rect,
    rad: f32,
    tint: [u8; 4],
    blur: f32,
    shadow: bool,
) {
    let id = Transform::identity();
    if shadow {
        let sh = Rect {
            top: at.top + (16.0 * blur / 24.0) as i32,
            ..inflate(at, -(12.0 * blur / 24.0) as i32)
        };
        let pad = (40.0 * blur / 24.0) as i32;
        if let Some(mut layer) =
            Pixmap::new((sh.width + 2 * pad) as u32, (sh.height + 2 * pad) as u32)
        {
            if let Some(p) = rrect(
                pad as f32,
                pad as f32,
                sh.width as f32,
                sh.height as f32,
                rad,
            ) {
                layer.fill_path(&p, &paint([0, 0, 0, 153]), FillRule::Winding, id, None);
            }
            annotate::box_blur(&mut layer, (pad / 3).max(1) as usize);
            pm.draw_pixmap(
                sh.left - pad,
                sh.top - pad,
                layer.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                id,
                None,
            );
        }
    }
    let Some(path) = rr(at, rad) else { return };
    let b = (blur / 2.0).round() as i32;
    if let Some((mut back, x0, y0)) = grab(pm, inflate(at, b * 3)) {
        annotate::box_blur(&mut back, b.max(1) as usize);
        let shader = tiny_skia::Pattern::new(
            back.as_ref(),
            tiny_skia::SpreadMode::Pad,
            tiny_skia::FilterQuality::Nearest,
            1.0,
            Transform::from_translate(x0 as f32, y0 as f32),
        );
        let p = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        pm.fill_path(&path, &p, FillRule::Winding, id, None);
    }
    pm.fill_path(&path, &paint(tint), FillRule::Winding, id, None);
    if let Some(inner) = rr(inflate(at, -1), (rad - 1.0).max(0.0)) {
        pm.stroke_path(&inner, &paint(HAIRLINE), &stroke(1.0), id, None);
    }
}
