// The `ort`-based ONNX inference engine ([E10-S6a]): loads ContentVec and
// RMVPE sessions and runs them on real windows of audio/mel input.
//
// The synthesizer (VITS-style, per-user `.pth` -> ONNX) is **not** wired up
// yet — the one-shot Python export tool it needs (`ExportableSynth`, see
// docs/voice-changer-rvc-pipeline.md) was prototyped and numerically
// verified in an earlier session but never committed as a durable script,
// so there is currently no `.onnx` synthesizer to load. ContentVec and
// RMVPE, by contrast, are published pre-converted in the project's own
// `elegos/Linux-Arctis-Manager-AI-Models` release and downloaded by
// `vc_base_models.rs`, so both are fully wired and live-verified here.
//
// Runtime dependency: this crate is built with `ort`'s `load-dynamic`
// feature (see daemon/Cargo.toml), so `libonnxruntime.so` is *not* bundled
// or linked at build time — `init_runtime` must be called once, pointed at
// a real shared library, before any `Session` is created. Where the daemon
// obtains that .so for real users is still an open packaging question; for
// local development, `pip install onnxruntime` and point at its bundled
// `onnxruntime/capi/libonnxruntime.so.*` (this is what this module's own
// `#[ignore]`d live tests do, via `LAM_ORT_DYLIB_PATH`).

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

use super::providers;

