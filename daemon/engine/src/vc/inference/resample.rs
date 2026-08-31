// One-shot mono resampling for the RVC pipeline ([E10-S6a]): the
// synthesizer runs at its own native sample rate (model-specific — e.g.
// 40kHz or 48kHz), but the daemon's downstream audio graph is fixed at
// 48kHz, so every hop's synthesizer output is resampled to that fixed rate
// (a no-op when they already match). Replaces
// `torchaudio.functional.resample` — not a bit-exact port, since a
// windowed-sinc resampler's exact numeric behaviour isn't something this
// application's tuned constants depend on (unlike `vc_dsp.rs`'s DSP).
//
// Each call is a fresh, independent one-shot resample (`rubato::Fft::process_all`
// resets internal state first) rather than a continuously-running streaming
// resampler with state carried across hops — this matches `pipeline.py`'s
// own `_resample_audio`, which is a plain stateless function called fresh
// every hop.

use audioadapter_buffers::owned::InterleavedOwned;
use rubato::{Fft, FixedSync, Resampler};

/// Resample mono `audio` from `from_sr` to `to_sr`. A no-op (returns a copy)
/// when the rates already match.
pub fn resample(audio: &[f32], from_sr: u32, to_sr: u32) -> Vec<f32> {
    if from_sr == to_sr || audio.is_empty() {
        return audio.to_vec();
    }

    const CHANNELS: usize = 1;
    // FixedSync::Both treats this as a hint only (the crate derives the
    // actual per-call chunk sizes to fit the ratio exactly) — any value in
    // the "few hundred to a few thousand frames" range the crate
    // recommends is fine; cap at the input length for very short hops.
    let chunk_size_hint = audio.len().clamp(1, 1024);

    let input = InterleavedOwned::<f32>::new_from(audio.to_vec(), CHANNELS, audio.len())
        .expect("mono buffer length always matches CHANNELS=1");
    let mut resampler = Fft::<f32>::new(
        from_sr as usize,
        to_sr as usize,
        chunk_size_hint,
        CHANNELS,
        FixedSync::Both,
    )
    .expect("Fft resampler construction with valid, non-zero rates");

    let output = resampler
        .process_all(&input, audio.len(), None)
        .expect("resampling a finite in-memory buffer cannot fail");
    output.take_data()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_is_passthrough() {
        let audio = [0.1f32, -0.2, 0.3];
        assert_eq!(resample(&audio, 48000, 48000), audio);
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(resample(&[], 40000, 48000), Vec::<f32>::new());
    }

    #[test]
    fn upsampling_preserves_a_known_tone_frequency() {
        // A 200 Hz sine at 40 kHz, resampled to 48 kHz, should still read as
        // ~200 Hz — check via zero-crossing count rather than a sample-level
        // reference (rubato is a quality *replacement* for `torchaudio`'s
        // resampler, not a bit-exact port of it — see this module's header).
        let sr_in = 40000u32;
        let freq = 200.0f32;
        let duration_s = 0.05; // 10 real cycles at 200 Hz
        let n_in = (sr_in as f32 * duration_s) as usize;
        let input: Vec<f32> = (0..n_in)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr_in as f32).sin())
            .collect();

        let out = resample(&input, sr_in, 48000);
        assert!(!out.is_empty());
        // Output length should scale with the rate ratio, within rubato's
        // own delay/padding slack (a handful of frames).
        let expected_len = (n_in as f64 * 48000.0 / sr_in as f64) as usize;
        assert!(
            out.len().abs_diff(expected_len) < 50,
            "out.len()={} expected~{}",
            out.len(),
            expected_len
        );

        let zero_crossings = out
            .windows(2)
            .filter(|w| w[0].signum() != w[1].signum())
            .count();
        let expected_crossings = (2.0 * freq * duration_s) as usize; // 2 per cycle
        assert!(
            zero_crossings.abs_diff(expected_crossings) <= 2,
            "zero_crossings={zero_crossings} expected~{expected_crossings}"
        );
    }
}
