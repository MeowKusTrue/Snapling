use super::*;

fn scene(sel: Option<Rect>, confirmed: bool) -> Scene<'static> {
    let mon = Rect {
        left: 0,
        top: 0,
        width: 1440,
        height: 900,
    };
    Scene {
        scale: 1.0,
        monitor: mon,
        cursor_monitor: mon,
        selection: sel,
        confirmed,
        dragging: false,
        hover: None,
        hover_button: None,
        loupe: None,
        tools: None,
    }
}

#[test]
fn toolbar_matches_mockup_geometry() {
    let sel = Rect {
        left: 368,
        top: 210,
        width: 488,
        height: 214,
    };
    let lay = layout(&scene(Some(sel), true));
    let bar = lay.toolbar.unwrap();
    assert_eq!((bar.left, bar.top, bar.height), (368, 438, 46));
    assert_eq!(lay.buttons.len(), 11);
    assert_eq!(lay.buttons[0].1, Control::Action(Button::Copy));
    let pill = lay.size_pill.as_ref().unwrap();
    assert_eq!((pill.0.left, pill.0.top), (368, 182));
    assert_eq!(pill.1, "488 × 214");
    let p = Point {
        x: lay.buttons[1].0.left + 5,
        y: lay.buttons[1].0.top + 5,
    };
    assert_eq!(button_at(&lay, p), Some(Control::Action(Button::Save)));
}

#[test]
fn toolbar_flips_above_at_screen_bottom() {
    let sel = Rect {
        left: 100,
        top: 500,
        width: 300,
        height: 390,
    };
    let bar = layout(&scene(Some(sel), true)).toolbar.unwrap();
    assert!(bar.bottom() <= sel.top, "{bar:?}");
}

#[test]
fn draws_into_bgra_buffer() {
    let (w, h) = (1440u32, 900u32);
    let mut buf = vec![40u8; (w * h * 4) as usize];
    let sel = Rect {
        left: 368,
        top: 210,
        width: 488,
        height: 214,
    };
    let sc = scene(Some(sel), true);
    let lay = layout(&sc);
    let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
    draw(&mut pm, &sc, &lay);
    let px = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2]]
    };
    let copy = lay.buttons[0].0;
    assert_eq!(
        px(copy.left as u32 + 4, copy.top as u32 + 20),
        [0xE8, 0x5B, 0x29],
        "blue as BGRA"
    );
    assert_eq!(
        px(sel.left as u32, sel.top as u32 + 8),
        [255, 255, 255],
        "white corner"
    );
    assert_ne!(
        px(sel.left as u32, sel.top as u32 + 80),
        [255, 255, 255],
        "only a hairline between corners"
    );
}

#[test]
fn loupe_stays_on_the_monitor_and_shows_the_pixel() {
    let (w, h) = (400u32, 300u32);
    let mon = Rect {
        left: 0,
        top: 0,
        width: 400,
        height: 300,
    };
    let mut frame = [10u8, 20, 30, 255].repeat((w * h) as usize);
    let i = ((295 * w + 395) * 4) as usize;
    frame[i..i + 4].copy_from_slice(&[0x33, 0x22, 0x11, 255]);
    let at = Point { x: 395, y: 295 };
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
            at,
            frame: &frame,
            width: w,
            height: h,
            format: ColorFormat::Hex,
        }),
        tools: None,
    };
    let lay = layout(&sc);
    let panel = lay.loupe.unwrap();
    assert!(panel.right() < at.x && panel.bottom() < at.y, "{panel:?}");
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
    draw(&mut pm, &sc, &lay);
    let c = loupe_cell(1.0) as i32;
    let (x, y) = (
        panel.left + 6 + 5 * c + c / 2,
        panel.top + 6 + 5 * c + c / 2,
    );
    let i = ((y as u32 * w + x as u32) * 4) as usize;
    assert_eq!(
        buf[i..i + 3],
        [0x33, 0x22, 0x11],
        "the middle cell is the pixel"
    );
    assert_eq!(
        color_text(&[0x33, 0x22, 0x11, 255], ColorFormat::Hex),
        "#112233"
    );
}

#[test]
fn cached_glass_follows_what_is_under_it() {
    let (w, h) = (300u32, 200u32);
    let at = Rect {
        left: 100,
        top: 80,
        width: 100,
        height: 40,
    };
    let run = |buf: &mut [u8]| {
        let mut pm = PixmapMut::from_bytes(buf, w, h).unwrap();
        glass(&mut pm, at, 8.0, GLASS, 24.0, true);
    };
    drop_cache();
    let plain = [90u8, 90, 90, 255].repeat((w * h) as usize);
    let mut first = plain.clone();
    run(&mut first);
    let mut again = plain.clone();
    run(&mut again);
    assert_eq!(first, again, "a hit paints the same pixels");
    let mut other = plain.clone();
    let i = ((100 * w + 150) * 4) as usize;
    other[i..i + 40].fill(255);
    run(&mut other);
    assert_ne!(first, other, "a change under the panel is seen");
    drop_cache();
    assert_eq!(CACHE.with(|c| c.borrow().len()), 0);
}

