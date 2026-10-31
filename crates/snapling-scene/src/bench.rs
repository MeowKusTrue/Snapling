use super::*;

/// `cargo test --release -p snapling-scene bench_frame -- --ignored --nocapture`:
/// one overlay frame on the widest screen, panels and glass included.
#[test]
#[ignore]
fn bench_frame() {
    let (w, h) = (4480u32, 1527u32);
    let mon = Rect {
        left: 0,
        top: 0,
        width: 2560,
        height: 1527,
    };
    let sel = Rect {
        left: 600,
        top: 300,
        width: 900,
        height: 500,
    };
    let mut buf = vec![70u8; (w * h * 4) as usize];
    let sc = Scene {
        scale: 1.0,
        monitor: mon,
        cursor_monitor: mon,
        selection: Some(sel),
        confirmed: true,
        dragging: false,
        hover: None,
        hover_button: None,
        loupe: None,
        tools: Some(Draw {
            monitor: mon,
            rail_at: None,
            rail_hidden: false,
            lasso_mode: false,
            lasso: None,
            tool: Some(Tool::Arrow),
            color: COLORS[0],
            size: SIZES[1],
            blur: annotate::BlurStyle::default(),
            style: annotate::DrawStyle::default(),
            can_undo: true,
            can_redo: false,
            live: None,
            caret: None,
            picker: false,
            color_format: ColorFormat::Hex,
        }),
    };
    let frame = buf.clone();
    for _ in 0..4 {
        buf.copy_from_slice(&frame);
        let t0 = std::time::Instant::now();
        let lay = layout(&sc);
        let laid = t0.elapsed().as_micros();
        let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
        draw(&mut pm, &sc, &lay);
        println!("layout {laid} us, frame {} us", t0.elapsed().as_micros());
    }
}
