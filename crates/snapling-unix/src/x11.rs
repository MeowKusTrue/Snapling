//! The root window of an X11 session as BGRA.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};

pub fn root_image() -> Result<(Vec<u8>, u32, u32), String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = &conn.setup().roots[screen];
    let (w, h) = (root.width_in_pixels, root.height_in_pixels);
    let image = conn
        .get_image(ImageFormat::Z_PIXMAP, root.root, 0, 0, w, h, !0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let mut bgra = image.data;
    for px in bgra.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    Ok((bgra, u32::from(w), u32::from(h)))
}
