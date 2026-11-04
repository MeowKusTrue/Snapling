//! Recording an area of the screen on a thread of its own: GIF through `core::record`, MP4
//! through Media Foundation, which ships with Windows. Long shots live here too: they scroll
//! the window under the area with the wheel and stitch what passes.

use crate::audio::{self, Mixer};
use crate::{Image, grab};
use snapling_core::config::{Record, RecordFormat};
use snapling_core::i18n::t;
use snapling_core::record::{Added, GifWriter, Stitcher};
use snapling_core::selection::Rect;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
use windows::Win32::Media::MediaFoundation::{
    IMFMediaType, IMFSinkWriter, MF_API_VERSION, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_SDK_VERSION, MF_TRANSCODE_CONTAINERTYPE, MFAudioFormat_AAC,
    MFAudioFormat_PCM, MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFMediaType_Audio, MFMediaType_Video, MFSTARTUP_LITE, MFStartup,
    MFTranscodeContainerType_FMPEG4, MFVideoFormat_H264, MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, PostThreadMessageW, SetCursorPos, WM_APP,
};
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

struct Mp4 {
    writer: IMFSinkWriter,
    stream: u32,
    width: u32,
    height: u32,
    /// The sound track, when there is one.
    sound: Option<u32>,
    /// Sound frames written so far.
    heard: u64,
}

/// 16-bit PCM in, or AAC out at 192 kbit/s.
fn audio_type(aac: bool) -> windows::core::Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        let sub = if aac {
            MFAudioFormat_AAC
        } else {
            MFAudioFormat_PCM
        };
        t.SetGUID(&MF_MT_SUBTYPE, &sub)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, audio::RATE)?;
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, audio::CHANNELS)?;
        let bytes = if aac {
            24_000
        } else {
            audio::RATE * audio::CHANNELS * 2
        };
        t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes)?;
        if !aac {
            t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, audio::CHANNELS * 2)?;
        }
        Ok(t)
    }
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
    fn new(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        quality: u8,
        sound: bool,
    ) -> windows::core::Result<Self> {
        unsafe {
            // Fragmented MP4: what is written stays playable if Snapling or Windows goes down.
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1)?;
            let attrs = attrs.ok_or_else(windows::core::Error::empty)?;
            attrs.SetGUID(
                &MF_TRANSCODE_CONTAINERTYPE,
                &MFTranscodeContainerType_FMPEG4,
            )?;
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path), None, &attrs)?;
            let out = video_type(&MFVideoFormat_H264, width, height, fps)?;
            // Bits per pixel per frame: 0.1, 0.2, 0.4.
            let pixels = u64::from(width) * u64::from(height) * u64::from(fps);
            let bits = (pixels << quality.min(2)) / 10;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bits.clamp(500_000, 60_000_000) as u32)?;
            let stream = writer.AddStream(&out)?;
            let input = video_type(&MFVideoFormat_RGB32, width, height, fps)?;
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?;
            writer.SetInputMediaType(stream, &input, None)?;
            let sound = if sound {
                let track = writer.AddStream(&audio_type(true)?)?;
                writer.SetInputMediaType(track, &audio_type(false)?, None)?;
                Some(track)
            } else {
                None
            };
            writer.BeginWriting()?;
            Ok(Self {
                writer,
                stream,
                width,
                height,
                sound,
                heard: 0,
            })
        }
    }

    /// One frame of `src_width` wide BGRA, taken `at` into the recording, shown for `span`.
    fn push(
        &mut self,
        bgra: &[u8],
        src_width: u32,
        at: Duration,
        span: Duration,
    ) -> windows::core::Result<()> {
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
            sample.SetSampleTime((at.as_nanos() / 100) as i64)?;
            sample.SetSampleDuration((span.as_nanos() / 100) as i64)?;
            self.writer.WriteSample(self.stream, &sample)
        }
    }

    /// Interleaved float sound, right after what was written before.
    fn push_sound(&mut self, samples: &[f32]) -> windows::core::Result<()> {
        let Some(track) = self.sound else {
            return Ok(());
        };
        let frames = (samples.len() / audio::CHANNELS as usize) as u64;
        if frames == 0 {
            return Ok(());
        }
        let bytes: Vec<u8> = samples
            .iter()
            .flat_map(|v| ((v * 32767.0) as i16).to_le_bytes())
            .collect();
        let at = |n: u64| (n * 10_000_000 / u64::from(audio::RATE)) as i64;
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(at(self.heard))?;
            sample.SetSampleDuration(at(self.heard + frames) - at(self.heard))?;
            self.heard += frames;
            self.writer.WriteSample(track, &sample)
        }
    }

    /// Brings the sound up to `active` of recording, keeping `lag` for late packets.
    fn catch_up(&mut self, mix: &Mixer, active: Duration, lag: Duration) -> Result<(), String> {
        let want = (active.saturating_sub(lag).as_secs_f64() * f64::from(audio::RATE)) as u64;
        let frames = want.saturating_sub(self.heard) as usize;
        self.push_sound(&mix.take(frames * audio::CHANNELS as usize))
            .map_err(|e| e.to_string())
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
    /// Sound sources that could not be opened.
    pub sound_errors: Vec<String>,
    /// The last frame, for the notification.
    pub last: Image,
    pub seconds: u32,
}

