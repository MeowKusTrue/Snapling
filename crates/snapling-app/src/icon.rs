//! Frame Spark mark rasterized in code: no SVG dependency.

type P = (f32, f32);

fn quad(a: P, c: P, b: P, out: &mut Vec<P>) {
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        let u = 1.0 - t;
        out.push((
            u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0,
            u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1,
        ));
    }
}

fn arc(c: P, from_deg: f32, to_deg: f32, out: &mut Vec<P>) {
    for i in 0..=8 {
        let a = (from_deg + (to_deg - from_deg) * i as f32 / 8.0).to_radians();
        out.push((c.0 + 3.0 * a.cos(), c.1 + 3.0 * a.sin()));
    }
}

fn corners() -> Vec<Vec<P>> {
    let mut tl = vec![(4.0, 10.0)];
    arc((7.0, 7.0), 180.0, 270.0, &mut tl);
    tl.push((10.0, 4.0));
    let mut bl = vec![(4.0, 14.0)];
    arc((7.0, 17.0), 180.0, 90.0, &mut bl);
    bl.push((10.0, 20.0));
    let mut br = vec![(20.0, 14.0)];
    arc((17.0, 17.0), 0.0, 90.0, &mut br);
    br.push((14.0, 20.0));
    vec![tl, bl, br]
}

fn spark() -> Vec<P> {
    let mut s = vec![(17.0, 2.0)];
    quad((17.0, 2.0), (17.9, 6.1), (22.0, 7.0), &mut s);
    quad((22.0, 7.0), (17.9, 7.9), (17.0, 12.0), &mut s);
    quad((17.0, 12.0), (16.1, 7.9), (12.0, 7.0), &mut s);
    quad((12.0, 7.0), (16.1, 6.1), (17.0, 2.0), &mut s);
    s
}

fn seg_dist(p: P, a: P, b: P) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

fn inside(p: P, poly: &[P]) -> bool {
    let mut c = false;
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

/// RGBA, `size`×`size`, 4×4 supersampled.
pub fn frame_spark(size: u32, corner_rgb: [u8; 3], spark_rgb: [u8; 3]) -> Vec<u8> {
    const SS: u32 = 4;
    let (lines, star) = (corners(), spark());
    let k = 24.0 / size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut nc, mut ns) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let p = (
                        (x as f32 + (sx as f32 + 0.5) / SS as f32) * k,
                        (y as f32 + (sy as f32 + 0.5) / SS as f32) * k,
                    );
                    if inside(p, &star) {
                        ns += 1;
                    } else if lines
                        .iter()
                        .any(|l| l.windows(2).any(|w| seg_dist(p, w[0], w[1]) <= 1.5))
                    {
                        nc += 1;
                    }
                }
            }
            let n = nc + ns;
            let rgb = if ns > nc { spark_rgb } else { corner_rgb };
            out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], (n * 255 / (SS * SS)) as u8]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn shape_lands_where_the_svg_has_it() {
        let img = super::frame_spark(24, [0, 0, 255], [255, 0, 0]);
        let px = |x: usize, y: usize| &img[(y * 24 + x) * 4..(y * 24 + x) * 4 + 4];
        assert_eq!(px(12, 12)[3], 0, "center is empty");
        assert_eq!(px(4, 18), &[0, 0, 255, 255], "bottom-left corner stroke");
        assert_eq!(px(17, 7), &[255, 0, 0, 255], "spark center");
        assert_eq!(px(21, 3)[3], 0, "between spark rays");
    }
}
