use super::*;

#[test]
#[ignore]
fn render_loupe() {
    let dir = std::path::PathBuf::from(std::env::var("SNAP_PREVIEW").expect("SNAP_PREVIEW"));
    let (w, h) = (420u32, 300u32);
    let mon = Rect {
        left: 0,
        top: 0,
        width: 420,
        height: 300,
    };
    let mut frame = [54u8, 36, 26, 255].repeat((w * h) as usize);
    for y in 60..180u32 {
        for x in 80..300u32 {
            let i = ((y * w + x) * 4) as usize;
            let v = if (x + y) % 9 < 3 { 250 } else { 49 };
            frame[i..i + 4].copy_from_slice(&[v, v / 2 + 40, 255 - v / 2, 255]);
        }
    }
    let sc = Scene {
        scale: 1.0,
        monitor: mon,
        cursor_monitor: mon,
        selection: None,
        confirmed: false,
        dragging: false,
        hover: None,
        hover_button: None,
        loupe: Some(Loupe {
            at: Point { x: 120, y: 90 },
            frame: &frame,
            width: w,
            height: h,
            format: ColorFormat::Hex,
        }),
        tools: None,
    };
    let lay = layout(&sc);
    let mut buf = frame.clone();
    let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
    draw(&mut pm, &sc, &lay);
    snapling_core::storage::save_bgra(
        &buf,
        w,
        h,
        &dir,
        std::path::Path::new("loupe"),
        snapling_core::config::Format::Png,
        90,
    )
    .unwrap();
}

#[test]
#[ignore]
fn render_tray_menu() {
    let dir = std::path::PathBuf::from(std::env::var("SNAP_PREVIEW").expect("SNAP_PREVIEW"));
    let item = |i, a: &str, b: &str| MenuRow::Item(i, a.into(), b.into());
    let rows = vec![
        item(MenuIcon::Region, "Выделить область", "PrtScn"),
        item(MenuIcon::Screen, "Весь экран", "Shift+PrtScn"),
        item(MenuIcon::Repeat, "Повторить область", "Ctrl+PrtScn"),
        item(MenuIcon::Text, "Распознать текст", "Ctrl+Shift+O"),
        MenuRow::Line,
        item(MenuIcon::History, "История", ""),
        item(MenuIcon::Folder, "Папка снимков", ""),
        item(MenuIcon::Settings, "Настройки", ""),
        MenuRow::Line,
        item(MenuIcon::Quit, "Выход", ""),
    ];
    for (name, light, scale) in [("menu_dark", false, 1.0), ("menu_light", true, 1.5)] {
        let (w, h) = menu_size(&rows, scale);
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(26, 36, 54, 255));
        draw_menu(&mut pm.as_mut(), &rows, scale, Some(0), light);
        let path = snapling_core::storage::save_bgra(
            pm.data(),
            w,
            h,
            &dir,
            std::path::Path::new(name),
            snapling_core::config::Format::Png,
            90,
        )
        .unwrap();
        println!("{}", path.display());
    }
}

/// `SNAP_PREVIEW=<dir> cargo test -p snapling-scene preview -- --ignored`: the mockup scene as PNG.
#[test]
#[ignore]
fn render_mockup_scene() {
    let dir = std::path::PathBuf::from(std::env::var("SNAP_PREVIEW").expect("SNAP_PREVIEW"));
    if let Ok(lang) = std::env::var("SNAP_LANG") {
        snapling_core::i18n::set(&lang, "en", std::path::Path::new("."));
    }
    let (w, h) = (1440u32, 900u32);
    let sel = Rect {
        left: 368,
        top: 210,
        width: 488,
        height: 214,
    };
    let mut orig = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let panel = (96..856).contains(&x) && (80..640).contains(&y);
            let light = (900..1360).contains(&x) && (160..760).contains(&y);
            let stripe = ((x / 40 + y / 40) % 2) as u8 * 18;
            let (r, g, b) = if light {
                (247, 249, 252)
            } else if panel {
                (49 + stripe, 51, 56)
            } else {
                (26, 36, 54)
            };
            orig[i..i + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    let mut buf = orig.clone();
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if !sel.contains(Point { x, y }) {
                let i = ((y as u32 * w + x as u32) * 4) as usize;
                for (c, k) in [(0, 16u32), (1, 8), (2, 4)] {
                    buf[i + c] = ((u32::from(buf[i + c]) + k) / 2) as u8;
                }
            }
        }
    }
    let mon = Rect {
        left: 0,
        top: 0,
        width: 1440,
        height: 900,
    };
    let live = annotate::Element {
        kind: annotate::Kind::Arrow(
            Pt { x: 420.0, y: 380.0 },
            Pt { x: 700.0, y: 280.0 },
            annotate::Head::Sharp,
        ),
        color: COLORS[0],
        size: SIZES[1],
    };
    for (name, sc) in [
        (
            "selection",
            Scene {
                scale: 1.0,
                monitor: mon,
                cursor_monitor: mon,
                selection: Some(sel),
                confirmed: true,
                dragging: false,
                hover: None,
                hover_button: Some(Control::Action(Button::Edit)),
                loupe: None,
                tools: Some(Draw {
                    monitor: mon,
                    rail_at: None,
                    rail_hidden: false,
                    lasso_mode: false,
                    lasso: None,
                    tool: Some(Tool::Blur),
                    color: COLORS[0],
                    size: SIZES[1],
                    blur: annotate::BlurStyle {
                        gauss: true,
                        strength: 1,
                    },
                    can_undo: true,
                    can_redo: false,
                    live: Some(&live),
                    caret: Some((Pt { x: 400.0, y: 230.0 }, "Hello")),
                    picker: false,
                    color_format: ColorFormat::Hex,
                }),
            },
        ),
        (
            "hover",
            Scene {
                scale: 1.0,
                monitor: mon,
                cursor_monitor: mon,
                selection: None,
                confirmed: false,
                dragging: false,
                hover: Some(Hover {
                    rect: Rect {
                        left: 900,
                        top: 160,
                        width: 460,
                        height: 600,
                    },
                    target: Target::Window,
                    app: "Firefox",
                }),
                hover_button: None,
                loupe: None,
                tools: None,
            },
        ),
    ] {
        let mut b = buf.clone();
        let lay = layout(&sc);
        let mut pm = PixmapMut::from_bytes(&mut b, w, h).unwrap();
        draw(&mut pm, &sc, &lay);
        let path = snapling_core::storage::save_bgra(
            &b,
            w,
            h,
            &dir,
            std::path::Path::new(name),
            snapling_core::config::Format::Png,
            90,
        )
        .unwrap();
        println!("{}", path.display());
    }
}