/// What the tray asks of the recording thread.
#[derive(Default)]
struct Flags {
    stop: AtomicBool,
    paused: AtomicBool,
}

pub struct Recording {
    flags: Arc<Flags>,
    /// Stopped to throw the file away.
    discard: AtomicBool,
    thread: std::thread::JoinHandle<Result<Finished, String>>,
    pub started: Instant,
}

impl Recording {
    /// Records `area` (desktop pixels) into `path` until [`Recording::stop`] or `max_seconds`;
    /// 0 has no limit.
    pub fn start(area: Rect, rec: Record, path: PathBuf, notify: u32) -> Self {
        let flags = Arc::new(Flags::default());
        let shared = flags.clone();
        let thread = std::thread::spawn(move || {
            let res = record(area, rec, path, &shared);
            wake(notify);
            res
        });
        Self {
            flags,
            discard: AtomicBool::new(false),
            thread,
            started: Instant::now(),
        }
    }

    pub fn stop(&self) {
        self.flags.stop.store(true, Ordering::Relaxed);
    }

    /// Pauses or goes on; returns whether it is paused now.
    pub fn toggle_pause(&self) -> bool {
        !self.flags.paused.fetch_xor(true, Ordering::Relaxed)
    }

    /// Stops; the caller drops the file when [`Recording::discarded`] says so.
    pub fn cancel(&self) {
        self.discard.store(true, Ordering::Relaxed);
        self.stop();
    }

