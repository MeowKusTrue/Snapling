use super::*;

/// Mockup icons (24-unit grid, 2-unit round strokes) as SVG path data.
pub(crate) const UNDO: [&str; 2] = [
    "M 9 14 L 4 9 L 9 4",
    "M 4 9 H 15 C 18 9 20 11 20 14 C 20 17 18 19 15 19 H 12",
];
pub(crate) const REDO: [&str; 2] = [
    "M 15 14 L 20 9 L 15 4",
    "M 20 9 H 9 C 6 9 4 11 4 14 C 4 17 6 19 9 19 H 12",
];
pub(crate) const CUBES: [&str; 5] = [
    "M 5 5 H 9 V 9 H 5 Z",
    "M 15 5 H 19 V 9 H 15 Z",
    "M 10 10 H 14 V 14 H 10 Z",
    "M 5 15 H 9 V 19 H 5 Z",
    "M 15 15 H 19 V 19 H 15 Z",
];
pub(crate) const SELECT: [&str; 4] = [
    "M 4 8 V 5 H 7",
    "M 17 5 H 20 V 8",
    "M 20 16 V 19 H 17",
    "M 7 19 H 4 V 16",
];
pub(crate) const LASSO: [&str; 2] = [
    "M 12 4 C 17 4 20 6.2 20 9.5 C 20 12.8 16 15 12 15 C 8 15 4 12.8 4 9.5 C 4 6.2 7 4 12 4 Z",
    "M 7 14 C 6 16 6 18 8 19 C 9.5 19.7 11 19 11 17.5",
];
pub(crate) const PIPETTE: [&str; 3] = [
    "M 14 6 L 18 10",
    "M 16 4 C 17 3 19 3 20 4 C 21 5 21 7 20 8 L 17 11 L 13 7 Z",
    "M 14 9 L 6 17 L 5 20 L 8 19 L 16 11",
];
pub(crate) const HEAD_SHARP: [&str; 2] = ["M 5 19 L 14 10", "M 11 5 H 19 V 13 Z"];
pub(crate) const HEAD_SOFT: [&str; 2] = ["M 5 19 L 19 5", "M 10 5 H 19 V 14"];
pub(crate) const HEAD_DOUBLE: [&str; 3] =
    ["M 8 16 L 16 8", "M 12 5 H 19 V 12 Z", "M 5 12 V 19 H 12 Z"];
pub(crate) const SHAPE_LINE: [&str; 1] = ["M 4 20 L 20 4"];
pub(crate) const SHAPE_HAND: [&str; 1] = ["M 4 19 C 8 9 12 21 20 5"];
pub(crate) const TO_END: [&str; 3] = [
    "M 8 12 H 20",
    "M 15 7 L 20 12 L 15 17",
    "M 3 10 H 6 V 14 H 3 Z",
];
pub(crate) const TO_START: [&str; 3] = [
    "M 4 12 H 16",
    "M 9 7 L 4 12 L 9 17",
    "M 18 10 H 21 V 14 H 18 Z",
];
pub(crate) const PREV: [&str; 1] = ["M 15 6 L 9 12 L 15 18"];
pub(crate) const NEXT: [&str; 1] = ["M 9 6 L 15 12 L 9 18"];
pub(crate) const DROP: [&str; 1] =
    ["M 12 4 C 12 4 6 11 6 15 C 6 18.3 8.7 21 12 21 C 15.3 21 18 18.3 18 15 C 18 11 12 4 12 4 Z"];

pub(crate) fn tool_icon(t: Tool) -> &'static [&'static str] {
    match t {
        Tool::Arrow => &["M 5 19 L 19 5", "M 10 5 H 19 V 14"],
        Tool::Line => &["M 5 19 L 19 5"],
        Tool::Rect => {
            &["M 7 5 H 17 Q 19 5 19 7 V 17 Q 19 19 17 19 H 7 Q 5 19 5 17 V 7 Q 5 5 7 5 Z"]
        }
        Tool::Ellipse => &[
            "M 12 5 C 15.9 5 19 8.1 19 12 C 19 15.9 15.9 19 12 19 C 8.1 19 5 15.9 5 12 C 5 8.1 8.1 5 12 5 Z",
        ],
        Tool::Pen => &["M 4 20 L 8 19 L 19 8 L 16 5 L 5 16 L 4 20 Z"],
        Tool::Marker => &["M 4 20 H 9", "M 7 17 L 16 8 L 19 11 L 10 20 Z"],
        Tool::Text => &["M 6 6 H 18", "M 12 6 V 19", "M 9 19 H 15"],
        Tool::Step => &[
            "M 12 4 C 16.4 4 20 7.6 20 12 C 20 16.4 16.4 20 12 20 C 7.6 20 4 16.4 4 12 C 4 7.6 7.6 4 12 4 Z",
            "M 10.5 9.5 L 12.5 8 V 16",
        ],
        Tool::Blur => &CUBES,
    }
}

