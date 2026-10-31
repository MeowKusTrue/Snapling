use super::*;

#[test]
fn fullscreen_outline_stays_visible() {
    let (w, h) = (800u32, 600u32);
    let mon = Rect {
        left: 0,
        top: 0,
        width: 800,
        height: 600,
    };
    let mut buf = vec![30u8; (w * h * 4) as usize];
    let sc = Scene {
        scale: 1.0,
        monitor: mon,
        cursor_monitor: mon,
        selection: None,
        confirmed: false,
        dragging: false,
        hover: Some(Hover {
            rect: mon,
            target: Target::Window,
            app: "Valorant",
        }),
        hover_button: None,
        loupe: None,
        tools: None,
    };
    let lay = layout(&sc);
    let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
    draw(&mut pm, &sc, &lay);
    let i = ((300 * w + 1) * 4) as usize;
    assert_eq!(
        [buf[i], buf[i + 1], buf[i + 2]],
        [0x1F, 0x8A, 0xFF],
        "orange at the left edge"
    );
}
