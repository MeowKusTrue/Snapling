"""Builds the three Snapling sound packs: synthesis, the studio recording, the freedesktop theme.

python tools/sound_design.py   -> assets/sounds/packs/*.wav (embedded in the app)
Needs assets/sounds/raw/untitled32.wav, which is not in git.
Rules: click 80-90 ms and one main element; copy rises a third; error falls a semitone and lasts
longer; level grows with importance (click quietest); 2-5 kHz tamed; mono; peak at most -1 dB.
"""
import json
import os
import numpy as np
import soundfile as sf
from scipy import signal

SR = 48000
ROOT = os.path.join(os.path.dirname(__file__), '..', 'assets', 'sounds')
OUT = os.path.join(ROOT, 'packs')
rng = np.random.default_rng(7)


def onset(x):
    e = np.sqrt(np.convolve(x**2, np.ones(24) / 24, 'same'))
    return x[max(0, int(np.argmax(e > e.max() * 0.05)) - 24):]


_rec = None


def rec(t, ms=400):
    """Slice of the recording, hum and rumble removed."""
    global _rec
    if _rec is None:
        x, _ = sf.read(os.path.join(ROOT, 'raw', 'untitled32.wav'), dtype='float64', always_2d=True)
        y = x[:, 0] - x[:, 0].mean()
        for h in (1395, 1482, 1562, 8678, 11039):
            b, a = signal.iirnotch(h, h / 12, SR)
            y = signal.filtfilt(b, a, y)
        _rec = hp(y, 60)
    return onset(_rec[int((t - 0.02) * SR):int((t + ms / 1000) * SR)])


def hp(x, f, o=2):
    return signal.sosfiltfilt(signal.butter(o, f, 'hp', fs=SR, output='sos'), x)


def lp(x, f, o=2):
    return signal.sosfiltfilt(signal.butter(o, f, 'lp', fs=SR, output='sos'), x)


def bp(x, a, b):
    return signal.sosfiltfilt(signal.butter(2, [a, b], 'bp', fs=SR, output='sos'), x)


def pitch(x, semi):
    """Tape-style: pitch and speed move together."""
    r = 2 ** (semi / 12)
    return signal.resample(x, max(8, int(len(x) / r)))


def cut(x, ms, fade=0.6):
    n = int(ms / 1000 * SR)
    x = x[:n].copy()
    f = max(8, int(len(x) * fade))
    x[-f:] *= np.cos(np.linspace(0, np.pi / 2, f)) ** 2
    x[:24] *= np.linspace(0, 1, min(24, len(x)))
    return x


def norm(x):
    return x / max(np.abs(x).max(), 1e-9)


def air(ms, a=3000, b=12000):
    x = rng.standard_normal(int(ms / 1000 * SR))
    t = np.arange(len(x)) / len(x)
    return norm(bp(x, a, b)) * np.exp(-6 * t)


def mix(*layers):
    """(signal, offset ms, gain dB)."""
    n = max(int(o / 1000 * SR) + len(x) for x, o, _ in layers)
    out = np.zeros(n)
    for x, o, g in layers:
        s = int(o / 1000 * SR)
        out[s:s + len(x)] += x * 10 ** (g / 20)
    return out


LEVEL = {'snap': -24.0, 'copy': -21.0, 'error': -18.0}


def dip(x, f=3200.0, q=0.9, db=-5.0):
    """Peaking cut: 2-5 kHz is what makes short sounds tiring."""
    w = 2 * np.pi * f / SR
    a = 10 ** (db / 40)
    al = np.sin(w) / (2 * q)
    b = [1 + al * a, -2 * np.cos(w), 1 - al * a]
    c = [1 + al / a, -2 * np.cos(w), 1 - al / a]
    return signal.filtfilt(b, c, x)


def level(x, event):
    """Level by importance, peak at most -1 dB."""
    w = int(0.03 * SR)
    e = np.sqrt(np.convolve(x**2, np.ones(w) / w, 'same')).max()
    x = x * 10 ** (LEVEL[event] / 20) / e
    return x * min(1.0, 10 ** (-1 / 20) / np.abs(x).max())


def master(x, ms, event, tail=0.5):
    """Mono, 120 Hz high-pass, harsh band tamed, soft clip."""
    x = dip(lp(hp(cut(x, ms, tail), 120), 8500))
    return level(np.tanh(norm(x) * 1.2) / np.tanh(1.2), event)


