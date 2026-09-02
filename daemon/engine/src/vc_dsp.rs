// RVC pipeline DSP — deterministic signal-processing glue around the three
// neural inference calls (ContentVec, RMVPE, synthesizer).
//
// Direct, function-by-function port of the pure-numpy parts of
// `voice_changer/rvc/pipeline.py`. Every function here is pure (no model
// calls, no streaming state) and is verified against the real Python
// reference's output on fixed test vectors — see docs/voice-changing-feature.md
// for the wider verification strategy.
//
// Not yet wired into anything — this is the first piece of [E10-S6a]'s Rust
// engine; the `ort` sessions and streaming loop land in a later commit.
#![allow(dead_code)]

// F0 <-> mel-scale constants, matching pipeline.py's _F0_MIN/_F0_MAX/_F0_MEL_MIN/_F0_MEL_MAX.
const F0_MIN: f32 = 50.0;
const F0_MAX: f32 = 1100.0;

fn f0_mel_min() -> f32 {
    1127.0 * (1.0 + F0_MIN / 700.0).ln()
}

fn f0_mel_max() -> f32 {
    1127.0 * (1.0 + F0_MAX / 700.0).ln()
}

/// Quantise Hz F0 to the model's 0..255 coarse pitch index (0 = unvoiced).
/// Port of `pipeline.py::_f0_to_coarse`.
pub fn f0_to_coarse(f0: &[f32]) -> Vec<i64> {
    let mel_min = f0_mel_min();
    let mel_max = f0_mel_max();
    f0.iter()
        .map(|&hz| {
            if hz <= 0.0 {
                return 0i64;
            }
            let mel = 1127.0 * (1.0 + hz.max(1e-6) / 700.0).ln();
            let coarse = ((mel - mel_min) * 254.0 / (mel_max - mel_min) + 1.0).clamp(1.0, 255.0);
            coarse.round() as i64
        })
        .collect()
}

/// Interpolate over short unvoiced flickers (gaps of `<= max_gap` frames)
/// inside voiced runs. Port of `pipeline.py::_fill_f0_gaps`.
pub fn fill_f0_gaps(f0: &mut [f32], max_gap: usize) {
    let voiced: Vec<usize> = f0
        .iter()
        .enumerate()
        .filter(|(_, &v)| v > 0.0)
        .map(|(i, _)| i)
        .collect();
    if voiced.len() < 2 {
        return;
    }
    for w in voiced.windows(2) {
        let (a, b) = (w[0], w[1]);
        let gap = b - a - 1;
        if gap > 0 && gap <= max_gap {
            let (va, vb) = (f0[a], f0[b]);
            for (k, idx) in (a + 1..b).enumerate() {
                let t = (k + 1) as f32 / (gap + 1) as f32;
                f0[idx] = va + (vb - va) * t;
            }
        }
    }
}

/// Scale `target`'s volume-envelope *shape* toward `source`'s (RVC WebUI's
/// "rms_mix_rate"). `rate=1` keeps the model's own envelope untouched;
/// `rate=0` makes the output follow the input dynamics exactly.
/// Port of `pipeline.py::_mix_rms`.
pub fn mix_rms(source: &[f32], target: &[f32], rate: f32) -> Vec<f32> {
    if rate >= 0.999 || source.is_empty() || target.is_empty() {
        return target.to_vec();
    }
    const N_FRAMES: usize = 32;

    fn envelope(x: &[f32]) -> Vec<f32> {
        let frame = (x.len() / N_FRAMES).max(1);
        let usable = (x.len() / frame) * frame;
        let mut e: Vec<f32> = x[..usable]
            .chunks_exact(frame)
            .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / frame as f32).sqrt())
            .map(|v| v.max(1e-6))
            .collect();
        let mean: f32 = e.iter().sum::<f32>() / e.len() as f32;
        for v in &mut e {
            *v /= mean;
        }
        e
    }

    fn resample_linear(src: &[f32], out_len: usize) -> Vec<f32> {
        if src.len() == out_len {
            return src.to_vec();
        }
        if out_len == 1 {
            return vec![src[0]];
        }
        (0..out_len)
            .map(|i| {
                let pos = i as f32 / (out_len - 1) as f32 * (src.len() - 1) as f32;
                let lo = pos.floor() as usize;
                let hi = (lo + 1).min(src.len() - 1);
                let frac = pos - lo as f32;
                src[lo] * (1.0 - frac) + src[hi] * frac
            })
            .collect()
    }

    let env_s = envelope(source);
    let env_t = envelope(target);
    let env_s = if env_s.len() != env_t.len() {
        resample_linear(&env_s, env_t.len())
    } else {
        env_s
    };

    let gain: Vec<f32> = env_s
        .iter()
        .zip(env_t.iter())
        .map(|(&s, &t)| (s / t).powf(1.0 - rate).clamp(0.0, 4.0))
        .collect();
    let gain_full = resample_linear(&gain, target.len());

    target
        .iter()
        .zip(gain_full.iter())
        .map(|(&t, &g)| t * g)
        .collect()
}

/// Root-mean-square level of a chunk. Trivial, but used at several call
/// sites in the streaming pipeline (VAD level checks) that all need exactly
/// this and nothing more.
pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

/// One static gain, computed once from the whole recording's own
/// *active-speech* level and applied uniformly — not a real-time/dynamic
/// compressor (unnecessary for an offline calibration render, where the
/// whole file is available upfront), and not naive peak normalization
/// either. Same "active-frame RMS" approach `pipeline.py`/`pipeline.rs`'s
/// own `run_inference` already uses per-window (160-sample frames, only
/// those at ≥20% of the loudest frame's level count toward the reference),
/// just computed once over the full recording instead of per-window —
/// where it can actually help, since the per-window version only ever
/// normalizes windows the VAD gate already let through. A brief loud
/// transient within the file's normal dynamic range (roughly under 5x the
/// quiet baseline — a plosive, a stressed syllable) doesn't skew the
/// reference the way it would skew naive single-sample peak-normalization;
/// one loud enough to exceed that ratio can still dominate it, same known
/// limitation the existing per-window version already has — not full
/// immunity, just a real reduction (see this function's own tests for
/// both the normal case and this documented edge).
///
/// A single global gain never changes the recording's signal-to-noise
/// ratio: genuine silence stays silence (`0 × gain == 0`), and any
/// background noise mixed with real speech is scaled by the exact same
/// factor as the speech itself — it is not a substitute for a noise
/// gate/expander if the problem is audible background noise, only for a
/// recording that is quiet *overall*. What it does fix: several of the
/// pipeline's *absolute* level thresholds (the VAD's `VAD_RMS` floor, the
/// output-gate mask's harsh-knee floor) don't scale with the recording's
/// own loudness, so a quiet recording sits closer to them even during
/// genuine (if quiet) speech — closing the VAD gate, or crushing the
/// output envelope mask, on passages a human would still hear as normal.
///
/// Only ever raises the level (never attenuates an already-healthy
/// recording) and never pushes the true peak sample past 0.98, so a
/// recording that's already loud enough — or one whose peak is already
/// near full-scale — is returned unchanged.
pub fn normalize_input_level(samples: &[f32], target_active_rms: f32, max_gain: f32) -> Vec<f32> {
    const FRAME: usize = 160; // matches run_inference's own active-frame convention
    if samples.len() < FRAME {
        return samples.to_vec();
    }

    let n_frames = samples.len() / FRAME;
    let frame_rms: Vec<f32> = (0..n_frames)
        .map(|i| rms(&samples[i * FRAME..(i + 1) * FRAME]))
        .collect();
    let max_frame = frame_rms.iter().cloned().fold(0.0f32, f32::max);
    if max_frame < 1e-6 {
        return samples.to_vec(); // true digital silence throughout
    }

    let active_threshold = max_frame * 0.2;
    let active: Vec<f32> = frame_rms
        .iter()
        .cloned()
        .filter(|&v| v >= active_threshold)
        .collect();
    let active_rms = if active.is_empty() {
        max_frame
    } else {
        active.iter().sum::<f32>() / active.len() as f32
    };
    if active_rms < 1e-6 {
        return samples.to_vec();
    }

    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let gain = (target_active_rms / active_rms)
        .clamp(1.0, max_gain)
        .min(0.98 / peak);

    if gain <= 1.0 {
        return samples.to_vec();
    }
    samples.iter().map(|&s| s * gain).collect()
}

