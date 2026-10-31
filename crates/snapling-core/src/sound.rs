//! Feedback sounds: three packs of 16-bit mono WAV embedded in the program, built by
//! `tools/sound_design.py`. System pack: freedesktop sound theme, see THIRD_PARTY_NOTICES.md.

use crate::config::{SelectSound, SoundCfg, SoundPack};
use crate::i18n::{t, tf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Capture,
    Copy,
    Error,
    /// A sample of `[sound] select`, for the settings window.
    Select,
}

macro_rules! pack {
    ($name:literal) => {
        [
            include_bytes!(concat!("../../../assets/sounds/packs/", $name, "-snap.wav")).as_slice(),
            include_bytes!(concat!("../../../assets/sounds/packs/", $name, "-copy.wav")).as_slice(),
            include_bytes!(concat!(
                "../../../assets/sounds/packs/",
                $name,
                "-error.wav"
            ))
            .as_slice(),
        ]
    };
}

const SOFT: [&[u8]; 3] = pack!("soft");
const REAL: [&[u8]; 3] = pack!("real");
const SYSTEM: [&[u8]; 3] = pack!("system");

/// Canonical RIFF header in front of the samples.
const HEADER: usize = 44;

/// The sound scaled by `volume` 0–100.
pub fn wav(pack: SoundPack, sound: Sound, volume: u8) -> Vec<u8> {
    let set = match pack {
        SoundPack::Soft => SOFT,
        SoundPack::Real => REAL,
        SoundPack::System => SYSTEM,
    };
    let mut w = set[sound as usize].to_vec();
    let gain = f32::from(volume.min(100)) / 100.0;
    for s in w[HEADER..].as_chunks_mut::<2>().0 {
        *s = ((f32::from(i16::from_le_bytes(*s)) * gain) as i16).to_le_bytes();
    }
    w
}

/// Own files larger than this are not short feedback sounds.
const MAX_FILE: usize = 10 * 1024 * 1024;

/// Where the sample data of a WAV lies, and its format tag and bit depth.
fn wav_data(w: &[u8]) -> Result<(std::ops::Range<usize>, u16, u16), String> {
    if w.len() < 12 || &w[..4] != b"RIFF" || &w[8..12] != b"WAVE" {
        return Err(t("это не файл WAV").into());
    }
    let mut at = 12;
    let mut format = None;
    while at + 8 <= w.len() {
        let size = u32::from_le_bytes([w[at + 4], w[at + 5], w[at + 6], w[at + 7]]) as usize;
        let body = at + 8;
        let end = body.saturating_add(size).min(w.len());
        match &w[at..at + 4] {
            b"fmt " if end - body >= 16 => {
                let tag = u16::from_le_bytes([w[body], w[body + 1]]);
                let bits = u16::from_le_bytes([w[body + 14], w[body + 15]]);
                // WAVE_FORMAT_EXTENSIBLE keeps the real tag in the first bytes of its GUID.
                let tag = if tag == 0xFFFE && end - body >= 26 {
                    u16::from_le_bytes([w[body + 24], w[body + 25]])
                } else {
                    tag
                };
                format = Some((tag, bits));
            }
            b"data" => {
                let (tag, bits) = format.ok_or(t("в файле WAV нет описания формата"))?;
                return Ok((body..end, tag, bits));
            }
            _ => {}
        }
        at = body.saturating_add(size + (size & 1));
    }
    Err(t("в файле WAV нет звука").into())
}

/// Own WAV scaled by `volume`: integer PCM of 16, 24, 32 bits or 32-bit float.
pub fn scale_wav(mut w: Vec<u8>, volume: u8) -> Result<Vec<u8>, String> {
    let (data, tag, bits) = wav_data(&w)?;
    let gain = f64::from(volume.min(100)) / 100.0;
    let body = &mut w[data];
    match (tag, bits) {
        (1, 16) => {
            for s in body.as_chunks_mut::<2>().0 {
                *s = ((f64::from(i16::from_le_bytes(*s)) * gain) as i16).to_le_bytes();
            }
        }
        (1, 24) => {
            for s in body.as_chunks_mut::<3>().0 {
                let v = i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8;
                let b = ((f64::from(v) * gain) as i32).to_le_bytes();
                *s = [b[0], b[1], b[2]];
            }
        }
        (1, 32) => {
            for s in body.as_chunks_mut::<4>().0 {
                *s = ((f64::from(i32::from_le_bytes(*s)) * gain) as i32).to_le_bytes();
            }
        }
        (3, 32) => {
            for s in body.as_chunks_mut::<4>().0 {
                *s = ((f64::from(f32::from_le_bytes(*s)) * gain) as f32).to_le_bytes();
            }
        }
        _ => {
            return Err(tf(
                "формат WAV не поддерживается: код {tag}, {bits} бит",
                &[("tag", &tag), ("bits", &bits)],
            ));
        }
    }
    Ok(w)
}