def natural(x, ms, event, tail=0.4):
    """Recording kept as it is: rumble and hiss out, a light touch on the harsh band."""
    return level(dip(lp(hp(cut(x, ms, tail), 160), 14000), db=-2.0), event)


def system(name, ms, event):
    """Freedesktop sound: mono, 48 kHz, silence trimmed, our level."""
    x, sr = sf.read(os.path.join(ROOT, 'system', name), dtype='float64', always_2d=True)
    x = signal.resample_poly(x.mean(1), SR, sr)
    e = np.sqrt(np.convolve(x**2, np.ones(240) / 240, 'same'))
    end = len(e) - int(np.argmax(e[::-1] > e.max() * 0.01))
    return level(cut(onset(x[:end]), ms, 0.25), event)


def soft(f, ms, decay=6.0):
    """Rounded sine with faint harmonics; 4 cents of detune gives slow warm beating."""
    t = np.arange(int(ms / 1000 * SR)) / SR
    d = 2 ** (4 / 1200)
    x = (np.sin(2 * np.pi * f * t) + 0.5 * np.sin(2 * np.pi * f * d * t)
         + 0.12 * np.sin(4 * np.pi * f * t) + 0.04 * np.sin(6 * np.pi * f * t))
    x *= np.exp(-decay * t / t[-1])
    a = int(0.002 * SR)
    x[:a] *= np.linspace(0, 1, a)
    return norm(x)


def pop(f0, f1, ms):
    """Short rising sweep: the soft tick of a confirmed action."""
    t = np.arange(int(ms / 1000 * SR)) / SR
    f = f0 + (f1 - f0) * (t / t[-1]) ** 0.5
    x = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-6 * t / t[-1])
    x[:48] *= np.linspace(0, 1, 48)
    return x


def builds():
    c6, e6, a4, gs4 = 1046.5, 1318.5, 440.0, 415.3
    mirror = norm(rec(30.866, 130))
    closing = norm(rec(29.200, 90))
    tap = norm(rec(125.380, 200))
    knock = norm(rec(143.998, 200))
    knock2 = norm(rec(148.978, 200))
    return {
        'soft': dict(
            title='Мягкий',
            note='Выбранный: округлые чистые тона, самый спокойный.',
            snap=master(mix((pop(520, 880, 60), 0, 0), (lp(air(14, 2000, 9000), 7000), 0, -20)), 85, 'snap'),
            copy=master(mix((soft(c6, 130), 0, -2), (soft(e6, 170), 70, 0)), 240, 'copy'),
            error=master(mix((soft(a4, 170, 5), 0, 0), (soft(gs4, 220, 5), 130, 0)), 350, 'error')),
        # A fast shutter: the mirror and the curtain 60 ms apart, as at 1/250 s.
        'real': dict(
            title='Реалистичный',
            note='Только твоя запись: зеркало и шторка Canon, стакан, стук по дереву.',
            snap=natural(mix((cut(mirror, 58, 0.3), 0, 0), (cut(closing, 70, 0.5), 60, -2)), 130, 'snap'),
            copy=natural(tap, 130, 'copy'),
            error=natural(mix((cut(knock, 120), 0, 0), (cut(knock2, 150), 135, -2)), 285, 'error')),
        'system': dict(
            title='Системный',
            note='Звуки Linux из темы freedesktop, тот же щелчок, что в Noctalia.',
            snap=system('screen-capture.oga', 500, 'snap'),
            copy=system('complete.oga', 700, 'copy'),
            error=system('dialog-warning.oga', 450, 'error')),
    }


def main():
    os.makedirs(OUT, exist_ok=True)
    meta = []
    for key, s in builds().items():
        for ev, title in (('snap', 'Снимок'), ('copy', 'Копирование'), ('error', 'Ошибка')):
            x = s[ev]
            file = f'{key}-{ev}.wav'
            sf.write(os.path.join(OUT, file), x, SR, subtype='PCM_16')
            sp = np.abs(np.fft.rfft(x * np.hanning(len(x))))
            fr = np.fft.rfftfreq(len(x), 1 / SR)
            meta.append(dict(file=file, set=s['title'], note=s['note'], event=ev, title=title,
                             ms=round(len(x) / SR * 1000)))
            print(file, meta[-1]['ms'], 'ms pk', round(20 * np.log10(np.abs(x).max()), 1),
                  'cen', round(float((sp * fr).sum() / sp.sum())))
    with open(os.path.join(ROOT, 'sets.json'), 'w', encoding='utf-8') as f:
        json.dump(dict(items=meta), f, ensure_ascii=False, indent=1)


if __name__ == '__main__':
    main()
