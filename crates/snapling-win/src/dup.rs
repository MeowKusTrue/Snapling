//! DXGI Desktop Duplication: sees exclusive-fullscreen games, where GDI gets a black frame.
//! System API only, nothing touches the game.

use crate::Image;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_OUTPUT_DESC, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput5,
    IDXGIResource,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::core::Interface;

/// The monitor's adapter and output: a device must come from the adapter that owns the output.
fn find_output(
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
) -> Result<(IDXGIAdapter1, IDXGIOutput, DXGI_OUTPUT_DESC), String> {
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|e| format!("DXGI factory: {e}"))?;
    for a in 0.. {
        let adapter = match unsafe { factory.EnumAdapters1(a) } {
            Ok(adapter) => adapter,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(format!("DXGI adapter: {e}")),
        };
        for o in 0.. {
            let Ok(output) = (unsafe { adapter.EnumOutputs(o) }) else {
                break;
            };
            if let Ok(desc) = unsafe { output.GetDesc() }
                && desc.Monitor == monitor
            {
                return Ok((adapter, output, desc));
            }
        }
    }
    Err("DXGI: monitor not found".into())
}

/// Opaque BGRA top-down, width, height, desktop position.
type Frame = (Vec<u8>, u32, u32, (i32, i32));

/// One frame of the monitor.
fn duplicate(monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> Result<Frame, String> {
    let (adapter, output, desc) = find_output(monitor)?;
    if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY
        && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED
    {
        // ponytail: a rotated monitor stays with GDI; turn the rows here if a game ever needs it.
        return Err("DXGI: rotated monitor".into());
    }
    let origin = (desc.DesktopCoordinates.left, desc.DesktopCoordinates.top);
    let mut device: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            &adapter.cast::<IDXGIAdapter>().map_err(|e| e.to_string())?,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )
    }
    .map_err(|e| format!("D3D11: {e}"))?;
    let (Some(device), Some(ctx)) = (device, ctx) else {
        return Err("D3D11: no device".into());
    };
    // Not plain DuplicateOutput: it refuses per-monitor-aware processes like this one.
    let dup = unsafe {
        output
            .cast::<IDXGIOutput5>()
            .map_err(|e| format!("IDXGIOutput5: {e}"))?
            .DuplicateOutput1(&device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM])
    }
    .map_err(|e| format!("DuplicateOutput1: {e}"))?;

    // A frame with nothing accumulated is only a cursor move, its texture says nothing new.
    let mut frame = None;
    for _ in 0..4 {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        match unsafe { dup.AcquireNextFrame(200, &mut info, &mut res) } {
            Ok(()) if info.AccumulatedFrames > 0 => {
                frame = res;
                break;
            }
            Ok(()) => {
                let _ = unsafe { dup.ReleaseFrame() };
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
            Err(e) => return Err(format!("AcquireNextFrame: {e}")),
        }
    }
    let frame = frame.ok_or("DXGI: no frame")?;
    let copied = copy_frame(&device, &ctx, &frame);
    let _ = unsafe { dup.ReleaseFrame() };
    let (bgra, w, h) = copied?;
    Ok((bgra, w, h, origin))
}

fn copy_frame(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    frame: &IDXGIResource,
) -> Result<(Vec<u8>, u32, u32), String> {
    let tex: ID3D11Texture2D = frame.cast().map_err(|e| e.to_string())?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { tex.GetDesc(&mut desc) };
    let (w, h) = (desc.Width, desc.Height);
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    desc.MiscFlags = 0;
    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging)) }
        .map_err(|e| format!("CreateTexture2D: {e}"))?;
    let staging = staging.ok_or("CreateTexture2D: empty")?;
    unsafe { ctx.CopyResource(&staging, &tex) };
    let mut map = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map)) }
        .map_err(|e| format!("Map: {e}"))?;
    let row = w as usize * 4;
    let mut bgra = vec![0u8; row * h as usize];
    for (y, out) in bgra.chunks_exact_mut(row).enumerate() {
        let src = unsafe {
            std::slice::from_raw_parts(map.pData.cast::<u8>().add(y * map.RowPitch as usize), row)
        };
        out.copy_from_slice(src);
    }
    unsafe { ctx.Unmap(&staging, 0) };
    for px in bgra.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    Ok((bgra, w, h))
}

/// Puts `src` of size `sw`×`sh` into `img` at the desktop point `at`; what falls outside is cut.
fn paste(img: &mut Image, src: &[u8], sw: u32, sh: u32, at: (i32, i32)) {
    let (dx, dy) = (at.0 - img.x, at.1 - img.y);
    let (iw, ih) = (img.width as i32, img.height as i32);
    let (x0, x1) = (dx.max(0), (dx + sw as i32).min(iw));
    let (y0, y1) = (dy.max(0), (dy + sh as i32).min(ih));
    if x1 <= x0 {
        return;
    }
    let bytes = (x1 - x0) as usize * 4;
    for y in y0..y1 {
        let d = (y as usize * iw as usize + x0 as usize) * 4;
        let s = ((y - dy) as usize * sw as usize + (x0 - dx) as usize) * 4;
        img.bgra[d..d + bytes].copy_from_slice(&src[s..s + bytes]);
    }
}

/// Replaces the monitor of the foreground window in `img` with what the graphics card shows.
pub fn overlay_foreground_monitor(img: &mut Image) -> Result<(), String> {
    let monitor = unsafe { MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY) };
    let (bgra, w, h, at) = duplicate(monitor)?;
    paste(img, &bgra, w, h, at);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Image {
        Image {
            x: -10,
            y: -10,
            width: 4,
            height: 2,
            bgra: vec![0; 4 * 2 * 4],
        }
    }

    #[test]
    fn paste_lands_on_the_desktop_point_and_clips() {
        let mut img = blank();
        // 2×1 at the desktop point (-9, -9): image cell (1, 1) and (2, 1).
        paste(&mut img, &[1, 1, 1, 1, 2, 2, 2, 2], 2, 1, (-9, -9));
        let px = |x: usize, y: usize| img.bgra[(y * 4 + x) * 4];
        assert_eq!((px(1, 1), px(2, 1), px(0, 1), px(1, 0)), (1, 2, 0, 0));
        let mut img = blank();
        // Sticks out over the right and left edges: no panic, only what fits.
        paste(&mut img, &[7; 4 * 8], 8, 1, (-12, -10));
        assert!(img.bgra[..16].iter().all(|b| *b == 7));
        let mut img = blank();
        paste(&mut img, &[7; 4], 1, 1, (500, 500));
        assert!(img.bgra.iter().all(|b| *b == 0));
    }

    /// `cargo test --release -p snapling-win dup_sees_the_desktop -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dup_sees_the_desktop() {
        crate::set_dpi_aware();
        let t0 = std::time::Instant::now();
        let mon = unsafe { MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY) };
        let (bgra, w, h, at) = duplicate(mon).unwrap();
        println!("{w}x{h} at {at:?} in {} ms", t0.elapsed().as_millis());
        assert_eq!(bgra.len(), (w * h * 4) as usize);
        assert!(bgra.as_chunks::<4>().0.iter().any(|p| p[..3] != [0, 0, 0]));
    }
}
