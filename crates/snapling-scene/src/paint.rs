use super::*;

/// Mockup color into the BGRA buffer.
pub(crate) fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[2], c[1], c[0], c[3]);
    p.anti_alias = true;
    p
}

pub(crate) fn stroke(w: f32) -> Stroke {
    Stroke {
        width: w,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

pub(crate) fn rrect(x: f32, y: f32, w: f32, h: f32, rad: f32) -> Option<Path> {
    let rad = rad.min(w / 2.0).min(h / 2.0);
    let k = 0.552_284_8 * rad;
    let mut pb = PathBuilder::new();
    pb.move_to(x + rad, y);
    pb.line_to(x + w - rad, y);
    pb.cubic_to(x + w - rad + k, y, x + w, y + rad - k, x + w, y + rad);
    pb.line_to(x + w, y + h - rad);
    pb.cubic_to(
        x + w,
        y + h - rad + k,
        x + w - rad + k,
        y + h,
        x + w - rad,
        y + h,
    );
    pb.line_to(x + rad, y + h);
    pb.cubic_to(x + rad - k, y + h, x, y + h - rad + k, x, y + h - rad);
    pb.line_to(x, y + rad);
    pb.cubic_to(x, y + rad - k, x + rad - k, y, x + rad, y);
    pb.close();
    pb.finish()
}

/// Keeps a frame on screen: a full-screen window's outline is drawn just inside the monitor.
pub(crate) fn fit(a: Rect, mon: Rect, half_stroke: f32) -> Rect {
    let m = inflate(mon, -(half_stroke.ceil() as i32));
    let (l, t) = (a.left.max(m.left), a.top.max(m.top));
    Rect {
        left: l,
        top: t,
        width: (a.right().min(m.right()) - l).max(0),
        height: (a.bottom().min(m.bottom()) - t).max(0),
    }
}

pub(crate) fn rr(a: Rect, rad: f32) -> Option<Path> {
    rrect(
        a.left as f32,
        a.top as f32,
        a.width as f32,
        a.height as f32,
        rad,
    )
}

pub(crate) fn text(
    pm: &mut PixmapMut<'_>,
    font: &FontRef<'_>,
    s: &str,
    x: f32,
    cy: f32,
    px: f32,
    c: [u8; 4],
) -> f32 {
    let (w, h) = annotate::text_size_in(font, s, px);
    if let Some(p) = annotate::text_path_in(font, s, Pt { x, y: cy - h / 2.0 }, px) {
        pm.fill_path(
            &p,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    w
}
