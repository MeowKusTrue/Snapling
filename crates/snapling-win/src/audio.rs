//! Sound for MP4 recordings: what the speakers play (WASAPI loopback) and the microphone, each
//! on its own thread, converted by Windows to 48 kHz stereo float and mixed into one track.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient,
    IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, eCapture, eConsole,
    eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
/// Most a source may run ahead of the mix before its oldest sound is dropped.
const BACKLOG: usize = (RATE * CHANNELS / 2) as usize;

type Queue = Arc<Mutex<VecDeque<f32>>>;

struct Source {
    queue: Queue,
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

/// The sound sources of one recording; dropping it stops them.
pub struct Mixer {
    sources: Vec<Source>,
}

impl Mixer {
    /// Opens the wanted sources; one that fails is left out and named in the second value.
    pub fn start(system: bool, mic: bool) -> (Self, Vec<String>) {
        let mut sources = Vec::new();
        let mut failed = Vec::new();
        for (on, loopback) in [(system, true), (mic, false)] {
            if !on {
                continue;
            }
            match Source::start(loopback) {
                Ok(s) => sources.push(s),
                Err(e) => failed.push(e),
            }
        }
        (Self { sources }, failed)
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Exactly `samples` interleaved samples of the mix; a quiet or late source adds silence.
    pub fn take(&self, samples: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; samples];
        for s in &self.sources {
            let Ok(mut q) = s.queue.lock() else { continue };
            let n = samples.min(q.len());
            for (o, v) in out.iter_mut().zip(q.drain(..n)) {
                *o += v;
            }
        }
        for o in &mut out {
            *o = o.clamp(-1.0, 1.0);
        }
        out
    }

    /// Throws away what came in, while the recording is paused.
    pub fn skip(&self) {
        for s in &self.sources {
            if let Ok(mut q) = s.queue.lock() {
                q.clear();
            }
        }
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        for s in self.sources.drain(..) {
            s.stop.store(true, Ordering::Relaxed);
            let _ = s.thread.join();
        }
    }
}

impl Source {
    fn start(loopback: bool) -> Result<Self, String> {
        let queue = Queue::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (q, flag) = (queue.clone(), stop.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let opened = unsafe { open(loopback) };
            let ok = opened.as_ref().map(|_| ()).map_err(|e| e.to_string());
            let _ = tx.send(ok);
            if let Ok((client, capture)) = opened {
                pump(&capture, &q, &flag);
                let _ = unsafe { client.Stop() };
            }
        });
        let what = if loopback {
            snapling_core::i18n::t("системный звук")
        } else {
            snapling_core::i18n::t("микрофон")
        };
        match rx.recv() {
            Ok(Ok(())) => Ok(Self {
                queue,
                stop,
                thread,
            }),
            Ok(Err(e)) => Err(format!("{what}: {e}")),
            Err(_) => Err(what.to_string()),
        }
    }
}

unsafe fn open(loopback: bool) -> windows::core::Result<(IAudioClient, IAudioCaptureClient)> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let flow = if loopback { eRender } else { eCapture };
        let device = devices.GetDefaultAudioEndpoint(flow, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = WAVEFORMATEX {
            wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
            nChannels: CHANNELS as u16,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * CHANNELS * 4,
            nBlockAlign: (CHANNELS * 4) as u16,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let mut flags =
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        if loopback {
            flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
        }
        // 200 ms of buffer, in 100 ns units.
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &format, None)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok((client, capture))
    }
}

/// Moves packets into the queue every 10 ms until stopped.
fn pump(capture: &IAudioCaptureClient, queue: &Queue, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        while let Ok(n) = unsafe { capture.GetNextPacketSize() }
            && n > 0
        {
            let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
            if unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err()
            {
                break;
            }
            let len = (frames * CHANNELS) as usize;
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
            if let Ok(mut q) = queue.lock() {
                if silent || data.is_null() {
                    q.extend(std::iter::repeat_n(0.0, len));
                } else {
                    let s = unsafe { std::slice::from_raw_parts(data as *const f32, len) };
                    q.extend(s);
                }
                let over = q.len().saturating_sub(BACKLOG);
                q.drain(..over);
            }
            let _ = unsafe { capture.ReleaseBuffer(frames) };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    /// `cargo test -p snapling-win audio_probe -- --ignored --nocapture`: opens both sources
    /// for a second and prints how much came in.
    #[test]
    #[ignore]
    fn audio_probe() {
        let (mix, failed) = super::Mixer::start(true, true);
        println!("failed: {failed:?}, sources: {}", mix.sources.len());
        std::thread::sleep(std::time::Duration::from_secs(1));
        for s in &mix.sources {
            println!("queued {}", s.queue.lock().unwrap().len());
        }
    }
}
