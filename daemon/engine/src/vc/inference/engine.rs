// The `ort`-based ONNX inference engine ([E10-S6a]): loads ContentVec,
// RMVPE, and per-model synthesizer sessions and runs them on real windows of
// audio/mel/feature input.
//
// ContentVec and RMVPE are published pre-converted in the project's own
// `elegos/Linux-Arctis-Manager-AI-Models` release and downloaded by
// `vc_base_models.rs`. The synthesizer (VITS-style) is per-user — converted
// locally, once, by the one-shot offline tool at
// `voice_changer/rvc/export_onnx.py` (the "one Python piece that stays") —
// and takes three externalised-randomness inputs (`prior_noise`,
// `rand_phase`, `source_noise`) that `SynthSession::infer` draws fresh per
// call, matching `SynthesizerTrnMs768NSFsid.infer()`'s own internal
// `torch.randn_like`/`torch.rand` draws. All three sessions are live-verified
// against real `onnxruntime` output in this module's `#[ignore]`d tests.
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
use rand::RngExt;

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
    /// `features` flattened row-major `[n_frames, 768]`. The ONNX graph
    /// itself only outputs HuBERT's native 50fps rate (confirmed against
    /// real published-model output — a correction of an earlier, wrong
    /// assumption here that the graph pre-doubled it); the 50fps -> 100fps
    /// doubling is `pipeline.py::_extract_features`'s own
    /// `np.repeat(feats, 2, axis=0)` step, applied here explicitly on the
    /// way out so callers get the frame rate the rest of the pipeline
    /// (RMVPE's 100fps salience, the synthesizer's phone input) expects.
    pub fn extract_features(&mut self, wav: &[f32]) -> Result<(Vec<f32>, usize), EngineError> {
        let input = Tensor::from_array((vec![1i64, wav.len() as i64], wav.to_vec()))?;
        let outputs = self.0.run(ort::inputs! { "wav" => input })?;
        let arr = outputs["features"].try_extract_array::<f32>()?;
        let shape = arr.shape();
        let n_frames_raw = shape[1];
        let dim = shape[2];
        let flat: Vec<f32> = arr.iter().copied().collect();

        // np.repeat(feats, 2, axis=0): each frame row appears twice in a row.
        let mut doubled = Vec::with_capacity(n_frames_raw * 2 * dim);
        for row in flat.chunks_exact(dim) {
            doubled.extend_from_slice(row);
            doubled.extend_from_slice(row);
        }
        Ok((doubled, n_frames_raw * 2))
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

// ── Synthesizer ──────────────────────────────────────────────────────────

/// Read the size of a named input's `dim_index`-th dimension from a
/// session's static-shape graph metadata (as exported by
/// `export_onnx.py` — every synthesizer input has a concrete, non-dynamic
/// shape, since the model was exported for exactly this application's fixed
/// windowing).
fn static_input_dim(session: &Session, name: &str, dim_index: usize) -> Result<usize, EngineError> {
    let outlet = session
        .inputs()
        .iter()
        .find(|o| o.name() == name)
        .ok_or_else(|| EngineError(format!("model has no input named {name:?}")))?;
    match outlet.dtype() {
        ort::value::ValueType::Tensor { shape, .. } => {
            let dim = shape.get(dim_index).copied().ok_or_else(|| {
                EngineError(format!(
                    "{name}: dimension {dim_index} out of range in shape {shape:?}"
                ))
            })?;
            if dim < 0 {
                return Err(EngineError(format!(
                    "{name}: dimension {dim_index} is dynamic ({dim}) — expected a static shape"
                )));
            }
            Ok(dim as usize)
        }
        other => Err(EngineError(format!(
            "{name}: expected a tensor input, got {other:?}"
        ))),
    }
}

/// A per-voice-model synthesizer session (VITS-style: TextEncoder +
/// normalizing-flow + NSF-HiFiGAN generator), exported by
/// `voice_changer/rvc/export_onnx.py`. `inter_channels`/`t_audio` are read
/// from the model's own static input shapes at load time rather than
/// hardcoded, since they vary per model (`inter_channels` is conventionally
/// 192 for the RVC v2/768 family, but `t_audio` depends on the model's
/// native sample rate — e.g. 24960 for a 48kHz model, 20800 for 40kHz).
pub struct SynthSession {
    session: Session,
    inter_channels: usize,
    t_audio: usize,
}

impl SynthSession {
    pub fn load(path: &Path) -> Result<Self, EngineError> {
        let session = load_session(path)?;
        let inter_channels = static_input_dim(&session, "prior_noise", 1)?;
        let t_audio = static_input_dim(&session, "source_noise", 1)?;
        Ok(Self {
            session,
            inter_channels,
            t_audio,
        })
    }

    pub fn t_audio(&self) -> usize {
        self.t_audio
    }

    /// `phone`: `[n_feat, 768]` row-major (ContentVec features). `pitch`:
    /// coarse F0 indices (0-255, port of `pipeline.py::_f0_to_coarse`).
    /// `pitchf`: F0 in Hz. `sid`: speaker id (0 for single-speaker models).
    /// Draws fresh `prior_noise`/`rand_phase`/`source_noise` from `rng` each
    /// call — matching `SynthesizerTrnMs768NSFsid.infer()`'s own internal
    /// `torch.randn_like`/`torch.rand` draws, which is *why* two calls with
    /// identical (phone, pitch, pitchf) inputs produce audibly-similar but
    /// not identical output (by VITS design, not a bug — see
    /// `docs/voice-changer-rvc-pipeline.md`). Returns the waveform at the
    /// model's native sample rate, length [`Self::t_audio`].
    pub fn infer(
        &mut self,
        phone: &[f32],
        n_feat: usize,
        pitch: &[i64],
        pitchf: &[f32],
        sid: i64,
        rng: &mut impl rand::Rng,
    ) -> Result<Vec<f32>, EngineError> {
        let standard_normal = rand_distr::StandardNormal;
        let prior_noise: Vec<f32> = (0..self.inter_channels * n_feat)
            .map(|_| rng.sample::<f32, _>(standard_normal))
            .collect();
        let rand_phase = vec![rng.random::<f32>()];
        let source_noise: Vec<f32> = (0..self.t_audio)
            .map(|_| rng.sample::<f32, _>(standard_normal))
            .collect();
        self.infer_with_noise(
            phone,
            n_feat,
            pitch,
            pitchf,
            sid,
            &prior_noise,
            &rand_phase,
            &source_noise,
        )
    }

    /// Lower-level entry point taking the three externalised-randomness
    /// tensors explicitly instead of drawing them — [`Self::infer`]'s real
    /// implementation, split out so tests can supply fixed, reproducible
    /// noise and assert an exact match against a Python/`onnxruntime`
    /// reference (matching this crate's usual verification style).
    #[allow(clippy::too_many_arguments)]
    pub fn infer_with_noise(
        &mut self,
        phone: &[f32],
        n_feat: usize,
        pitch: &[i64],
        pitchf: &[f32],
        sid: i64,
        prior_noise: &[f32],
        rand_phase: &[f32],
        source_noise: &[f32],
    ) -> Result<Vec<f32>, EngineError> {
        assert_eq!(phone.len(), n_feat * 768);
        assert_eq!(pitch.len(), n_feat);
        assert_eq!(pitchf.len(), n_feat);
        assert_eq!(prior_noise.len(), self.inter_channels * n_feat);
        assert_eq!(rand_phase.len(), 1);
        assert_eq!(source_noise.len(), self.t_audio);

        let prior_noise = prior_noise.to_vec();
        let rand_phase = rand_phase.to_vec();
        let source_noise = source_noise.to_vec();

        let phone_t = Tensor::from_array((vec![1i64, n_feat as i64, 768i64], phone.to_vec()))?;
        let pitch_t = Tensor::from_array((vec![1i64, n_feat as i64], pitch.to_vec()))?;
        let pitchf_t = Tensor::from_array((vec![1i64, n_feat as i64], pitchf.to_vec()))?;
        let sid_t = Tensor::from_array((vec![1i64], vec![sid]))?;
        let prior_t = Tensor::from_array((
            vec![1i64, self.inter_channels as i64, n_feat as i64],
            prior_noise,
        ))?;
        let phase_t = Tensor::from_array((vec![1i64, 1i64, 1i64], rand_phase))?;
        let noise_t = Tensor::from_array((vec![1i64, self.t_audio as i64, 1i64], source_noise))?;

        // export_onnx.py's static-shape export lets the tracer prove
        // phone_lengths is dead code (always == n_feat) and drop it from
        // the graph — only feed inputs the graph actually declares.
        let candidates: Vec<(&str, ort::session::SessionInputValue)> = vec![
            ("phone", phone_t.into()),
            ("pitch", pitch_t.into()),
            ("pitchf", pitchf_t.into()),
            ("sid", sid_t.into()),
            ("prior_noise", prior_t.into()),
            ("rand_phase", phase_t.into()),
            ("source_noise", noise_t.into()),
        ];
        let declared: std::collections::HashSet<&str> =
            self.session.inputs().iter().map(|o| o.name()).collect();
        let feed: Vec<(std::borrow::Cow<str>, ort::session::SessionInputValue)> = candidates
            .into_iter()
            .filter(|(name, _)| declared.contains(name))
            .map(|(name, v)| (std::borrow::Cow::Borrowed(name), v))
            .collect();

        let outputs = self.session.run(ort::session::SessionInputs::from(feed))?;
        let arr = outputs["audio"].try_extract_array::<f32>()?;
        Ok(arr.iter().copied().collect())
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
        // The ONNX graph itself outputs 26 raw (50fps) frames for this input;
        // extract_features doubles them to 52 (100fps), matching
        // pipeline.py::_extract_features's np.repeat(feats, 2, axis=0).
        assert_eq!(n_frames, 52);
        assert_eq!(features.len(), 52 * 768);

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

        // Raw (pre-doubling) row 13's values — now appear at doubled rows
        // 26 *and* 27 (np.repeat duplicates each row in place).
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
        let frame26 = &features[26 * 768..26 * 768 + 10];
        let frame27 = &features[27 * 768..27 * 768 + 10];
        for (got, want) in frame26.iter().zip(frame13_expected.iter()) {
            assert!(
                (got - want).abs() < 1e-3,
                "doubled frame 26: {got} vs {want}"
            );
        }
        assert_eq!(frame26, frame27, "np.repeat duplicates each row exactly");

        // Raw row 25 (the last of 26) — now appears at doubled rows 50/51.
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
        let last = &features[51 * 768..51 * 768 + 10];
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

    /// Not run by default — needs a real onnxruntime shared library and a
    /// real synthesizer `.onnx` exported by `export_onnx.py` from a real RVC
    /// v2 voice model. Run manually with
    /// `LAM_ORT_DYLIB_PATH=... LAM_TEST_SYNTH_ONNX_PATH=... cargo test --bin lam-daemon -- --ignored live_synth_matches_python_reference --nocapture`
    /// All inputs are closed-form (no RNG), reproduced bit-for-bit from
    /// `gen_synth_reference.py`; reference output values captured from the
    /// real `onnxruntime` Python package running the same exported model
    /// (`DvaOverwatch_350e.onnx`: RVC v2, 48kHz, inter_channels=192,
    /// t_audio=30720 for a 64-frame window — ContentVec's *real* production
    /// input is window+look-ahead+pad = 10560 samples, not just
    /// window+pad=8512, so T_f is 64 after 50fps->100fps doubling, not 52;
    /// see `export_onnx.py`'s `T_FEAT` comment for how this was found).
    #[test]
    #[ignore]
    fn live_synth_matches_python_reference() {
        init_runtime(&dylib_path()).expect("init_runtime");
        let onnx_path = std::env::var("LAM_TEST_SYNTH_ONNX_PATH").unwrap_or_else(|_| {
            format!(
                "{}/.config/arctis_manager/rvc_models/DvaOverwatch_350e.onnx",
                std::env::var("HOME").expect("HOME not set")
            )
        });
        let mut session = SynthSession::load(std::path::Path::new(&onnx_path))
            .unwrap_or_else(|e| panic!("failed to load {onnx_path}: {e}"));
        assert_eq!(session.inter_channels, 192);
        assert_eq!(session.t_audio(), 30720);

        const T: usize = 64;
        const INTER: usize = 192;
        const T_AUDIO: usize = 30720;

        let mut phone = vec![0.0f32; T * 768];
        for t in 0..T {
            for c in 0..768 {
                let (cf, tf) = (c as f32, t as f32);
                phone[t * 768 + c] =
                    0.3 * (cf * 0.02 + tf * 0.15).sin() + 0.05 * (cf * 0.01 - tf * 0.05).cos();
            }
        }
        let pitch: Vec<i64> = (0..T).map(|t| ((t as i64 * 7) % 254) + 1).collect();
        let pitchf: Vec<f32> = (0..T)
            .map(|t| 150.0 + 50.0 * (t as f32 * 0.3).sin())
            .collect();
        let sid = 0i64;

        let mut prior_noise = vec![0.0f32; INTER * T];
        for c in 0..INTER {
            for t in 0..T {
                let (cf, tf) = (c as f32, t as f32);
                prior_noise[c * T + t] = 0.2 * (cf * 0.07 + tf * 0.11).sin();
            }
        }
        let rand_phase = [0.37f32];
        let source_noise: Vec<f32> = (0..T_AUDIO)
            .map(|a| 0.15 * (a as f32 * 0.0031).sin())
            .collect();

        // Python reference generator lays prior_noise out as [INTER, T]
        // row-major (matching the ONNX tensor's [1, INTER, T] shape)
        // directly — no transpose needed here.
        let audio = session
            .infer_with_noise(
                &phone,
                T,
                &pitch,
                &pitchf,
                sid,
                &prior_noise,
                &rand_phase,
                &source_noise,
            )
            .expect("infer_with_noise");
        assert_eq!(audio.len(), T_AUDIO);

        let head_expected = [
            -0.10074884f32,
            -0.10417345,
            -0.10838882,
            -0.10982925,
            -0.11621542,
            -0.12508914,
            -0.12741813,
            -0.12091202,
            -0.10720991,
            -0.08920421,
        ];
        for (got, want) in audio[..10].iter().zip(head_expected.iter()) {
            assert!((got - want).abs() < 1e-2, "head: {got} vs {want}");
        }

        let mid_expected = [
            -0.03131239f32,
            -0.04543411,
            -0.05207795,
            -0.06087339,
            -0.07722096,
            -0.08814024,
            -0.09091135,
            -0.09129888,
            -0.08792963,
            -0.07666536,
        ];
        let mid = &audio[12480..12490];
        for (got, want) in mid.iter().zip(mid_expected.iter()) {
            assert!((got - want).abs() < 1e-2, "mid: {got} vs {want}");
        }

        let tail_expected = [
            -0.04306265f32,
            -0.04414488,
            -0.04486669,
            -0.04522632,
            -0.04521178,
            -0.04480137,
            -0.04437628,
            -0.04414027,
            -0.04397817,
            -0.04385395,
        ];
        let tail = &audio[T_AUDIO - 10..];
        for (got, want) in tail.iter().zip(tail_expected.iter()) {
            assert!((got - want).abs() < 1e-2, "tail: {got} vs {want}");
        }
    }
}