#[test]
fn menu_finds_rows_and_skips_lines() {
    let rows = vec![
        MenuRow::Item(MenuIcon::Region, "Выделить".into(), "PrtScn".into()),
        MenuRow::Line,
        MenuRow::Item(MenuIcon::Quit, "Выход".into(), String::new()),
    ];
    let (w, h) = menu_size(&rows, 1.0);
    assert_eq!((w, h), (364, 36 + 1 + 8 + 36 + 8 + 64));
    let at = |y: i32| menu_at(&rows, 1.0, Point { x: 100, y: y + 32 });
    assert_eq!(at(4), Some(0));
    assert_eq!(at(40), None, "the gap before the line");
    assert_eq!(at(44), None, "the line");
    assert_eq!(at(60), Some(2));
    assert_eq!(
        menu_at(&rows, 1.0, Point { x: 2, y: 40 }),
        None,
        "the shadow"
    );
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
    draw_menu(&mut pm, &rows, 1.0, Some(0), false);
    assert_eq!(buf[3], 0, "corners stay transparent");
}

#[test]
fn toast_draws_a_shot_and_finds_its_buttons() {
    let (w, h) = toast_size(1.0);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let shot = [200u8, 200, 200, 255].repeat(40 * 20);
    let long = "очень длинное имя файла ".repeat(10);
    let tt = Toast {
        scale: 1.0,
        title: "Снимок скопирован",
        sub: &long,
        shot: (&shot, 40, 20),
        hover: Some(ToastAction::Text),
    };
    let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
    draw_toast(&mut pm, &tt);
    let at = |x: u32, y: u32| buf[((y * w + x) * 4) as usize];
    assert_eq!(at(60, 48), 200, "the shot fills the thumbnail");
    assert_eq!(buf[3], 0, "corners stay transparent");
    let (b, a, _) = toast_buttons(1.0)[1];
    let p = Point {
        x: b.left + 2,
        y: b.top + 2,
    };
    assert_eq!(a, ToastAction::Text);
    assert_eq!(toast_at(1.0, p), Some(ToastAction::Text));
    assert_eq!(toast_at(1.0, Point { x: 5, y: 5 }), None);
    let cut = fit_text(&fonts().regular, &long, 12.0, 200.0);
    assert!(cut.ends_with('…') && text_w(&fonts().regular, &cut, 12.0) <= 200.0);
}

#[test]
fn paint_region_copies_inside_the_outline_only() {
    let (w, h) = (20u32, 20u32);
    let src = vec![200u8; (w * h * 4) as usize];
    let mut buf = vec![10u8; (w * h * 4) as usize];
    let pt = |x, y| Pt { x, y };
    let pts = [pt(5.0, 5.0), pt(15.0, 5.0), pt(15.0, 15.0), pt(5.0, 15.0)];
    let from = PixmapRef::from_bytes(&src, w, h).unwrap();
    let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
    paint_region(&mut pm, from, &pts);
    let at = |x: u32, y: u32| buf[((y * w + x) * 4) as usize];
    assert_eq!(at(10, 10), 200);
    assert_eq!(at(2, 2), 10);
}

#[test]
fn rail_leaves_room_for_a_selection_on_the_left() {
    let mon = Rect {
        left: 0,
        top: 0,
        width: 1440,
        height: 900,
    };
    let draw = |tool| Draw {
        monitor: mon,
        rail_at: None,
        rail_hidden: false,
        lasso_mode: false,
        lasso: None,
        tool,
        color: COLORS[0],
        size: SIZES[1],
        blur: annotate::BlurStyle::default(),
        style: annotate::DrawStyle::default(),
        can_undo: false,
        can_redo: false,
        live: None,
        caret: None,
        picker: false,
        color_format: ColorFormat::Hex,
    };
    let mut sc = scene(None, false);
    sc.tools = Some(draw(Some(Tool::Arrow)));
    let lay = layout(&sc);
    assert_eq!(lay.strip.unwrap().left, 24);
    assert!(lay.options.is_some());
    assert_eq!(lay.buttons[0].1, Control::Select);
    let sel = Rect {
        left: 60,
        top: 200,
        width: 300,
        height: 200,
    };
    let mut sc = scene(Some(sel), true);
    sc.tools = Some(draw(None));
    let rail = layout(&sc).strip.unwrap();
    assert_eq!(rail.right(), 1440 - 24);
}

#[test]
fn svg_subset_parses_all_icons() {
    for b in [
        Button::Copy,
        Button::Save,
        Button::Edit,
        Button::Text,
        Button::Translate,
        Button::Pin,
        Button::Record,
        Button::Scroll,
        Button::Blur,
        Button::Cancel,
    ] {
        for d in icon_data(b) {
            assert!(svg_path(d, 0.0, 0.0, 1.0).is_some(), "{b:?}: {d}");
        }
    }
    for t in TOOLS {
        let more = UNDO
            .iter()
            .chain(&REDO)
            .chain(&DROP)
            .chain(&SELECT)
            .chain(&LASSO);
        for d in tool_icon(t).iter().chain(more) {
            assert!(svg_path(d, 0.0, 0.0, 1.0).is_some(), "{t:?}: {d}");
        }
    }
}
