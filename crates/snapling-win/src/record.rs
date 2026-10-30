//! Recording an area of the screen on a thread of its own: GIF through `core::record`, MP4
//! through Media Foundation, which ships with Windows. Long shots live here too: they scroll
//! the window under the area with the wheel and stitch what passes.

use crate::{Image, grab};
use snapling_core::config::RecordFormat;
use snapling_core::i18n::t;
use snapling_core::record::{Added, GifWriter, Stitcher};
use snapling_core::selection::Rect;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Media::MediaFoundation::{
    IMFMediaType, IMFSinkWriter, MF_API_VERSION, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_SDK_VERSION, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video,
    MFSTARTUP_LITE, MFStartup, MFVideoFormat_H264, MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, SetCursorPos, WM_APP};
use windows::core::HSTRING;

/// Sent to the tray thread when a recording or a long shot has something to hand over.
pub const WM_JOB_DONE: u32 = WM_APP + 21;

/// Identifies the calling thread for [`Recording::start`] and [`long_shot`].
pub fn this_thread() -> u32 {
    unsafe { GetCurrentThreadId() }
}

fn wake(thread: u32) {
    let _ = unsafe { PostThreadMessageW(thread, WM_JOB_DONE, WPARAM(0), LPARAM(0)) };
}

const MF_VERSION: u32 = (MF_SDK_VERSION << 16) | MF_API_VERSION;
/// 100 ns units of a sample time.
const TICKS_PER_MS: i64 = 10_000;

struct Mp4 {
    writer: IMFSinkWriter,
    stream: u32,
    width: u32,
    height: u32,
    time: i64,
}

fn video_type(
    subtype: &windows::core::GUID,
    w: u32,
    h: u32,
    fps: u32,
) -> windows::core::Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(w) << 32) | u64::from(h))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, (u64::from(fps) << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
        Ok(t)
    }
}

impl Mp4 {
    /// H.264 takes even sizes only; the caller crops to `width`×`height`.
    fn new(path: &Path, width: u32, height: u32, fps: u32) -> windows::core::Result<Self> {
        unsafe {
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path), None, None)?;
            let out = video_type(&MFVideoFormat_H264, width, height, fps)?;
            let bits = (u64::from(width) * u64::from(height) * u64::from(fps) / 5).max(1_000_000);
            out.SetUINT32(&MF_MT_AVG_BITRATE, bits.min(40_000_000) as u32)?;
            let stream = writer.AddStream(&out)?;
            let input = video_type(&MFVideoFormat_RGB32, width, height, fps)?;
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?;
            writer.SetInputMediaType(stream, &input, None)?;
            writer.BeginWriting()?;
            Ok(Self {
                writer,
                stream,
                width,
                height,
                time: 0,
            })
        }
    }

    /// One frame of `src_width` wide BGRA shown for `ms`.
    fn push(&mut self, bgra: &[u8], src_width: u32, ms: u32) -> windows::core::Result<()> {
        let row = self.width as usize * 4;
        let len = row * self.height as usize;
        unsafe {
            let buffer = MFCreateMemoryBuffer(len as u32)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            let dst = std::slice::from_raw_parts_mut(data, len);
            let stride = src_width as usize * 4;
            for (d, s) in dst.chunks_exact_mut(row).zip(bgra.chunks_exact(stride)) {
                d.copy_from_slice(&s[..row]);
            }
            buffer.Unlock()?;
            buffer.SetCurrentLength(len as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            let span = i64::from(ms) * TICKS_PER_MS;
            sample.SetSampleTime(self.time)?;
            sample.SetSampleDuration(span)?;
            self.time += span;
            self.writer.WriteSample(self.stream, &sample)
        }
    }

    fn finish(self) -> windows::core::Result<()> {
        unsafe { self.writer.Finalize() }
    }
}

enum Sink {
    Gif(GifWriter),
    Mp4(Mp4),
}

fn start_media() -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| e.to_string())
    }
}

pub struct Finished {
    pub path: PathBuf,
    /// The last frame, for the notification.
    pub last: Image,
    pub seconds: u32,
}

pub struct Recording {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<Result<Finished, String>>,
    pub started: Instant,
}

impl Recording {
    /// Records `area` (desktop pixels) into `path` until [`Recording::stop`] or `max_seconds`.
    pub fn start(
        area: Rect,
        format: RecordFormat,
        fps: u32,
        max_seconds: u32,
        path: PathBuf,
        notify: u32,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let res = record(area, format, fps.clamp(1, 60), max_seconds, path, &flag);
            wake(notify);
            res
        });
        Self {
            stop,
            thread,
            started: Instant::now(),
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn finished(&self) -> bool {
        self.thread.is_finished()
    }

    pub fn join(self) -> Result<Finished, String> {
        self.thread
            .join()
            .map_err(|_| t("запись оборвалась").to_string())?
    }
}