pub(crate) fn icon_data(b: Button) -> &'static [&'static str] {
    match b {
        Button::Draw => tool_icon(Tool::Arrow),
        Button::Copy => &[
            "M 10.5 8 H 17.5 Q 20 8 20 10.5 V 17.5 Q 20 20 17.5 20 H 10.5 Q 8 20 8 17.5 V 10.5 Q 8 8 10.5 8 Z",
            "M 16 8 V 6 Q 16 4 14 4 H 6 Q 4 4 4 6 V 14 Q 4 16 6 16 H 8",
        ],
        Button::Save => &["M 12 4 V 15", "M 7 10 L 12 15 L 17 10", "M 5 20 H 19"],
        Button::Edit => &["M 4 20 L 8 19 L 19 8 L 16 5 L 5 16 L 4 20 Z"],
        Button::Text => &[
            "M 4 8 V 5 H 7",
            "M 17 5 H 20 V 8",
            "M 20 16 V 19 H 17",
            "M 7 19 H 4 V 16",
            "M 9 9 H 15",
            "M 12 9 V 15",
        ],
        Button::Translate => &[
            "M 4 6 H 13",
            "M 8.5 4 V 6",
            "M 6 6 C 6 10 9 13 12 14",
            "M 11 6 C 11 9 9 12 5 14",
            "M 13 20 L 17 11 L 21 20",
            "M 14.5 17 H 19.5",
        ],
        Button::Pin => &[
            "M 15 3 L 21 9",
            "M 16 4 L 10 10 L 6 10 L 14 18 L 14 14 L 20 8",
            "M 9 15 L 4 20",
        ],
        Button::Record => &[
            CIRCLE,
            "M 12 9 C 13.7 9 15 10.3 15 12 C 15 13.7 13.7 15 12 15 C 10.3 15 9 13.7 9 12 C 9 10.3 10.3 9 12 9 Z",
        ],
        Button::Scroll => &[
            "M 7 3 H 17 Q 19 3 19 5 V 13 Q 19 15 17 15 H 7 Q 5 15 5 13 V 5 Q 5 3 7 3 Z",
            "M 12 18 V 22",
            "M 9 19 L 12 22 L 15 19",
        ],
        Button::Blur => &CUBES,
        Button::Cancel => &["M 6 6 L 18 18", "M 18 6 L 6 18"],
    }
}

/// Parses the absolute M/L/H/V/C/Q/Z subset used by [`icon_data`].
pub(crate) fn svg_path(d: &str, x: f32, y: f32, k: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let mut toks = d.split_whitespace().peekable();
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    let mut cmd = 'M';
    let num = |t: &mut std::iter::Peekable<std::str::SplitWhitespace<'_>>| -> f32 {
        t.next().and_then(|v| v.parse().ok()).unwrap_or(0.0)
    };
    while let Some(&t) = toks.peek() {
        if let Some(c) = t.chars().next().filter(char::is_ascii_alphabetic) {
            cmd = c;
            toks.next();
            if cmd == 'Z' {
                pb.close();
                continue;
            }
        }
        match cmd {
            'M' => {
                cx = num(&mut toks);
                cy = num(&mut toks);
                pb.move_to(x + cx * k, y + cy * k);
                cmd = 'L';
            }
            'L' => {
                cx = num(&mut toks);
                cy = num(&mut toks);
                pb.line_to(x + cx * k, y + cy * k);
            }
            'H' => {
                cx = num(&mut toks);
                pb.line_to(x + cx * k, y + cy * k);
            }
            'V' => {
                cy = num(&mut toks);
                pb.line_to(x + cx * k, y + cy * k);
            }
            'Q' => {
                let (x1, y1) = (num(&mut toks), num(&mut toks));
                cx = num(&mut toks);
                cy = num(&mut toks);
                pb.quad_to(x + x1 * k, y + y1 * k, x + cx * k, y + cy * k);
            }
            'C' => {
                let (x1, y1, x2, y2) = (
                    num(&mut toks),
                    num(&mut toks),
                    num(&mut toks),
                    num(&mut toks),
                );
                cx = num(&mut toks);
                cy = num(&mut toks);
                pb.cubic_to(
                    x + x1 * k,
                    y + y1 * k,
                    x + x2 * k,
                    y + y2 * k,
                    x + cx * k,
                    y + cy * k,
                );
            }
            _ => {
                toks.next();
            }
        }
    }
    pb.finish()
}

pub(crate) fn icon(
    pm: &mut PixmapMut<'_>,
    paths: &[&str],
    cx: f32,
    cy: f32,
    size: f32,
    c: [u8; 4],
) {
    let k = size / 24.0;
    let (x, y) = (cx - size / 2.0, cy - size / 2.0);
    for d in paths {
        if let Some(p) = svg_path(d, x, y, k) {
            pm.stroke_path(&p, &paint(c), &stroke(2.0 * k), Transform::identity(), None);
        }
    }
}
