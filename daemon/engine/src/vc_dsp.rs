// RVC pipeline DSP — deterministic signal-processing glue around the three
// neural inference calls (ContentVec, RMVPE, synthesizer).
//
// Direct, function-by-function port of the pure-numpy parts of
// `voice_changer/rvc/pipeline.py`. Every function here is pure (no model
// calls, no streaming state) and is verified against the real Python
// reference's output on fixed test vectors — see docs/voice-changer-rvc-pipeline.md
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
}