/// The sound to play: the user's file if set, else the pack's. An error names the file.
pub fn load(cfg: &SoundCfg, sound: Sound) -> Result<Vec<u8>, String> {
    let file = match sound {
        Sound::Select => return Ok(select_sample(cfg.select, cfg.volume)),
        Sound::Capture => &cfg.capture_file,
        Sound::Copy => &cfg.copy_file,
        Sound::Error => &cfg.error_file,
    };
    let file = file.trim();
    if file.is_empty() {
        return Ok(wav(cfg.pack, sound, cfg.volume));
    }
    let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    if bytes.len() > MAX_FILE {
        return Err(tf("{file}: файл больше 10 МБ", &[("file", &file)]));
    }
    scale_wav(bytes, cfg.volume).map_err(|e| format!("{file}: {e}"))
}

/// Steps of pitch as a selection grows.
pub const SELECT_LEVELS: usize = 12;
const RATE: u32 = 44_100;

/// One grain of the selection sound, 16-bit mono; a higher `level` sounds tighter.
fn select_grain(kind: SelectSound, level: usize) -> Vec<i16> {
    let level = level as f32;
    let rate = RATE as f32;
    let tau = std::f32::consts::TAU;
    let tone = |ms: f32, f: &dyn Fn(f32) -> f32| -> Vec<i16> {
        (0..(ms / 1000.0 * rate) as usize)
            .map(|i| (f(i as f32 / rate).clamp(-1.0, 1.0) * 20_000.0) as i16)
            .collect()
    };
    match kind {
        SelectSound::Off => Vec::new(),
        SelectSound::Ticks => tone(14.0, &|t| {
            (tau * (1900.0 + level * 90.0) * t).sin() * (-t * 420.0).exp()
        }),
        SelectSound::Crackle => {
            // A few sharp impulses at fixed pseudo-random places, like a ratchet catching.
            let mut seed = 0x2F6E_2B1Du32.wrapping_add(level as u32 * 7919);
            let mut out = vec![0i16; (0.022 * rate) as usize];
            for _ in 0..5 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let at = (seed >> 8) as usize % out.len();
                let sign = if seed & 1 == 0 { 1.0 } else { -1.0 };
                for (k, s) in out[at..].iter_mut().take(40).enumerate() {
                    let v = sign * 15_000.0 * (-(k as f32) / 6.0).exp();
                    *s = (f32::from(*s) + v).clamp(-30_000.0, 30_000.0) as i16;
                }
            }
            out
        }
        SelectSound::Rubber => tone(52.0, &|t| {
            let f = 150.0 + level * 34.0 + t * 1600.0;
            let wobble = 1.0 + 0.35 * (tau * 70.0 * t).sin();
            let edge = (t * 400.0).min(1.0) * (1.0 - t / 0.052).max(0.0);
            (tau * f * t).sin() * wobble * edge * 0.9
        }),
    }
}

fn wav_from(samples: &[i16], volume: u8) -> Vec<u8> {
    let gain = f32::from(volume.min(100)) / 100.0;
    let bytes = samples.len() as u32 * 2;
    let mut w = Vec::with_capacity(HEADER + bytes as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + bytes).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&bytes.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&((f32::from(*s) * gain) as i16).to_le_bytes());
    }
    w
}

/// Grains from loose to tight, `None` when the sound is off.
pub fn select_wavs(kind: SelectSound, volume: u8) -> Option<Vec<Vec<u8>>> {
    (kind != SelectSound::Off).then(|| {
        (0..SELECT_LEVELS)
            .map(|l| wav_from(&select_grain(kind, l), volume))
            .collect()
    })
}

