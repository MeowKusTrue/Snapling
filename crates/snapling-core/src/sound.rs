//! Feedback sounds synthesized as WAV in memory: no audio files to ship or license.
//! Character follows assets/brand/brand-guide.md: capture — dry, light, crisp;
//! copy — soft and short; error — muted and low.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Capture,
    Copy,
    Error,
}

const RATE: u32 = 44_100;

/// 16-bit mono PCM WAV. `volume` 0–100.
pub fn wav(sound: Sound, volume: u8) -> Vec<u8> {
    let gain = f32::from(volume.min(100)) / 100.0 * 0.6;
    let samples: Vec<f32> = match sound {
        // Shutter: noise burst through a one-pole low-pass, fast exponential decay, ~80 ms.
        Sound::Capture => {
            let n = ms(80);
            let mut seed = 0x1234_5678u32;
            let mut lp = 0.0f32;
            (0..n)
                .map(|i| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let noise = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
                    lp += 0.35 * (noise - lp);
                    let t = i as f32 / n as f32;
                    let click =
                        (2.0 * std::f32::consts::PI * 2200.0 * i as f32 / RATE as f32).sin() * 0.25;
                    (lp + click) * (-t * 9.0).exp()
                })
                .collect()
        }
        // Copy: two quick soft sine tones rising a fifth, ~60 ms.
        Sound::Copy => tone(&[(880.0, 28), (1320.0, 32)], 0.8),
        // Error: two low falling tones, ~140 ms.
        Sound::Error => tone(&[(330.0, 70), (247.0, 70)], 0.9),
    };
    encode(&samples, gain)
}

fn ms(v: u32) -> usize {
    (RATE * v / 1000) as usize
}

fn tone(parts: &[(f32, u32)], level: f32) -> Vec<f32> {
    let mut out = Vec::new();
    for &(freq, dur) in parts {
        let n = ms(dur);
        let attack = ms(4).max(1);
        for i in 0..n {
            let env = (i as f32 / attack as f32).min(1.0) * (1.0 - i as f32 / n as f32).powf(1.5);
            out.push(
                (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin() * env * level,
            );
        }
    }
    out
}

fn encode(samples: &[f32], gain: f32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s * gain).clamp(-1.0, 1.0) * i16::MAX as f32;
        w.extend_from_slice(&(v as i16).to_le_bytes());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(w: &[u8]) -> i16 {
        w[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c).saturating_abs())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn valid_wav_with_brand_durations() {
        for (s, lo, hi) in [
            (Sound::Capture, 65, 95),
            (Sound::Copy, 45, 75),
            (Sound::Error, 110, 160),
        ] {
            let w = wav(s, 80);
            assert_eq!(&w[0..4], b"RIFF");
            assert_eq!(&w[8..16], b"WAVEfmt ");
            let len = u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize;
            assert_eq!(w.len(), 44 + len);
            let dur_ms = len / 2 * 1000 / RATE as usize;
            assert!((lo..=hi).contains(&dur_ms), "{s:?}: {dur_ms} ms");
            assert!(peak(&w) > 1000, "{s:?} is audible");
        }
    }

    #[test]
    fn volume_scales_and_zero_is_silent() {
        assert_eq!(peak(&wav(Sound::Copy, 0)), 0);
        assert!(peak(&wav(Sound::Copy, 100)) > peak(&wav(Sound::Copy, 30)));
        assert!(peak(&wav(Sound::Capture, 100)) < i16::MAX, "no clipping");
    }
}