/// Estimate a recording's ambient noise-floor RMS from its *leading* quiet
/// segment — the moment after the mic opens but before the speaker starts
/// talking, present at the start of nearly every real take, rather than
/// from the whole file (which would be dominated by actual speech).
///
/// Scans 20 ms windows forward from the very start. The first
/// [`BASELINE_WINDOWS`] windows set an initial baseline (their median, not
/// their minimum — one anomalously quiet window, e.g. a mic connecting
/// mid-silence, shouldn't set the reference on its own); every window
/// after that either lets the baseline drift *down* a little further (the
/// room settling) or, once a window's level jumps past `onset_ratio` times
/// the current baseline, is treated as the onset of real speech — an edge,
/// not an absolute level, so this works regardless of how loud or quiet
/// the room happens to be. Returns the RMS of everything before that
/// onset.
///
/// `None` when there's nothing meaningful to measure: the recording is too
/// short to judge, speech (or loud noise) starts almost immediately with
/// no real leading quiet stretch, or that stretch is exact digital
/// silence (a synthetic/muted input, not a real ambient-noise sample).
pub fn detect_leading_noise_floor(samples: &[f32], sr: u32) -> Option<f32> {
    const WINDOW_MS: f32 = 20.0;
    const MAX_SCAN_SECS: f32 = 2.0;
    const ONSET_RATIO: f32 = 4.0;
    const BASELINE_WINDOWS: usize = 5; // first 100ms
                                       // Sanity ceiling for what "quiet room tone" can plausibly look like —
                                       // a real mic's self-noise/ambient room tone essentially never reaches
                                       // this; a recording that never dips below it (uniformly loud from the
                                       // very start, so the relative onset check below never fires) isn't a
                                       // genuine leading-silence case, not "2 seconds of unusually loud room
                                       // tone".
    const MAX_PLAUSIBLE_FLOOR: f32 = 0.02;

    let window = ((WINDOW_MS / 1000.0) * sr as f32).round() as usize;
    if window == 0 {
        return None;
    }
    let max_windows = (((MAX_SCAN_SECS * sr as f32) as usize) / window).max(BASELINE_WINDOWS + 1);
    let n_windows = (samples.len() / window).min(max_windows);
    if n_windows <= BASELINE_WINDOWS {
        return None;
    }

    let window_rms: Vec<f32> = (0..n_windows)
        .map(|i| rms(&samples[i * window..(i + 1) * window]))
        .collect();

    let mut initial: Vec<f32> = window_rms[..BASELINE_WINDOWS].to_vec();
    initial.sort_by(f32::total_cmp);
    let mut baseline = initial[initial.len() / 2];

    let mut onset = n_windows;
    for (i, &r) in window_rms.iter().enumerate().skip(BASELINE_WINDOWS) {
        if baseline > 1e-6 && r > baseline * ONSET_RATIO {
            onset = i;
            break;
        }
        if r < baseline {
            baseline = 0.8 * baseline + 0.2 * r;
        }
    }
    if onset <= BASELINE_WINDOWS {
        return None; // no real leading quiet stretch to measure
    }

    let floor = rms(&samples[..onset * window]);
    (floor > 1e-6 && floor <= MAX_PLAUSIBLE_FLOOR).then_some(floor)
}

/// `Pipeline`-facing VAD/output-gate level thresholds derived from a
/// measured ambient noise-floor RMS (see [`detect_leading_noise_floor`]),
/// with margin above it so real quiet speech still clears them — same
/// purpose as `pipeline.rs`'s hardcoded `VAD_RMS`/gentle-knee-floor
/// constants (tuned once, by ear, on one particular mic/room), but
/// tailored to *this* recording's actual setup instead of assuming it
/// matches whatever that tuning session's setup happened to be. Maps
/// directly onto `vc::inference::pipeline::GateCalibration`'s fields —
/// kept as a separate type here rather than depending on that module, so
/// this stays a pure, dependency-free DSP function; callers convert.
pub struct NoiseFloorCalibration {
    pub vad_rms: f32,
    pub knee_floor: f32,
}

pub fn calibrate_gate_from_noise_floor(noise_floor_rms: f32) -> NoiseFloorCalibration {
    const VAD_MARGIN: f32 = 3.0;
    const KNEE_MARGIN: f32 = 4.0;
    NoiseFloorCalibration {
        vad_rms: noise_floor_rms * VAD_MARGIN,
        knee_floor: noise_floor_rms * KNEE_MARGIN,
    }
}

/// Per-hop soft limiter: linear below `threshold`, tanh-compressed above,
/// bounded to `[-1, 1]`. Inline math from `pipeline.py::convert`'s limiter stage.
pub fn soft_limit(samples: &mut [f32], threshold: f32) {
    if threshold >= 0.999 {
        return;
    }
    const CEIL: f32 = 1.0;
    for s in samples.iter_mut() {
        let a = s.abs();
        if a > threshold {
            *s = s.signum()
                * (threshold + (CEIL - threshold) * ((a - threshold) / (CEIL - threshold)).tanh());
        }
    }
}

/// VTLN formant warp applied to the HuBERT input only: multiplies the
/// apparent frequency of all spectral content by `1/alpha` without changing
/// array length or sample rate (`alpha < 1` shifts formants upward —
/// male→female). Port of `pipeline.py::_vtln_warp`.
pub fn vtln_warp(audio: &[f32], alpha: f32) -> Vec<f32> {
    if (alpha - 1.0).abs() < 0.001 {
        return audio.to_vec();
    }
    let n = audio.len();
    let mut planner = realfft::RealFftPlanner::<f32>::new();

    let fwd = planner.plan_fft_forward(n);
    let mut input = audio.to_vec();
    let mut spectrum = fwd.make_output_vec();
    fwd.process(&mut input, &mut spectrum)
        .expect("vtln_warp: forward FFT length mismatch");

    let n_bins = spectrum.len();
    let mut warped = vec![realfft::num_complex::Complex::new(0.0f32, 0.0f32); n_bins];
    for (k, w) in warped.iter_mut().enumerate() {
        let k_src = (k as f32 * alpha).clamp(0.0, (n_bins - 1) as f32);
        let lo = k_src.floor() as usize;
        let hi = (lo + 1).min(n_bins - 1);
        let frac = k_src - lo as f32;
        *w = spectrum[lo] * (1.0 - frac) + spectrum[hi] * frac;
    }
    // numpy's irfft silently discards the imaginary part of the DC and (for
    // even n) Nyquist bins — there's no valid negative-frequency counterpart
    // to pair them with, so only the real part can contribute to a real
    // output. realfft's C2R planner instead hard-asserts they're already
    // real, so zero them explicitly to match numpy's behaviour bit-for-bit.
    warped[0].im = 0.0;
    if n.is_multiple_of(2) {
        let nyquist = n_bins - 1;
        warped[nyquist].im = 0.0;
    }

    let inv = planner.plan_fft_inverse(n);
    let mut out = inv.make_output_vec();
    inv.process(&mut warped, &mut out)
        .expect("vtln_warp: inverse FFT length mismatch");
    let scale = 1.0 / n as f32;
    out.iter().map(|v| v * scale).collect()
}

