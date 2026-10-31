use super::*;

/// Size of the recording pill at scale 1.
pub const PILL_SIZE: (f32, f32) = (300.0, 40.0);
/// Share of the pill that stops the recording; the rest cancels it.
pub const PILL_STOP: f32 = 0.62;

/// Status pill of a recording or a long shot. `cancel`: a second half that drops the recording.
pub fn draw_pill(pm: &mut PixmapMut<'_>, label: &str, cancel: Option<&str>, s: f32) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let id = Transform::identity();
    if let Some(p) = rrect(0.5, 0.5, w - 1.0, h - 1.0, h / 2.0) {
        pm.fill_path(&p, &paint([22, 27, 39, 240]), FillRule::Winding, id, None);
        pm.stroke_path(&p, &paint([255, 255, 255, 40]), &stroke(1.0), id, None);
    }
    let font = &fonts().semibold;
    let main = if cancel.is_some() { w * PILL_STOP } else { w };
    let label = fit_text(font, label, 13.5 * s, main - 24.0 * s);
    let width = text_w(font, &label, 13.5 * s);
    text(
        pm,
        font,
        &label,
        (main - width) / 2.0,
        h / 2.0,
        13.5 * s,
        TEXT,
    );
    if let Some(cancel) = cancel {
        let line = tiny_skia::Rect::from_xywh(main, h * 0.25, s.max(1.0), h * 0.5);
        if let Some(line) = line {
            pm.fill_rect(line, &paint([255, 255, 255, 40]), id, None);
        }
        let width = text_w(font, cancel, 13.5 * s);
        let x = main + (w - main - width) / 2.0;
        text(pm, font, cancel, x, h / 2.0, 13.5 * s, ICON_MUTED);
    }
}

/// Red frame around what is being recorded.
pub fn draw_rec_frame(pm: &mut PixmapMut<'_>, s: f32) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let id = Transform::identity();
    if let Some(p) = rrect(1.5, 1.5, w - 3.0, h - 3.0, 3.0 * s) {
        pm.stroke_path(&p, &paint([255, 69, 58, 255]), &stroke(2.0), id, None);
    }
}

pub(crate) const DIM: [u32; 3] = [16, 8, 4];

/// Darkens a frame by `a` of 256 towards the overlay's night blue. Alpha ends up opaque.
pub fn dim_into(dst: &mut [u8], src: &[u8], a: u32) {
    let keep = 256 - a;
    for (d, p) in dst
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src.as_chunks::<4>().0)
    {
        let mix = |c: usize| ((u32::from(p[c]) * keep + DIM[c] * a) >> 8) as u8;
        *d = [mix(0), mix(1), mix(2), 255];
    }
}

/// Side of the countdown mark before a delayed shot.
pub const COUNT_SIZE: f32 = 104.0;

/// Seconds left, into a transparent BGRA buffer, premultiplied.
pub fn draw_count(pm: &mut PixmapMut<'_>, left: u32, s: f32) {
    let c = pm.width() as f32 / 2.0;
    let id = Transform::identity();
    if let Some(p) = PathBuilder::from_circle(c, c, c - 1.0) {
        pm.fill_path(&p, &paint([22, 27, 39, 235]), FillRule::Winding, id, None);
        pm.stroke_path(&p, &paint([255, 255, 255, 40]), &stroke(1.0), id, None);
    }
    let digits = left.to_string();
    let font = &fonts().semibold;
    let w = text_w(font, &digits, 44.0 * s);
    text(pm, font, &digits, c - w / 2.0, c, 44.0 * s, TEXT);
}
