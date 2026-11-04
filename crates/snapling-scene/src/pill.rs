use super::*;

/// Size of the recording pill at scale 1.
pub const PILL_SIZE: (f32, f32) = (380.0, 40.0);
/// Share of the pill that stops the recording; the side buttons split the rest.
pub const PILL_STOP: f32 = 0.5;

/// Side button of the pill under `x` (0..width), `None`: the stop part.
pub fn pill_button(x: f32, width: f32, buttons: usize) -> Option<usize> {
    let main = width * PILL_STOP;
    (buttons > 0 && x >= main)
        .then(|| (((x - main) / (width - main) * buttons as f32) as usize).min(buttons - 1))
}

/// Status pill of a recording or a long shot. `buttons`: side parts after the stop part
/// (pause, cancel); none for a long shot.
pub fn draw_pill(pm: &mut PixmapMut<'_>, label: &str, buttons: &[&str], s: f32) {
    let cancel = (!buttons.is_empty()).then_some(());
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let id = Transform::identity();
    if let Some(p) = rrect(0.5, 0.5, w - 1.0, h - 1.0, h / 2.0) {
        pm.fill_path(&p, &paint([22, 27, 39, 240]), FillRule::Winding, id, None);
        pm.stroke_path(&p, &paint([255, 255, 255, 40]), &stroke(1.0), id, None);
    }
    let font = &fonts().semibold;
    let main = if cancel.is_some() { w * PILL_STOP } else { w };
    // The stop half is a red button, so it reads as one.
    let inset = 4.0 * s;
    if cancel.is_some()
        && let Some(p) = rrect(
            inset,
            inset,
            main - inset * 1.5,
            h - inset * 2.0,
            h / 2.0 - inset,
        )
    {
        pm.fill_path(&p, &paint([255, 69, 58, 235]), FillRule::Winding, id, None);
    }
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
    let seg = (w - main) / buttons.len().max(1) as f32;
    for (i, b) in buttons.iter().enumerate() {
        let left = main + seg * i as f32;
        if i > 0 {
            let line = tiny_skia::Rect::from_xywh(left, h * 0.25, s.max(1.0), h * 0.5);
            if let Some(line) = line {
                pm.fill_rect(line, &paint([255, 255, 255, 40]), id, None);
            }
        }
        let width = text_w(font, b, 13.5 * s);
        let x = left + (seg - width) / 2.0;
        text(pm, font, b, x, h / 2.0, 13.5 * s, ICON_MUTED);
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