/// Normalized autocorrelation peak in the 50-400 Hz pitch band (0..1) — a
/// rough "is this periodic" score used to rescue quiet-but-voiced phrase-final
/// vowels from the relative VAD threshold. Port of `pipeline.py::_voicedness`,
/// computed directly in the time domain (the Python reference uses an
/// FFT-based circular convolution zero-padded to `2n`, which is numerically
/// equivalent to direct linear autocorrelation for lags `< n`).
pub fn voicedness(chunk: &[f32], sr: u32) -> f32 {
    let n = chunk.len();
    if n < (sr / 25) as usize {
        return 0.0;
    }
    let mean = chunk.iter().sum::<f32>() / n as f32;
    let x: Vec<f64> = chunk.iter().map(|&v| (v - mean) as f64).collect();
    let energy: f64 = x.iter().map(|&v| v * v).sum();
    if energy < 1e-8 {
        return 0.0;
    }
    let lo = (sr / 400) as usize;
    let hi = ((sr / 50) as usize).min(n - 1);
    if hi <= lo {
        return 0.0;
    }
    let mut ac_max = f64::MIN;
    for lag in lo..hi {
        let s: f64 = (0..n - lag).map(|i| x[i] * x[i + lag]).sum();
        if s > ac_max {
            ac_max = s;
        }
    }
    (ac_max / (energy + 1e-9)) as f32
}

/// SOLA alignment search: the offset (`0..=seg.len()-tail.len()`) where
/// `seg`'s waveform best correlates (normalized, loudness-independent) with
/// `tail` — the un-drained reserve of the previous hop. Port of the SOLA
/// block in `pipeline.py::convert`.
pub fn sola_offset(seg: &[f32], tail: &[f32]) -> usize {
    let xfade = tail.len();
    assert!(seg.len() >= xfade, "seg must be at least as long as tail");
    let search_len = seg.len() - xfade + 1;
    let mut best_k = 0usize;
    let mut best_val = f64::MIN;
    for k in 0..search_len {
        let mut corr = 0.0f64;
        let mut energy = 0.0f64;
        for i in 0..xfade {
            let s = seg[k + i] as f64;
            corr += s * tail[i] as f64;
            energy += s * s;
        }
        let val = corr / (energy + 1e-8).sqrt();
        if val > best_val {
            best_val = val;
            best_k = k;
        }
    }
    best_k
}

/// Decode RMVPE's per-frame 360-bin salience into (F0 Hz, confidence).
/// `salience` is row-major `[n_frames, n_bins]` (n_bins = 360 in practice).
/// Port of `rmvpe.py::RMVPE._decode`, including the backward-only onset
/// backfill that rescues weak-salience vowel onsets from being marked
/// unvoiced.
#[allow(clippy::excessive_precision)] // matches rmvpe.py's cents formula verbatim
pub fn rmvpe_decode(
    salience: &[f32],
    n_frames: usize,
    n_bins: usize,
    threshold: f32,
) -> (Vec<f32>, Vec<f32>) {
    assert_eq!(salience.len(), n_frames * n_bins);

    let mut cents_pad = vec![0.0f32; n_bins + 8];
    for (i, c) in cents_pad.iter_mut().skip(4).take(n_bins).enumerate() {
        *c = 20.0 * i as f32 + 1997.3794084376191;
    }

    let mut f0 = vec![0.0f32; n_frames];
    let mut peak = vec![0.0f32; n_frames];

    for t in 0..n_frames {
        let row = &salience[t * n_bins..(t + 1) * n_bins];
        let (center_idx, &max_val) = row.iter().enumerate().fold(
            (0usize, &row[0]),
            |acc, (i, v)| if v > acc.1 { (i, v) } else { acc },
        );
        peak[t] = max_val;

        let mut num = 0.0f64;
        let mut den = 0.0f64;
        for d in 0..9usize {
            let idx = center_idx + d; // offset into the (n_bins+8)-padded window
            let sal = if idx >= 4 && idx - 4 < n_bins {
                row[idx - 4]
            } else {
                0.0
            };
            num += sal as f64 * cents_pad[idx] as f64;
            den += sal as f64;
        }
        let cents = if max_val <= threshold {
            0.0
        } else {
            (num / (den + 1e-10)) as f32
        };
        let f0v = 10.0 * 2f32.powf(cents / 1200.0);
        f0[t] = if f0v == 10.0 { 0.0 } else { f0v };
    }

    let weak: Vec<bool> = peak.iter().map(|&p| p > threshold / 3.0).collect();
    for i in 1..n_frames {
        if f0[i] > 0.0 && f0[i - 1] == 0.0 {
            let mut j = i as isize - 1;
            while j >= 0 && (i as isize - j) <= 5 && weak[j as usize] && f0[j as usize] == 0.0 {
                f0[j as usize] = f0[i];
                j -= 1;
            }
        }
    }

    (f0, peak)
}

/// Input-envelope output-gate mask, at sample resolution: fast attack,
/// `gate_release_s` release, carried across hops via `env_last`. Silences the
/// model's out-of-distribution noise floor on true silence while riding
/// through intra-word plosive closures. Port of the gate block in
/// `pipeline.py::convert` (`env_rel[i] = max(env[j]·decay^(i−j))`, expressed
/// there as a cumulative-max trick and here as the equivalent direct
/// recursion `level[i] = max(env[i], level[i-1]·decay)`).
/// Returns `(env_rel, new_env_last)`.
pub fn gate_envelope(
    chunk: &[f32],
    env_last: f32,
    gate_release_s: f32,
    sr: u32,
) -> (Vec<f32>, f32) {
    const BOXCAR: usize = 160;
    let env = boxcar_smooth_same(chunk, BOXCAR);
    let decay = (-1.0f64 / (gate_release_s as f64 * sr as f64)).exp() as f32;
    let mut level = env_last;
    let mut out = Vec::with_capacity(env.len());
    for e in env {
        level = e.max(level * decay);
        out.push(level);
    }
    let last = out.last().copied().unwrap_or(env_last);
    (out, last)
}

/// `numpy.convolve(abs(x), ones(kernel_len) / kernel_len, 'same')`.
fn boxcar_smooth_same(x: &[f32], kernel_len: usize) -> Vec<f32> {
    let n = x.len();
    let weight = 1.0 / kernel_len as f32;
    let full_len = n + kernel_len - 1;
    let mut full = vec![0.0f32; full_len];
    for (i, &xi) in x.iter().enumerate() {
        let xi = xi.abs() * weight;
        for j in 0..kernel_len {
            full[i + j] += xi;
        }
    }
    let start = (kernel_len - 1) / 2;
    full[start..start + n].to_vec()
}