/// The grains one after another with pauses, the way a growing selection sounds.
fn select_sample(kind: SelectSound, volume: u8) -> Vec<u8> {
    let gap = vec![0i16; (0.05 * RATE as f32) as usize];
    let samples: Vec<i16> = (0..SELECT_LEVELS)
        .flat_map(|l| select_grain(kind, l).into_iter().chain(gap.iter().copied()))
        .collect();
    wav_from(&samples, volume)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKS: [SoundPack; 3] = [SoundPack::Soft, SoundPack::Real, SoundPack::System];
    const SOUNDS: [Sound; 3] = [Sound::Capture, Sound::Copy, Sound::Error];

    fn peak(w: &[u8]) -> i16 {
        w[HEADER..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c).saturating_abs())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn every_pack_is_canonical_mono_pcm() {
        for p in PACKS {
            for s in SOUNDS {
                let w = wav(p, s, 100);
                assert_eq!(&w[0..4], b"RIFF", "{p:?} {s:?}");
                assert_eq!(&w[36..40], b"data", "{p:?} {s:?}: header is not 44 bytes");
                assert_eq!(u16::from_le_bytes([w[20], w[21]]), 1, "PCM");
                assert_eq!(u16::from_le_bytes([w[22], w[23]]), 1, "mono");
                let len = u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize;
                assert_eq!(w.len(), HEADER + len, "{p:?} {s:?}");
                assert!(peak(&w) > 1000, "{p:?} {s:?} is audible");
            }
        }
    }

    #[test]
    fn volume_scales_and_zero_is_silent() {
        assert_eq!(peak(&wav(SoundPack::Soft, Sound::Copy, 0)), 0);
        let (loud, quiet) = (
            peak(&wav(SoundPack::Soft, Sound::Copy, 100)),
            peak(&wav(SoundPack::Soft, Sound::Copy, 30)),
        );
        assert!(loud > quiet);
    }

    fn file(tag: u16, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut w = b"RIFF\0\0\0\0WAVELIST\x03\0\0\0abc\0fmt \x10\0\0\0".to_vec();
        w.extend_from_slice(&tag.to_le_bytes());
        w.extend_from_slice(&[1, 0, 0x80, 0xBB, 0, 0, 0, 0, 0, 0, 2, 0]);
        w.extend_from_slice(&bits.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(data);
        w
    }

    #[test]
    fn scale_wav_handles_extra_chunks_and_depths() {
        let w = scale_wav(file(1, 16, &1000i16.to_le_bytes()), 50).unwrap();
        assert_eq!(i16::from_le_bytes([w[w.len() - 2], w[w.len() - 1]]), 500);
        let w = scale_wav(file(3, 32, &1.0f32.to_le_bytes()), 25).unwrap();
        let n = w.len();
        assert_eq!(
            f32::from_le_bytes([w[n - 4], w[n - 3], w[n - 2], w[n - 1]]),
            0.25
        );
        let w = scale_wav(file(1, 24, &[0, 0, 0x80]), 50).unwrap();
        assert_eq!(&w[w.len() - 3..], &[0, 0, 0xC0], "negative stays negative");
    }

    #[test]
    fn scale_wav_refuses_garbage_without_panicking() {
        for bad in [
            Vec::new(),
            b"RIFF".to_vec(),
            b"OggS not a wav file at all".to_vec(),
            b"RIFF\0\0\0\0WAVEdata\xFF\xFF\xFF\xFF".to_vec(),
            file(85, 16, &[1, 2]),
        ] {
            assert!(scale_wav(bad, 50).is_err());
        }
    }

    #[test]
    fn missing_own_file_is_an_error_naming_it() {
        let cfg = SoundCfg {
            copy_file: "Z:/нет/такого.wav".into(),
            ..SoundCfg::default()
        };
        assert!(load(&cfg, Sound::Copy).unwrap_err().contains("такого.wav"));
        assert!(
            load(&cfg, Sound::Capture).is_ok(),
            "the others stay on the pack"
        );
    }

    #[test]
    fn packs_differ() {
        let a = wav(SoundPack::Soft, Sound::Capture, 100);
        assert_ne!(a, wav(SoundPack::Real, Sound::Capture, 100));
        assert_ne!(a, wav(SoundPack::System, Sound::Capture, 100));
    }

    #[test]
    fn select_grains_are_audible_wavs_that_change_with_level() {
        assert!(select_wavs(SelectSound::Off, 60).is_none());
        for kind in [
            SelectSound::Ticks,
            SelectSound::Crackle,
            SelectSound::Rubber,
        ] {
            let wavs = select_wavs(kind, 100).unwrap();
            assert_eq!(wavs.len(), SELECT_LEVELS);
            assert_ne!(wavs[0], wavs[SELECT_LEVELS - 1]);
            for w in &wavs {
                let (data, tag, bits) = wav_data(w).unwrap();
                assert_eq!((tag, bits), (1, 16));
                let body = &w[data];
                assert!(
                    body.as_chunks::<2>()
                        .0
                        .iter()
                        .any(|s| i16::from_le_bytes(*s) != 0)
                );
            }
            let quiet = select_wavs(kind, 0).unwrap();
            assert!(quiet[0][HEADER..].iter().all(|b| *b == 0));
        }
    }
}