#[derive(Debug)]
pub struct EngineError(String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<ort::Error> for EngineError {
    fn from(e: ort::Error) -> Self {
        EngineError(e.to_string())
    }
}

impl From<ort::LoadDynamicError> for EngineError {
    fn from(e: ort::LoadDynamicError) -> Self {
        EngineError(e.to_string())
    }
}

/// Initialise the ONNX Runtime environment from a real shared library on
/// disk, with this engine's execution-provider priority order. Must be
/// called once, before any [`Session`] is created. Calling it more than
/// once is harmless (the second call is a silent no-op — see `ort::init`'s
/// `commit()` docs).
pub fn init_runtime(dylib_path: &Path) -> Result<(), EngineError> {
    ort::init_from(dylib_path)?
        .with_execution_providers(providers::build_providers(&providers::PRIORITY_ORDER))
        .commit();
    Ok(())
}

fn load_session(path: &Path) -> Result<Session, EngineError> {
    Ok(Session::builder()?.commit_from_file(path)?)
}

// ── ContentVec ───────────────────────────────────────────────────────────

pub struct ContentVecSession(Session);

impl ContentVecSession {
    pub fn load(path: &Path) -> Result<Self, EngineError> {
        Ok(Self(load_session(path)?))
    }

    /// `wav`: mono float32 audio at 16 kHz. Returns `(features, n_frames)`,
    /// `features` flattened row-major `[n_frames, 768]` — the last
    /// transformer layer, already doubled 50fps -> 100fps by the graph
    /// itself (matches `rmvpe.py`... no, `pipeline.py::_extract_features`'s
    /// `np.repeat(feats, 2, axis=0)` — the ONNX graph's own `features`
    /// output is pre-doubled, confirmed by its `768`-wide, `frames`-tall
    /// real output shape against a known input length during export
    /// verification).
    pub fn extract_features(&mut self, wav: &[f32]) -> Result<(Vec<f32>, usize), EngineError> {
        let input = Tensor::from_array((vec![1i64, wav.len() as i64], wav.to_vec()))?;
        let outputs = self.0.run(ort::inputs! { "wav" => input })?;
        let arr = outputs["features"].try_extract_array::<f32>()?;
        let shape = arr.shape();
        let n_frames = shape[1];
        let flat: Vec<f32> = arr.iter().copied().collect();
        Ok((flat, n_frames))
    }
}

// ── RMVPE ────────────────────────────────────────────────────────────────

pub struct RmvpeSession(Session);

impl RmvpeSession {
    pub fn load(path: &Path) -> Result<Self, EngineError> {
        Ok(Self(load_session(path)?))
    }

    /// `mel`: `[128, n_frames]` row-major (as returned by
    /// `vc::inference::mel::mel_spectrogram`). Returns `(salience, n_frames)`,
    /// `salience` flattened row-major `[n_frames, 360]`.
    ///
    /// The DeepUnet has 5 stride-2 encoder layers, so the graph requires the
    /// frame axis to be a multiple of 32 for its skip-connection concats to
    /// line up — right-pads per mel-channel (not a trailing flat append,
    /// which would corrupt every row but the first) and trims the padded
    /// frames back off the output, exactly like `rmvpe.py::RMVPE.infer`.
    pub fn infer_salience(
        &mut self,
        mel: &[f32],
        n_frames: usize,
    ) -> Result<(Vec<f32>, usize), EngineError> {
        const N_MELS: usize = 128;
        assert_eq!(mel.len(), N_MELS * n_frames);

        let padded_frames = n_frames.max(1).div_ceil(32) * 32;
        let mel_padded = if padded_frames == n_frames {
            mel.to_vec()
        } else {
            let mut out = vec![0.0f32; N_MELS * padded_frames];
            for m in 0..N_MELS {
                out[m * padded_frames..m * padded_frames + n_frames]
                    .copy_from_slice(&mel[m * n_frames..(m + 1) * n_frames]);
            }
            out
        };

        let input =
            Tensor::from_array((vec![1i64, N_MELS as i64, padded_frames as i64], mel_padded))?;
        let outputs = self.0.run(ort::inputs! { "mel" => input })?;
        let arr = outputs["salience"].try_extract_array::<f32>()?;
        debug_assert_eq!(arr.shape()[2], 360);

        // arr is [1, padded_frames, 360] row-major; since the batch dim is
        // 1, a flat iteration already yields frames in order, so trimming
        // the padded tail is just slicing off the first n_frames*360 values.
        let flat: Vec<f32> = arr.iter().copied().collect();
        let trimmed: Vec<f32> = flat[..n_frames * 360].to_vec();
        Ok((trimmed, n_frames))
    }
}

#[cfg(test)]
#[allow(clippy::excessive_precision)] // fixture values pasted verbatim from the Python reference
mod tests {
    use super::*;

    fn dylib_path() -> std::path::PathBuf {
        std::env::var("LAM_ORT_DYLIB_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                panic!(
                    "set LAM_ORT_DYLIB_PATH to a real onnxruntime shared library \
                     (e.g. `pip install onnxruntime` then point at its \
                     onnxruntime/capi/libonnxruntime.so.*)"
                )
            })
    }

    fn models_dir() -> std::path::PathBuf {
        std::env::var("LAM_TEST_MODELS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(std::env::var("HOME").expect("HOME not set"))
                    .join(".config/arctis_manager/models")
            })
    }

    /// Not run by default — needs a real onnxruntime shared library and the
    /// real published ContentVec ONNX model on disk. Run manually with
    /// `LAM_ORT_DYLIB_PATH=... cargo test --bin lam-daemon -- --ignored live_contentvec_matches_python_reference --nocapture`
    /// The input `wav` is a closed-form two-tone signal (no RNG involved),
    /// reproduced bit-for-bit from `gen_ort_reference.py`; reference output
    /// values captured from the real `onnxruntime` Python package (1.29.0)
    /// running the same published `content_vec_best.onnx`.
    #[test]
    #[ignore]
    fn live_contentvec_matches_python_reference() {
        init_runtime(&dylib_path()).expect("init_runtime");
        let mut session = ContentVecSession::load(&models_dir().join("content_vec_best.onnx"))
            .expect("load content_vec_best.onnx");

        let n_samples = 8512usize;
        let wav: Vec<f32> = (0..n_samples)
            .map(|i| {
                let t = i as f32 / 16000.0;
                0.05 * (2.0 * std::f32::consts::PI * 150.0 * t).sin()
                    + 0.01 * (2.0 * std::f32::consts::PI * 837.0 * t + 0.7).sin()
            })
            .collect();
        let expected_wav_head = [
            0.00644218f32,
            0.01150977,
            0.01565000,
            0.01872345,
            0.02070285,
        ];
        for (got, want) in wav.iter().zip(expected_wav_head.iter()) {
            assert!(
                (got - want).abs() < 1e-4,
                "wav input mismatch: {got} vs {want}"
            );
        }

        let (features, n_frames) = session.extract_features(&wav).expect("extract_features");
        assert_eq!(n_frames, 26);
        assert_eq!(features.len(), 26 * 768);

        let frame0_expected = [
            -0.18691345f32,
            0.25318468,
            0.15402710,
            -0.13066056,
            0.05024971,
            0.06438043,
            0.23997840,
            0.10797806,
            0.30149177,
            0.00514719,
        ];
        for (got, want) in features[..10].iter().zip(frame0_expected.iter()) {
            assert!((got - want).abs() < 1e-3, "frame 0: {got} vs {want}");
        }

        let frame13_expected = [
            -0.20932506f32,
            0.29558086,
            0.04970899,
            -0.14002623,
            0.11810461,
            0.05294594,
            0.28503087,
            0.14256932,
            0.25454322,
            0.01335284,
        ];
        let frame13 = &features[13 * 768..13 * 768 + 10];
        for (got, want) in frame13.iter().zip(frame13_expected.iter()) {
            assert!((got - want).abs() < 1e-3, "frame 13: {got} vs {want}");
        }

        let last_expected = [
            -0.21188195f32,
            0.28683445,
            0.14780310,
            -0.08531348,
            0.08067966,
            0.09359311,
            0.22976878,
            0.19099477,
            0.28351271,
            -0.00987115,
        ];
        let last = &features[25 * 768..25 * 768 + 10];
        for (got, want) in last.iter().zip(last_expected.iter()) {
            assert!((got - want).abs() < 1e-3, "last frame: {got} vs {want}");
        }
    }

    /// Not run by default — see `live_contentvec_matches_python_reference`.
    /// The input `mel` is a closed-form two-frequency sin/cos surface (no
    /// RNG), reproduced bit-for-bit from `gen_ort_reference.py`.
    #[test]
    #[ignore]
    fn live_rmvpe_matches_python_reference() {
        init_runtime(&dylib_path()).expect("init_runtime");
        let mut session =
            RmvpeSession::load(&models_dir().join("rmvpe.onnx")).expect("load rmvpe.onnx");

        let n_mels = 128usize;
        let n_frames = 65usize;
        let mut mel = vec![0.0f32; n_mels * n_frames];
        for m in 0..n_mels {
            for t in 0..n_frames {
                let (mf, tf) = (m as f32, t as f32);
                mel[m * n_frames + t] =
                    0.5 * (mf * 0.10 + tf * 0.21).sin() + 0.1 * (mf * 0.05 - tf * 0.13).cos();
            }
        }
        let expected_mel_frame0_head = [
            0.10000000f32,
            0.14979173,
            0.19883507,
            0.24663721,
            0.29271582,
        ];
        for m in 0..5 {
            let got = mel[m * n_frames]; // frame 0 of mel row m
            assert!(
                (got - expected_mel_frame0_head[m]).abs() < 1e-4,
                "mel input mismatch row {m}: {got} vs {}",
                expected_mel_frame0_head[m]
            );
        }

        let (salience, out_frames) = session
            .infer_salience(&mel, n_frames)
            .expect("infer_salience");
        assert_eq!(out_frames, n_frames);
        assert_eq!(salience.len(), n_frames * 360);

        let frame0_expected = [
            0.00014544f32,
            0.00034875,
            0.00047883,
            0.00049502,
            0.00050709,
            0.00075868,
            0.00093448,
            0.00124130,
            0.00188890,
            0.00125307,
        ];
        for (got, want) in salience[..10].iter().zip(frame0_expected.iter()) {
            assert!((got - want).abs() < 1e-4, "frame 0: {got} vs {want}");
        }

        let frame32_expected = [
            0.00005338f32,
            0.00007203,
            0.00013739,
            0.00029290,
            0.00047857,
            0.00075185,
            0.00177601,
            0.00188217,
            0.00285321,
            0.00121403,
        ];
        let frame32 = &salience[32 * 360..32 * 360 + 10];
        for (got, want) in frame32.iter().zip(frame32_expected.iter()) {
            assert!((got - want).abs() < 1e-4, "frame 32: {got} vs {want}");
        }

        let frame64_expected = [
            0.00001022f32,
            0.00001633,
            0.00003392,
            0.00009492,
            0.00010240,
            0.00015455,
            0.00044289,
            0.00055844,
            0.00063756,
            0.00041986,
        ];
        let frame64 = &salience[64 * 360..64 * 360 + 10];
        for (got, want) in frame64.iter().zip(frame64_expected.iter()) {
            assert!((got - want).abs() < 1e-4, "frame 64: {got} vs {want}");
        }
    }
}