/// Linear resize with PyTorch's `F.interpolate(..., mode='linear',
/// align_corners=True)` semantics: `out[i]` samples input position
/// `i * (src_len-1) / (target_len-1)` (or `0` when `target_len == 1`).
/// Port of `pipeline.py::_resize1d`.
pub fn resize1d(src: &[f32], target_len: usize) -> Vec<f32> {
    if src.len() == target_len {
        return src.to_vec();
    }
    if target_len == 1 {
        return vec![src[0]];
    }
    (0..target_len)
        .map(|i| {
            let pos = i as f32 / (target_len - 1) as f32 * (src.len() - 1) as f32;
            let lo = pos.floor() as usize;
            let hi = (lo + 1).min(src.len() - 1);
            let frac = pos - lo as f32;
            src[lo] * (1.0 - frac) + src[hi] * frac
        })
        .collect()
}

/// RVC WebUI's `filter_radius`: a sliding-window median filter that kills
/// single-frame pitch spikes (glottal bursts / crackle in the NSF source).
/// `radius < 3` is a no-op (matches `pipeline.py`'s `if radius >= 3` guard);
/// an even radius is rounded up to odd, matching `radius |= 1`. Never smears
/// voiced F0 into unvoiced (zero) frames.
pub fn f0_median_filter(f0: &mut [f32], radius: i32) {
    if radius < 3 || f0.len() < radius as usize {
        return;
    }
    let radius = (radius as usize) | 1;
    let pad = radius / 2;
    let n = f0.len();

    // numpy.pad(f0, pad, mode='edge')
    let mut padded = vec![0.0f32; n + 2 * pad];
    padded[pad..pad + n].copy_from_slice(f0);
    for p in &mut padded[..pad] {
        *p = f0[0];
    }
    for p in &mut padded[pad + n..] {
        *p = f0[n - 1];
    }

    let mut smooth = vec![0.0f32; n];
    let mut window: Vec<f32> = Vec::with_capacity(radius);
    for (i, s) in smooth.iter_mut().enumerate() {
        window.clear();
        window.extend_from_slice(&padded[i..i + radius]);
        window.sort_by(|a, b| a.total_cmp(b));
        *s = window[radius / 2]; // radius is always odd, so this is the true median
    }

    for i in 0..n {
        if f0[i] > 0.0 {
            f0[i] = if smooth[i] > 0.0 { smooth[i] } else { f0[i] };
        }
    }
}

/// Pitch-continuity clamp: any voiced frame further than half an octave
/// (`|log2(f0/ref)| > 0.5`) from the running reference is pulled back to it
/// — real pitch never jumps that far in one 10ms frame, so this catches
/// phrase-final creak/octave-tracking errors that would otherwise render as
/// random vocals in the NSF source. `ref_f0` carries across calls
/// (`None` after a VAD gate close, matching `pipeline.py`'s
/// `self._f0_ref = None` on silence) and is updated in place (80/20 EMA
/// toward each accepted frame). Port of the continuity-clamp block in
/// `pipeline.py::_run_inference`.
pub fn f0_continuity_clamp(f0: &mut [f32], ref_f0: &mut Option<f32>) {
    let mut r = *ref_f0;
    for v in f0.iter_mut() {
        if *v <= 0.0 {
            continue;
        }
        match r {
            None => r = Some(*v),
            Some(rf) => {
                if ((*v / rf).log2()).abs() > 0.5 {
                    *v = rf;
                }
            }
        }
        r = Some(0.8 * r.unwrap() + 0.2 * *v);
    }
    *ref_f0 = r;
}

/// Speaker-relative F0 floor for phrase-final fry/creak: frames whose
/// tracked F0 falls below `0.8×` the running modal-pitch anchor (median of
/// the last 15 strongly-voiced windows' medians, `f0_meds`) *and* whose
/// RMVPE confidence is weak, are floored — but only within the trailing
/// voiced run when the window ends in enough unvoiced tail (phrase-final
/// position), never a sustained low note mid-phrase. Port of the floor
/// block in `pipeline.py::_extract_f0`. `f0_meds` is a bounded history
/// (caller maintains the `maxlen = 15` truncation, matching Python's
/// `collections.deque(maxlen=15)`); only windows with `>= 20` voiced frames
/// contribute a new median, matching the "don't let a transient poison the
/// anchor" rationale in the Python comment.
pub fn f0_phrase_final_floor(f0: &mut [f32], f0_conf: Option<&[f32]>, f0_meds: &mut Vec<f32>) {
    let voiced: Vec<f32> = f0.iter().copied().filter(|&v| v > 0.0).collect();
    if voiced.len() >= 20 {
        f0_meds.push(median(&voiced));
    }
    if voiced.is_empty() || f0_meds.len() < 5 {
        return;
    }

    let anchor = median(f0_meds);
    let floor = (0.8 * anchor).max(55.0);

    let n = f0.len();
    let mut apply = vec![false; n];
    if let Some(conf) = f0_conf {
        if conf.len() == n {
            for i in 0..n {
                apply[i] = conf[i] < 0.10;
            }
        }
    }

    if let Some(run_end) = (0..n).rev().find(|&i| f0[i] > 0.0) {
        if n - 1 - run_end >= 10 {
            let mut run_start = run_end;
            while run_start > 0 && f0[run_start - 1] > 0.0 {
                run_start -= 1;
            }
            run_start = run_start.max(run_end.saturating_sub(30));
            for a in &mut apply[run_start..=run_end] {
                *a = true;
            }
        }
    }

    for i in 0..n {
        if f0[i] > 0.0 && f0[i] < floor && apply[i] {
            f0[i] = floor;
        }
    }
}

fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

#[cfg(test)]
#[allow(clippy::excessive_precision)] // fixture values pasted verbatim from the Python reference
mod tests {
    use super::*;

    // ── f0_to_coarse — reference values from pipeline.py::_f0_to_coarse ────

    #[test]
    fn f0_to_coarse_matches_python_reference() {
        let f0 = [0.0f32, 50.0, 100.0, 220.0, 440.0, 1100.0, 1.0];
        let expected: Vec<i64> = vec![0, 1, 20, 60, 122, 255, 1];
        assert_eq!(f0_to_coarse(&f0), expected);
    }

    #[test]
    fn f0_to_coarse_zero_is_always_zero() {
        assert_eq!(f0_to_coarse(&[0.0]), vec![0]);
    }

    // ── fill_f0_gaps — reference values from pipeline.py::_fill_f0_gaps ────