fn record(
    area: Rect,
    format: RecordFormat,
    fps: u32,
    max_seconds: u32,
    path: PathBuf,
    stop: &AtomicBool,
) -> Result<Finished, String> {
    let (w, h) = (area.width as u32, area.height as u32);
    let mut sink = match format {
        RecordFormat::Gif => Sink::Gif(GifWriter::new(&path, w, h)?),
        RecordFormat::Mp4 => {
            start_media()?;
            Sink::Mp4(Mp4::new(&path, w & !1, h & !1, fps).map_err(|e| e.to_string())?)
        }
    };
    // The overlay has just closed: its last frame must not open the recording.
    std::thread::sleep(Duration::from_millis(150));
    let step = Duration::from_millis(1000 / u64::from(fps));
    let begin = Instant::now();
    let limit = Duration::from_secs(u64::from(max_seconds.max(1)));
    let mut last = None;
    let mut prev = begin;
    while !stop.load(Ordering::Relaxed) && begin.elapsed() < limit {
        let tick = Instant::now();
        let frame = grab(area.left, area.top, area.width, area.height, true)?;
        let ms = prev.elapsed().as_millis().max(1) as u32;
        prev = tick;
        match &mut sink {
            Sink::Gif(g) => g.push(&frame, ms)?,
            Sink::Mp4(m) => m.push(&frame, w, ms).map_err(|e| e.to_string())?,
        }
        last = Some(frame);
        std::thread::sleep(step.saturating_sub(tick.elapsed()));
    }
    if let Sink::Mp4(m) = sink {
        m.finish().map_err(|e| e.to_string())?;
    }
    let bgra = last.ok_or(t("запись оборвалась"))?;
    Ok(Finished {
        path,
        last: Image {
            x: area.left,
            y: area.top,
            width: w,
            height: h,
            bgra,
        },
        seconds: begin.elapsed().as_secs() as u32,
    })
}

const WHEEL_NOTCH: i32 = 120;
const SCROLL_NOTCHES: i32 = 3;
const SCROLL_WAIT: Duration = Duration::from_millis(180);
/// Rows of a long shot, about 30 screens: the clipboard and most viewers give up beyond it.
const MAX_ROWS: usize = 32_000;

fn wheel_down() {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                mouseData: (-WHEEL_NOTCH * SCROLL_NOTCHES) as u32,
                dwFlags: MOUSEEVENTF_WHEEL,
                ..Default::default()
            },
        },
    };
    unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
}

/// Scrolls the window under `area` and stitches it, on its own thread. Ends at the end of the
/// page, on Esc, or when frames stop lining up. The result arrives through `done`.
pub fn long_shot(
    area: Rect,
    notify: u32,
    done: impl FnOnce(Result<Image, String>) + Send + 'static,
) {
    std::thread::spawn(move || {
        let res = scroll_and_stitch(area);
        done(res);
        wake(notify);
    });
}

fn scroll_and_stitch(area: Rect) -> Result<Image, String> {
    let _ = unsafe { SetCursorPos(area.left + area.width / 2, area.top + area.height / 2) };
    std::thread::sleep(SCROLL_WAIT);
    let shot = || grab(area.left, area.top, area.width, area.height, false);
    let mut stitched = Stitcher::new(&shot()?, area.width as u32, area.height as u32);
    while stitched.rows() < MAX_ROWS {
        if unsafe { GetAsyncKeyState(i32::from(VK_ESCAPE.0)) } < 0 {
            break;
        }
        wheel_down();
        std::thread::sleep(SCROLL_WAIT);
        let moved = matches!(stitched.add(&shot()?), Added::Grew(_));
        if !moved && !grew(&mut stitched, &shot) {
            break;
        }
    }
    let (bgra, width, height) = stitched.finish();
    Ok(Image {
        x: area.left,
        y: area.top,
        width,
        height,
        bgra,
    })
}

/// Pages with smooth scrolling are still moving after the wait: looks again before giving up.
fn grew(stitched: &mut Stitcher, shot: &impl Fn() -> Result<Vec<u8>, String>) -> bool {
    std::thread::sleep(SCROLL_WAIT);
    shot().is_ok_and(|f| matches!(stitched.add(&f), Added::Grew(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mp4_frames_make_a_real_file() {
        let dir = std::env::temp_dir().join(format!("snapling-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.mp4");
        start_media().unwrap();
        let (w, h) = (320u32, 240u32);
        let mut mp4 = Mp4::new(&path, w, h, 15).unwrap();
        for i in 0..15u8 {
            let frame = [i * 16, 80, 200, 255].repeat((w * h) as usize);
            mp4.push(&frame, w, 66).unwrap();
        }
        mp4.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        assert!(bytes.len() > 1000, "{}", bytes.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
