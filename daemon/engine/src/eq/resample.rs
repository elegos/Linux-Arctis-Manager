use crate::eq::preset::{BandMode, EqPreset};

pub const FIXED_10_HZ: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
pub const FIXED_5_HZ: [f32; 5] = [63.0, 250.0, 1000.0, 4000.0, 16000.0];

pub fn hw_freqs_for_mode(mode: BandMode) -> &'static [f32] {
    match mode {
        BandMode::Fixed10 | BandMode::Parametric10 => &FIXED_10_HZ,
        BandMode::Fixed5 => &FIXED_5_HZ,
    }
}

/// Resample `preset` to gains at `target_freqs`, clamped to `gain_range`.
///
/// Fixed bands are assigned standard frequencies for their mode; parametric
/// bands use their stored frequency.  Interpolation is linear in log2(Hz)
/// space; gains outside the source range are held flat at the edge value.
pub fn resample(preset: &EqPreset, target_freqs: &[f32], gain_range: (f32, f32)) -> Vec<f32> {
    let source = bands_with_freqs(preset);
    target_freqs
        .iter()
        .map(|&tf| interp_log2(&source, tf).clamp(gain_range.0, gain_range.1))
        .collect()
}

fn bands_with_freqs(preset: &EqPreset) -> Vec<(f32, f32)> {
    let default_freqs: &[f32] = match preset.band_mode {
        BandMode::Fixed10 => &FIXED_10_HZ,
        BandMode::Fixed5 => &FIXED_5_HZ,
        BandMode::Parametric10 => &FIXED_10_HZ,
    };
    preset
        .bands
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let freq = match b.frequency {
                Some(f) => f as f32,
                None => *default_freqs.get(i).unwrap_or(&1000.0),
            };
            (freq, b.gain)
        })
        .collect()
}

fn interp_log2(points: &[(f32, f32)], freq: f32) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    let x = freq.max(1.0).log2();
    if x <= points[0].0.max(1.0).log2() {
        return points[0].1;
    }
    let last = points.last().unwrap();
    if x >= last.0.max(1.0).log2() {
        return last.1;
    }
    for w in points.windows(2) {
        let x0 = w[0].0.max(1.0).log2();
        let x1 = w[1].0.max(1.0).log2();
        if x >= x0 && x <= x1 {
            let t = (x - x0) / (x1 - x0);
            return w[0].1 + t * (w[1].1 - w[0].1);
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::preset::{EqBand, FilterType};

    fn fixed10(gains: [f32; 10]) -> EqPreset {
        EqPreset {
            name: "t".into(),
            band_mode: BandMode::Fixed10,
            bands: gains.iter().map(|&g| EqBand::gain_only(g)).collect(),
        }
    }

    fn fixed5(gains: [f32; 5]) -> EqPreset {
        EqPreset {
            name: "t".into(),
            band_mode: BandMode::Fixed5,
            bands: gains.iter().map(|&g| EqBand::gain_only(g)).collect(),
        }
    }

    #[test]
    fn identity_fixed10_to_same_freqs() {
        let gains = [4.0, 3.0, 2.0, 1.0, 0.0, -1.0, -2.0, -3.0, -4.0, -5.0];
        let preset = fixed10(gains);
        let out = resample(&preset, &FIXED_10_HZ, (-10.0, 10.0));
        for (a, b) in gains.iter().zip(out.iter()) {
            assert!((a - b).abs() < 1e-4, "identity mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn flat_stays_flat() {
        let preset = fixed10([0.0; 10]);
        let out = resample(&preset, &FIXED_10_HZ, (-10.0, 10.0));
        assert!(out.iter().all(|&g| g.abs() < 1e-6));
    }

    #[test]
    fn clamp_applied() {
        let gains = [
            15.0, 15.0, 15.0, 15.0, 15.0, -15.0, -15.0, -15.0, -15.0, -15.0,
        ];
        let preset = fixed10(gains);
        let out = resample(&preset, &FIXED_10_HZ, (-10.0, 10.0));
        assert!(out.iter().all(|&g| (-10.0..=10.0).contains(&g)));
    }

    #[test]
    fn parametric_to_fixed10_interpolates() {
        // Single mid-band boost at 1 kHz; adjacent bands should interpolate.
        let freqs: [u16; 10] = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
        let bands: Vec<EqBand> = freqs
            .iter()
            .map(|&f| {
                let gain = if f == 1000 { 6.0 } else { 0.0 };
                EqBand::parametric(f, FilterType::Peaking, gain)
            })
            .collect();
        let preset = EqPreset {
            name: "t".into(),
            band_mode: BandMode::Parametric10,
            bands,
        };
        let out = resample(&preset, &FIXED_10_HZ, (-10.0, 10.0));
        // 1 kHz band (index 5) should be exactly 6 dB.
        assert!((out[5] - 6.0).abs() < 1e-4);
        // Bands at exactly 0 dB in the source stay at 0 dB.
        assert!(out[4].abs() < 1e-4);
        assert!(out[6].abs() < 1e-4);
        // Interpolation between 500 Hz (0 dB) and 1 kHz (6 dB) at 750 Hz → ~3 dB.
        let mid = resample(&preset, &[750.0], (-10.0, 10.0));
        assert!(mid[0] > 0.0 && mid[0] < 6.0);
        // Remote bands should be near 0.
        assert!(out[0].abs() < 1e-4);
        assert!(out[9].abs() < 1e-4);
    }

    #[test]
    fn fixed5_to_fixed10_produces_ten_gains() {
        let preset = fixed5([2.0, 1.0, 0.0, -1.0, -2.0]);
        let out = resample(&preset, &FIXED_10_HZ, (-10.0, 10.0));
        assert_eq!(out.len(), 10);
    }

    #[test]
    fn hw_freqs_for_mode_returns_correct_slice() {
        assert_eq!(hw_freqs_for_mode(BandMode::Fixed10).len(), 10);
        assert_eq!(hw_freqs_for_mode(BandMode::Parametric10).len(), 10);
        assert_eq!(hw_freqs_for_mode(BandMode::Fixed5).len(), 5);
    }

    #[test]
    fn extrapolates_flat_below_source() {
        let preset = fixed10([3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0]);
        let out = resample(&preset, &[1.0], (-10.0, 10.0));
        assert!((out[0] - 3.0).abs() < 1e-4);
    }

    #[test]
    fn extrapolates_flat_above_source() {
        let preset = fixed10([5.0; 10]);
        let out = resample(&preset, &[50000.0], (-10.0, 10.0));
        assert!((out[0] - 5.0).abs() < 1e-4);
    }
}