    #[test]
    fn fill_f0_gaps_bridges_short_gap_matches_python() {
        let mut f0 = [100.0f32, 105.0, 0.0, 0.0, 110.0, 0.0, 0.0, 0.0, 0.0, 120.0];
        fill_f0_gaps(&mut f0, 3);
        let expected = [
            100.0f32,
            105.0,
            106.66666412,
            108.33333588,
            110.0,
            0.0,
            0.0,
            0.0,
            0.0,
            120.0,
        ];
        for (got, want) in f0.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-3, "{got} vs {want}");
        }
    }

    #[test]
    fn fill_f0_gaps_bridges_max_gap_exactly_matches_python() {
        let mut f0 = [100.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 120.0];
        fill_f0_gaps(&mut f0, 8);
        let expected = [
            100.0f32,
            102.22222137,
            104.44444275,
            106.66666412,
            108.8888855,
            111.1111145,
            113.33333588,
            115.55555725,
            117.77777863,
            120.0,
        ];
        for (got, want) in f0.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-3, "{got} vs {want}");
        }
    }

    #[test]
    fn fill_f0_gaps_leaves_gap_longer_than_max_untouched() {
        let mut f0 = [100.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 120.0];
        fill_f0_gaps(&mut f0, 3); // gap = 5 > max_gap = 3
        assert_eq!(f0, [100.0, 0.0, 0.0, 0.0, 0.0, 0.0, 120.0]);
    }

    #[test]
    fn fill_f0_gaps_noop_with_fewer_than_two_voiced_frames() {
        let mut f0 = [0.0f32, 0.0, 100.0, 0.0, 0.0];
        let before = f0;
        fill_f0_gaps(&mut f0, 8);
        assert_eq!(f0, before);
    }

    // ── mix_rms — reference values from pipeline.py::_mix_rms ──────────────

    fn mix_rms_fixture() -> (Vec<f32>, Vec<f32>) {
        // Same seeded vectors as gen_dsp_vectors.py (numpy RandomState(42)).
        let source = vec![
            -0.012545988,
            0.04507143,
            0.023199392,
            0.00986585,
            -0.034398135,
            -0.03440055,
            -0.04419164,
            0.036617614,
            0.010111499,
            0.02080726,
            -0.04794155,
            0.046990987,
            0.033244263,
            -0.02876609,
            -0.031817503,
            -0.03165955,
            -0.019575775,
            0.0024756433,
            -0.006805498,
            -0.020877088,
            0.011185288,
            -0.036050614,
            -0.020785535,
            -0.013363815,
            -0.0043930025,
            0.028517598,
            -0.030032624,
            0.0014234424,
            0.009241456,
            -0.045354959,
            0.010754484,
            -0.032947589,
            -0.043494843,
            0.044888556,
            0.046563204,
            0.030839736,
            -0.019538624,
            -0.04023279,
            0.018423302,
            -0.005984751,
            -0.037796177,
            -0.00048230888,
            -0.046561148,
            0.04093204,
            -0.024122003,
            0.016252225,
            -0.018828893,
            0.0020068050,
            0.0046710256,
            -0.031514555,
            0.046958465,
            0.027513284,
            0.043949898,
            0.039482739,
            0.0097899977,
            0.042187423,
            -0.041150749,
            -0.030401712,
            -0.045477271,
            -0.017466968,
            -0.011132270,
            -0.022865096,
            0.03287375,
            -0.014324668,
        ];
        let target = vec![
            -0.17525239,
            0.034156848,
            -0.28726062,
            0.24175759,
            -0.34035951,
            0.38950953,
            0.2177958,
            -0.24102746,
            -0.39558229,
            0.25236917,
            0.16548586,
            0.18320575,
            0.21701626,
            -0.34076428,
            -0.11322742,
            -0.30730477,
            0.29048276,
            0.098638490,
            -0.13528159,
            -0.34915334,
            -0.15121415,
            -0.13985334,
            0.18368493,
            0.11004596,
            0.3097702,
            -0.022228051,
            -0.3043246,
            0.17059584,
            0.20862804,
            0.049021769,
            0.21677375,
            -0.0049635172,
            0.018186284,
            -0.057967186,
            -0.37966472,
            -0.31368685,
            -0.37485668,
            0.10912833,
            -0.14851522,
            0.0068565370,
            0.3260532,
            -0.20056622,
            -0.071693659,
            0.20444094,
            -0.21696149,
            -0.33841607,
            -0.16819885,
            -0.27102301,
            0.34375811,
            0.2464963,
            0.10672303,
            0.29716849,
            0.24293767,
            -0.25074396,
            0.31404719,
            0.031473782,
            0.24595213,
            0.31687304,
            -0.14559722,
            -0.31195846,
            -0.21765187,
            -0.058313776,
            0.25441179,
            0.28858447,
        ];
        (source, target)
    }

    #[test]
    fn mix_rms_rate_one_is_passthrough() {
        let (source, target) = mix_rms_fixture();
        assert_eq!(mix_rms(&source, &target, 1.0), target);
    }

    #[test]
    fn mix_rms_rate_half_matches_python_reference() {
        let (source, target) = mix_rms_fixture();
        let out = mix_rms(&source, &target, 0.5);
        // Spot-check a handful of reference values from gen_dsp_vectors.py.
        let expected: &[(usize, f32)] = &[
            (0, -0.25320548),
            (1, 0.037359245),
            (33, -0.13955986),
            (34, -0.55679369),
            (63, 0.24867876),
        ];
        for &(i, want) in expected {
            assert!(
                (out[i] - want).abs() < 1e-3,
                "index {i}: {} vs {want}",
                out[i]
            );
        }
    }

    #[test]
    fn mix_rms_empty_inputs_return_target() {
        assert_eq!(mix_rms(&[], &[1.0, 2.0], 0.5), vec![1.0, 2.0]);
    }

    // ── normalize_input_level ────────────────────────────────────────────

    fn tone(n: usize, amplitude: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 16000.0).sin())
            .collect()
    }

    #[test]
    fn normalize_input_level_boosts_a_quiet_recording() {
        let quiet = tone(16000, 0.02); // active RMS ~0.014, well under target
        let out = normalize_input_level(&quiet, 0.06, 8.0);
        let boosted_rms = rms(&out);
        let original_rms = rms(&quiet);
        assert!(
            boosted_rms > original_rms * 1.5,
            "boosted={boosted_rms} original={original_rms}"
        );
    }

    #[test]
    fn normalize_input_level_leaves_an_already_loud_recording_unchanged() {
        let loud = tone(16000, 0.5); // active RMS ~0.35, already well above target
        let out = normalize_input_level(&loud, 0.06, 8.0);
        assert_eq!(out, loud);
    }

    #[test]
    fn normalize_input_level_leaves_true_silence_unchanged() {
        let silence = vec![0.0f32; 16000];
        let out = normalize_input_level(&silence, 0.06, 8.0);
        assert_eq!(out, silence);
    }

    #[test]
    fn normalize_input_level_never_amplifies_signal_to_noise_ratio() {
        // A quiet "speech" tone plus a constant quiet "noise" floor: after
        // normalization both must have grown by exactly the same factor —
        // a single global gain cannot improve SNR, only raise everything.
        let mut with_noise = tone(16000, 0.02);
        for s in with_noise.iter_mut() {
            *s += 0.001;
        }
        let out = normalize_input_level(&with_noise, 0.06, 8.0);
        let implied_gain = out[100] / with_noise[100];
        for i in [0usize, 4000, 8000, 12000] {
            let g = out[i] / with_noise[i];
            assert!(
                (g - implied_gain).abs() < 1e-4,
                "gain must be uniform: {g} vs {implied_gain} at sample {i}"
            );
        }
    }

    #[test]
    fn normalize_input_level_a_moderate_transient_does_not_suppress_the_boost() {
        // A brief, moderately loud burst (a consonant/plosive — within the
        // 20%-of-loudest active-frame tolerance, i.e. under ~5x the quiet
        // baseline) still lets the quiet majority set the reference and
        // get boosted normally.
        let mut samples = tone(16000, 0.02);
        for s in samples.iter_mut().take(200) {
            *s = 0.04; // 2x the quiet tone's amplitude — a normal loud syllable
        }
        let out = normalize_input_level(&samples, 0.06, 8.0);
        let gain_on_tail = out[10000] / samples[10000];
        assert!(gain_on_tail > 2.0, "gain_on_tail={gain_on_tail}");
    }

    #[test]
    fn normalize_input_level_documents_the_known_limit_on_extreme_transients() {
        // Known, accepted limitation (inherited from the same fixed
        // 20%-of-loudest-frame threshold `pipeline.rs::run_inference`
        // already uses per-window): a transient loud enough to push the
        // quiet majority under that ratio (here 4x — over the ~5x
        // tolerance) gets excluded from the active set same as the quiet
        // content is, so the reference (and thus the gain) ends up set by
        // the transient instead. This is *still* less severe than naive
        // peak-normalization would be here (which references the single
        // loudest *sample*, not an already-averaged frame), but it is not
        // full immunity — documented via this test rather than silently
        // relied upon.
        let mut samples = tone(16000, 0.02);
        for s in samples.iter_mut().take(200) {
            *s = 0.08; // 4x the quiet tone's amplitude
        }
        let out = normalize_input_level(&samples, 0.06, 8.0);
        let gain_on_tail = out[10000] / samples[10000];
        assert!(
            (0.9..=1.1).contains(&gain_on_tail),
            "expected ~no boost (reference dominated by the transient), got gain_on_tail={gain_on_tail}"
        );
    }

    #[test]
    fn normalize_input_level_never_exceeds_peak_safety() {
        let mut samples = tone(16000, 0.02);
        samples[500] = 0.9; // one sample near full-scale
        let out = normalize_input_level(&samples, 0.06, 8.0);
        let peak = out.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak <= 0.98 + 1e-6, "peak={peak}");
    }

    #[test]
    fn normalize_input_level_respects_max_gain_cap() {
        let very_quiet = tone(16000, 0.001);
        let out = normalize_input_level(&very_quiet, 0.06, 3.0);
        let implied_gain = out[100] / very_quiet[100];
        assert!(implied_gain <= 3.0 + 1e-3, "implied_gain={implied_gain}");
    }

    #[test]
    fn normalize_input_level_short_buffer_is_a_noop() {
        let tiny = vec![0.01f32; 50]; // shorter than one 160-sample frame
        assert_eq!(normalize_input_level(&tiny, 0.06, 8.0), tiny);
    }

    // ── detect_leading_noise_floor / calibrate_gate_from_noise_floor ─────

    /// Deterministic pseudo-noise (xorshift32) in `[-amplitude, amplitude]`
    /// — a flat tone would have zero variance frame-to-frame and isn't a
    /// realistic stand-in for room tone/mic self-noise.
    fn noise(n: usize, amplitude: f32, mut seed: u32) -> Vec<f32> {
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let unit = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                unit * amplitude
            })
            .collect()
    }

    #[test]
    fn detect_leading_noise_floor_measures_the_quiet_prefix_not_the_speech() {
        let sr = 16000;
        let mut samples = noise(sr, 0.0008, 1); // 1s of quiet room tone
        samples.extend(noise(sr, 0.15, 2)); // 1s of "speech"
        let floor = detect_leading_noise_floor(&samples, sr as u32)
            .expect("should find a leading quiet stretch");
        // Should track the quiet segment's own level, not be dragged up by
        // the loud segment that follows.
        assert!(floor < 0.005, "floor={floor}");
        assert!(floor > 0.0001, "floor={floor}");
    }

    #[test]
    fn detect_leading_noise_floor_none_when_loud_from_the_start() {
        let sr = 16000;
        let samples = noise(sr, 0.2, 3); // loud immediately, no quiet prefix
        assert!(detect_leading_noise_floor(&samples, sr as u32).is_none());
    }

    #[test]
    fn detect_leading_noise_floor_none_for_true_silence() {
        let samples = vec![0.0f32; 16000];
        assert!(detect_leading_noise_floor(&samples, 16000).is_none());
    }

    #[test]
    fn detect_leading_noise_floor_none_for_a_too_short_recording() {
        let samples = noise(50, 0.001, 4); // well under one 20ms window
        assert!(detect_leading_noise_floor(&samples, 16000).is_none());
    }

    #[test]
    fn calibrate_gate_from_noise_floor_applies_margins_above_the_floor() {
        let cal = calibrate_gate_from_noise_floor(0.001);
        assert!(
            (cal.vad_rms - 0.003).abs() < 1e-6,
            "vad_rms={}",
            cal.vad_rms
        );
        assert!(
            (cal.knee_floor - 0.004).abs() < 1e-6,
            "knee_floor={}",
            cal.knee_floor
        );
        assert!(
            cal.knee_floor > cal.vad_rms,
            "knee should sit above vad_rms"
        );
    }

    // ── soft_limit ───────────────────────────────────────────────────────

    #[test]
    fn soft_limit_disabled_at_threshold_near_one() {
        let mut s = [0.5f32, 1.5, -1.5];
        soft_limit(&mut s, 1.0);
        assert_eq!(s, [0.5, 1.5, -1.5]);
    }

    #[test]
    fn soft_limit_leaves_values_below_threshold_untouched() {
        let mut s = [0.1f32, -0.2, 0.5];
        soft_limit(&mut s, 0.8);
        assert_eq!(s, [0.1, -0.2, 0.5]);
    }

    #[test]
    fn soft_limit_compresses_above_threshold_and_stays_bounded() {
        let mut s = [0.95f32, -0.95, 2.0, -2.0];
        soft_limit(&mut s, 0.8);
        for v in s {
            assert!(v.abs() <= 1.0, "{v} exceeds [-1,1]");
            assert!(v.abs() > 0.8, "{v} should still carry through the knee");
        }
        assert!(s[0] > 0.0 && s[1] < 0.0);
    }

    // ── vtln_warp — reference values from gen_dsp_vectors2.py::vtln_warp ───

    #[test]
    fn vtln_warp_alpha_one_is_passthrough() {
        let audio = [0.1f32, -0.2, 0.3, -0.4];
        assert_eq!(vtln_warp(&audio, 1.0), audio);
        assert_eq!(vtln_warp(&audio, 1.0004), audio); // within the 0.001 no-op band
    }

    fn vtln_input_fixture() -> Vec<f32> {
        vec![
            0.16905257,
            -0.04659374,
            0.00328202,
            0.04075163,
            -0.07889231,
            0.00020656,
            -0.00008904,
            -0.17547242,
            0.10176580,
            0.06004985,
            -0.06254290,
            -0.01715483,
            0.05052994,
            -0.02613564,
            -0.02427491,
            -0.14532416,
            0.05545804,
            0.01238809,
            0.02744599,
            -0.15265246,
            0.16506998,
            0.01543355,
            -0.03871400,
            0.20290723,
            -0.00453860,
            -0.14506787,
            -0.04052279,
            -0.22883151,
            0.10493965,
            -0.04164743,
            -0.07425535,
            0.10724702,
        ]
    }

    #[test]
    fn vtln_warp_alpha_point_nine_matches_python_reference() {
        let audio = vtln_input_fixture();
        let expected = [
            0.15874134f32,
            -0.04547187,
            0.02190168,
            0.00523577,
            -0.08551087,
            0.06086430,
            -0.15480708,
            0.02029343,
            0.08780135,
            -0.07001247,
            -0.00344816,
            0.02941220,
            -0.01198159,
            -0.05788241,
            -0.01933928,
            0.03428645,
            -0.03587975,
            -0.05558585,
            0.05140277,
            0.01944261,
            -0.07530718,
            0.07374760,
            0.04934485,
            -0.06565049,
            0.19644985,
            -0.09138960,
            -0.06166558,
            -0.15937456,
            -0.05487300,
            0.06661205,
            -0.13648918,
            0.12295064,
        ];
        let out = vtln_warp(&audio, 0.9);
        for (got, want) in out.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-4, "{got} vs {want}");
        }
    }

    #[test]
    fn vtln_warp_alpha_one_point_one_five_matches_python_reference() {
        let audio = vtln_input_fixture();
        let expected = [
            0.26360670f32,
            -0.12843288,
            0.06408871,
            -0.04636750,
            0.05176250,
            -0.10372966,
            0.05543928,
            -0.04265054,
            -0.13865748,
            0.06535684,
            0.05260114,
            0.00073313,
            -0.09979769,
            0.00547247,
            0.04277946,
            -0.00622636,
            0.00204725,
            -0.13561140,
            0.05646693,
            0.08668885,
            -0.06333721,
            0.06101104,
            0.07709470,
            -0.00087060,
            -0.09428741,
            -0.04463105,
            -0.14150153,
            -0.04983757,
            0.14917275,
            -0.19406426,
            0.07623603,
            -0.00673660,
        ];
        let out = vtln_warp(&audio, 1.15);
        for (got, want) in out.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-4, "{got} vs {want}");
        }
    }

    // ── voicedness — reference values from gen_dsp_vectors2.py::voicedness ─

    #[test]
    fn voicedness_periodic_tone_scores_near_one() {
        let sr = 16000u32;
        let t: Vec<f32> = (0..2048).map(|i| i as f32 / sr as f32).collect();
        let chunk: Vec<f32> = t
            .iter()
            .map(|&t| {
                0.3 * (2.0 * std::f32::consts::PI * 150.0 * t).sin()
                    + 0.05 * (2.0 * std::f32::consts::PI * 450.0 * t).sin()
            })
            .collect();
        let v = voicedness(&chunk, sr);
        assert!((v - 0.94750148).abs() < 1e-3, "{v}");
    }

    #[test]
    fn voicedness_silence_is_zero() {
        assert_eq!(voicedness(&[0.0f32; 2048], 16000), 0.0);
    }

    #[test]
    fn voicedness_below_min_length_is_zero() {
        assert_eq!(voicedness(&[0.5f32; 10], 16000), 0.0);
    }

    // ── sola_offset — reference values from gen_dsp_vectors2.py::sola_offset

    #[test]
    fn sola_offset_finds_true_shift_matching_python_reference() {
        let tail = [
            0.0f32,
            0.35355338,
            0.50000000,
            0.35355338,
            0.0,
            -0.35355338,
            -0.50000000,
            -0.35355338,
            -0.0,
            0.35355338,
            0.50000000,
            0.35355338,
            0.0,
            -0.35355338,
            -0.50000000,
            -0.35355338,
        ];
        let seg = [
            -0.01387343f32,
            -0.01856902,
            -0.02136979,
            -0.00411033,
            0.02333992,
            0.00207338,
            -0.01221086,
            0.00329900,
            -0.00310424,
            0.00162806,
            0.31360394,
            0.43698111,
            0.31281957,
            0.00708862,
            -0.31519863,
            -0.43781257,
            -0.32892582,
            0.01048861,
            0.31883061,
            0.45197394,
            0.33072990,
            -0.00122468,
            -0.31554535,
            -0.44348010,
            -0.31157756,
            0.00829843,
            0.01309398,
            0.00549974,
            -0.00417853,
            0.02472352,
            -0.00004873,
            0.00405693,
            -0.00717232,
            -0.02151596,
            -0.00745983,
            0.00097575,
            0.00564315,
            0.00062019,
            0.00829163,
            0.01026050,
        ];
        assert_eq!(sola_offset(&seg, &tail), 9);
    }

    // ── rmvpe_decode — reference values from gen_dsp_vectors2.py::rmvpe_decode

    #[test]
    fn rmvpe_decode_matches_python_reference() {
        // Same fixture construction as gen_dsp_vectors2.py: RandomState(3),
        // frames 0-1 unvoiced, frame 2 weak onset (backfilled from frame 3
        // territory but self-decoded since its own peak clears threshold/3),
        // frames 3-6 a confident voiced run drifting across bins 100-104,
        // frames 7-9 unvoiced. Regenerated here bit-for-bit isn't practical
        // in Rust, so this test hand-builds an equivalent salience grid.
        const T: usize = 10;
        const BINS: usize = 360;
        let mut salience = vec![0.0f32; T * BINS];
        // Deterministic low-noise floor (doesn't need to match Python's RNG
        // exactly — only the peak bin and its magnitude drive the outcome).
        for v in salience.iter_mut() {
            *v = 0.005;
        }
        salience[2 * BINS + 100] = 0.06;
        for (t, b) in [(3, 100usize), (4, 102), (5, 104), (6, 103)] {
            salience[t * BINS + b] = 0.8;
        }
        let (f0, conf) = rmvpe_decode(&salience, T, BINS, 0.05);

        assert_eq!(f0[0], 0.0);
        assert_eq!(f0[1], 0.0);
        assert!(
            f0[2] > 0.0,
            "frame 2 should self-decode (peak 0.06 > thred 0.05)"
        );
        assert!((conf[2] - 0.06).abs() < 1e-6);
        for t in 3..=6 {
            assert!(f0[t] > 0.0, "frame {t} should be voiced");
            assert!((conf[t] - 0.8).abs() < 1e-6);
        }
        assert_eq!(f0[7], 0.0);
        assert_eq!(f0[8], 0.0);
        assert_eq!(f0[9], 0.0);
        // F0 rises with the drifting peak bin (100 -> 102 -> 104), matching
        // the Python reference's 100.79 / 100.61 / 103.02 / 105.35 / 104.12 trend.
        assert!(f0[4] > f0[3]);
        assert!(f0[5] > f0[4]);
    }

    #[test]
    fn rmvpe_decode_onset_backfill_matches_python_reference() {
        // Isolated, exact port of gen_dsp_vectors2.py's fixture (no RNG noise
        // needed: onset backfill only depends on peak/weak/zero booleans).
        const T: usize = 4;
        const BINS: usize = 16;
        let mut salience = vec![0.0f32; T * BINS];
        salience[5] = 0.01; // frame 0, below thred/3 (0.0167): stays unvoiced
        salience[BINS + 5] = 0.03; // frame 1, above thred/3: eligible for backfill
        salience[2 * BINS + 5] = 0.9; // confident onset
        salience[3 * BINS + 5] = 0.9;
        let (f0, _conf) = rmvpe_decode(&salience, T, BINS, 0.05);
        assert_eq!(f0[0], 0.0, "below weak threshold: not backfilled");
        assert!(
            f0[1] > 0.0,
            "weak frame directly preceding onset: backfilled"
        );
        assert_eq!(f0[1], f0[2], "backfill copies the onset's own F0 value");
        assert!(f0[2] > 0.0 && f0[3] > 0.0);
    }

    // ── gate_envelope — reference values from gen_dsp_vectors2.py::gate_envelope

    #[test]
    fn gate_envelope_matches_python_reference() {
        let mut chunk = vec![0.0f32; 512];
        for s in chunk.iter_mut().take(220).skip(200) {
            *s = 0.5;
        }
        let (env_rel, env_last) = gate_envelope(&chunk, 0.1, 0.060, 16000);
        assert_eq!(env_rel.len(), 512);
        let expected_every_32nd = [
            0.09989589f32,
            0.09662091,
            0.09345330,
            0.09038953,
            0.08742622,
            0.08456004,
            0.08178783,
            0.07910651,
            0.07651309,
            0.07400469,
            0.07157853,
            0.06923191,
            0.06696221,
            0.06476694,
            0.06264362,
            0.06058992,
        ];
        for (i, &want) in expected_every_32nd.iter().enumerate() {
            let got = env_rel[i * 32];
            assert!(
                (got - want).abs() < 1e-3,
                "index {}: {got} vs {want}",
                i * 32
            );
        }
        assert!((env_last - 0.05866462).abs() < 1e-3, "{env_last}");
    }

    // ── resize1d — reference values from mix_rms's identical align_corners=True formula

    #[test]
    fn resize1d_same_length_is_passthrough() {
        let src = [1.0f32, 2.0, 3.0];
        assert_eq!(resize1d(&src, 3), src);
    }

    #[test]
    fn resize1d_single_output_takes_first_sample() {
        assert_eq!(resize1d(&[5.0f32, 9.0, 2.0], 1), vec![5.0]);
    }

    #[test]
    fn resize1d_upsamples_with_align_corners() {
        // align_corners=True: endpoints map exactly, interior linearly interpolated.
        let src = [0.0f32, 10.0];
        let out = resize1d(&src, 3);
        assert_eq!(out, vec![0.0, 5.0, 10.0]);
    }

    // ── f0_median_filter — reference values from gen_f0_postproc_vectors.py

    #[test]
    fn f0_median_filter_radius3_matches_python_reference() {
        let mut f0 = [
            0.0f32, 100.0, 500.0, 105.0, 110.0, 0.0, 200.0, 210.0, 90.0, 220.0, 0.0, 0.0, 300.0,
        ];
        f0_median_filter(&mut f0, 3);
        let expected = [
            0.0f32, 100.0, 105.0, 110.0, 105.0, 0.0, 200.0, 200.0, 210.0, 90.0, 0.0, 0.0, 300.0,
        ];
        assert_eq!(f0, expected);
    }

    #[test]
    fn f0_median_filter_radius5_matches_python_reference() {
        let mut f0 = [
            0.0f32, 100.0, 500.0, 105.0, 110.0, 0.0, 200.0, 210.0, 90.0, 220.0, 0.0, 0.0, 300.0,
        ];
        f0_median_filter(&mut f0, 5);
        let expected = [
            0.0f32, 100.0, 105.0, 105.0, 110.0, 0.0, 110.0, 200.0, 200.0, 90.0, 0.0, 0.0, 300.0,
        ];
        assert_eq!(f0, expected);
    }

    #[test]
    fn f0_median_filter_radius_below_3_is_noop() {
        let mut f0 = [100.0f32, 500.0, 105.0];
        let before = f0;
        f0_median_filter(&mut f0, 1);
        assert_eq!(f0, before);
    }

    // ── f0_continuity_clamp — reference values from gen_f0_postproc_vectors.py

    #[test]
    fn f0_continuity_clamp_matches_python_reference() {
        let mut f0 = [150.0f32, 155.0, 0.0, 400.0, 160.0, 165.0, 40.0];
        let mut ref_f0 = None;
        f0_continuity_clamp(&mut f0, &mut ref_f0);
        let expected = [150.0f32, 155.0, 0.0, 151.0, 160.0, 165.0, 155.24];
        for (got, want) in f0.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-2, "{got} vs {want}");
        }
        assert!((ref_f0.unwrap() - 155.24).abs() < 1e-2);
    }

    #[test]
    fn f0_continuity_clamp_carries_ref_across_calls() {
        let mut f0 = [170.0f32, 0.0, 500.0];
        let mut ref_f0 = Some(158.61417995392074f32);
        f0_continuity_clamp(&mut f0, &mut ref_f0);
        let expected = [170.0f32, 0.0, 160.89134396];
        for (got, want) in f0.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-2, "{got} vs {want}");
        }
        assert!((ref_f0.unwrap() - 160.8913439631366).abs() < 1e-2);
    }

    // ── f0_phrase_final_floor — reference values from gen_f0_postproc_vectors.py

    #[test]
    fn f0_phrase_final_floor_matches_python_reference() {
        // Same fixture construction as gen_f0_postproc_vectors.py: 50 voiced
        // frames around ~180Hz (seeded with numpy RandomState(5)), then a
        // 15-frame decaying, low-confidence tail — hand-transcribed input
        // values from the reference script's own printed dump so this test
        // doesn't need to reproduce numpy's RNG.
        const N: usize = 65;
        let mut f0 = vec![0.0f32; N];
        #[rustfmt::skip]
        let f0_45_65 = [
            182.58392677f32, 182.27868596, 180.98888080, 179.32732748, 179.79877131,
            150.0, 142.14285714, 134.28571429, 126.42857143, 118.57142857,
            110.71428571, 102.85714286, 95.0, 87.14285714, 79.28571429,
            71.42857143, 63.57142857, 55.71428571, 47.85714286, 40.0,
        ];
        f0[45..65].copy_from_slice(&f0_45_65);
        // Frames 0..45 just need to be voiced (>=20 frames triggers the
        // anchor-update push) with a known median — set to a single
        // constant so the median of 45 identical (odd count) values is
        // exactly that constant, matching the Python fixture's actual
        // noisy-data median (179.38829395766675) bit-for-bit, so the
        // appended f0_meds entry — and thus the floor — matches exactly.
        for v in f0.iter_mut().take(45) {
            *v = 179.38829395766675;
        }

        let mut f0_conf = vec![0.5f32; N];
        for c in f0_conf.iter_mut().take(65).skip(50) {
            *c = 0.05;
        }

        let mut f0_meds = vec![180.0f32, 182.0, 178.0, 181.0, 179.0];
        f0_phrase_final_floor(&mut f0, Some(&f0_conf), &mut f0_meds);

        // The floor plateau (frames 51..65) is insensitive to the exact
        // frame-0..45 values (only f0_meds' median drives it), so assert it
        // exactly; frames 45..51 (untouched, above the floor) are skipped
        // since this test's frames 0..45 deliberately diverge from the
        // Python fixture's noisy ones.
        let expected_floor = 143.75531758f32;
        for &v in &f0[51..65] {
            assert!((v - expected_floor).abs() < 1e-2, "{v} vs {expected_floor}");
        }
        assert_eq!(f0_meds.len(), 6, "a new median should have been appended");
    }

    #[test]
    fn f0_phrase_final_floor_noop_with_few_voiced_frames() {
        let mut f0 = vec![0.0f32; 10];
        f0[0] = 100.0;
        let before = f0.clone();
        let mut f0_meds = vec![100.0f32; 5];
        f0_phrase_final_floor(&mut f0, None, &mut f0_meds);
        assert_eq!(f0, before);
    }
}