    pub fn discarded(&self) -> bool {
        self.discard.load(Ordering::Relaxed)
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

fn record(area: Rect, rec: Record, path: PathBuf, flags: &Flags) -> Result<Finished, String> {
    let (stop, paused) = (&flags.stop, &flags.paused);
    let (fps, cursor) = (rec.fps.clamp(1, 60), rec.cursor);
    let (w, h) = (area.width as u32, area.height as u32);
    let mp4 = rec.format == RecordFormat::Mp4;
    // Opened before the start: a device takes a moment to begin.
    let (mix, sound_errors) = Mixer::start(mp4 && rec.system_audio, mp4 && rec.mic);
    let mut sink = match rec.format {
        RecordFormat::Gif => Sink::Gif(GifWriter::new(&path, w, h)?),
        RecordFormat::Mp4 => {
            start_media()?;
            let made = Mp4::new(&path, w & !1, h & !1, fps, rec.quality, !mix.is_empty());
            Sink::Mp4(made.map_err(|e| e.to_string())?)
        }
    };
    // The overlay has just closed: its last frame must not open the recording.
    std::thread::sleep(Duration::from_millis(150));
    mix.skip();
    // Sound is written this far behind, so a late packet still lands in its place.
    let lag = Duration::from_millis(150);
    let step = Duration::from_millis(1000 / u64::from(fps));
    let begin = Instant::now();
    let limit = (rec.max_seconds > 0).then(|| Duration::from_secs(u64::from(rec.max_seconds)));
    let mut last = None;
    let mut prev = begin;
    // Time spent paused: left out of frame lengths, the limit and the length.
    let mut held = Duration::ZERO;
    while !stop.load(Ordering::Relaxed) && limit.is_none_or(|l| begin.elapsed() - held < l) {
        if paused.load(Ordering::Relaxed) {
            let from = Instant::now();
            while paused.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(30));
                mix.skip();
            }
            held += from.elapsed();
            prev += from.elapsed();
            continue;
        }
        let tick = Instant::now();
        let frame = grab(area.left, area.top, area.width, area.height, cursor)?;
        // From the last frame's start to this one's: the grab itself is not counted twice.
        let ms = tick.duration_since(prev).as_millis().max(1) as u32;
        prev = tick;
        match &mut sink {
            Sink::Gif(g) => g.push(&frame, ms)?,
            Sink::Mp4(m) => {
                let at = tick - begin - held;
                m.push(&frame, w, at, step).map_err(|e| e.to_string())?;
                m.catch_up(&mix, begin.elapsed() - held, lag)?;
            }
        }
        last = Some(frame);
        std::thread::sleep(step.saturating_sub(tick.elapsed()));
    }
    if let Sink::Mp4(mut m) = sink {
        m.catch_up(&mix, begin.elapsed() - held, Duration::ZERO)?;
        m.finish().map_err(|e| e.to_string())?;
    }
    let bgra = last.ok_or(t("запись оборвалась"))?;
    Ok(Finished {
        path,
        sound_errors,
        last: Image {
            x: area.left,
            y: area.top,
            width: w,
            height: h,
            bgra,
        },
        seconds: (begin.elapsed() - held).as_secs() as u32,
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
    let mut home = POINT::default();
    let _ = unsafe { GetCursorPos(&mut home) };
    let res = stitch_page(area);
    let _ = unsafe { SetCursorPos(home.x, home.y) };
    res
}

fn stitch_page(area: Rect) -> Result<Image, String> {
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
        let mut mp4 = Mp4::new(&path, w, h, 15, 1, true).unwrap();
        for i in 0..150u8 {
            let frame = [i.wrapping_mul(16), 80, 200, 255].repeat((w * h) as usize);
            let at = Duration::from_millis(66 * u64::from(i));
            mp4.push(&frame, w, at, Duration::from_millis(66)).unwrap();
            mp4.push_sound(&vec![0.1; 2 * 3168]).unwrap();
        }
        // Fragments reach the disk while recording, so a crash keeps them.
        std::thread::sleep(Duration::from_secs(2));
        let early = std::fs::read(&path).unwrap();
        assert!(
            early.windows(4).any(|w| w == b"moof"),
            "nothing on disk before the end"
        );
        mp4.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(4).any(|w| w == b"moof"), "not fragmented");
        assert_eq!(&bytes[4..8], b"ftyp");
        assert!(bytes.len() > 1000, "{}", bytes.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod live {
    /// `cargo test -p snapling-win record_live -- --ignored --nocapture`: 2 s, a 1 s pause,
    /// 2 s more, with sound; prints where the file is.
    #[test]
    #[ignore]
    fn record_live() {
        use snapling_core::config::{Record, RecordFormat};
        let path = std::env::temp_dir().join("snapling-live.mp4");
        let rec = Record {
            format: RecordFormat::Mp4,
            mic: true,
            ..Record::default()
        };
        let area = snapling_core::selection::Rect {
            left: 0,
            top: 0,
            width: 640,
            height: 360,
        };
        let run = super::Recording::start(area, rec, path.clone(), 0);
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(run.toggle_pause());
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(!run.toggle_pause());
        std::thread::sleep(std::time::Duration::from_secs(2));
        run.stop();
        let done = run.join().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        println!(
            "{} s, {} bytes, mp4a {}, errors {:?}, {}",
            done.seconds,
            bytes.len(),
            bytes.windows(4).any(|w| w == b"mp4a"),
            done.sound_errors,
            path.display()
        );
    }
}
