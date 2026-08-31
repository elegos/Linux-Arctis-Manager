// The real-time streaming state machine ([E10-S6a]'s last piece): ties
// ContentVec, RMVPE, the synthesizer, retrieval, and every pure DSP function
// in `vc_dsp.rs`/`vc/inference/{mel,resample}.rs` into the sliding-window
// loop, line-by-line ported from `voice_changer/rvc/pipeline.py`'s
// `RVCPipeline.convert`/`_run_inference`.
//
// Every constant and code path here mirrors the Python reference exactly —
// including ones that look arbitrary (VAD thresholds, the 0.33 prior-noise
// scale, the 80-sample unfreeze fade) because they were tuned by ear against
// real speech in an earlier session, not derived mathematically. See
// `docs/voice-changer-rvc-pipeline.md`'s "What is not yet de-risked" section:
// the DSP pieces are individually verified against Python references, but
// this orchestration hasn't had the equivalent of live acoustic testing yet
// (the Python original was verified live on real hardware; this port has
// not been, only structurally/shape-tested) — treat it as faithful-but-
// unheard until that happens.
//
// Debug WAV recording and the auto-tuner metrics deque from the Python
// original are intentionally not ported: both are optional/dev-only
// (`_DEBUG_WAVS = False` by default; the auto-tuner they'd feed doesn't
// exist yet either in Python or Rust).

use std::collections::VecDeque;

use crate::vc_dsp::{
    f0_continuity_clamp, f0_median_filter, f0_phrase_final_floor, f0_to_coarse, fill_f0_gaps,
    gate_envelope, mix_rms, rms, soft_limit, sola_offset, voicedness, vtln_warp,
};
use crate::vc_rvc_config::RvcParams;

use super::engine::{ContentVecSession, EngineError, RmvpeSession, SynthSession};
use super::mel::{mel_filterbank, mel_spectrogram};
use super::resample::resample;
use super::retrieval::{retrieval_blend, RetrievalIndex};

const HUBERT_SR: u32 = 16000; // parec capture rate / RMVPE+ContentVec input rate
const OUTPUT_SR: u32 = 48000; // fixed downstream output rate
const WINDOW_FRAMES: usize = 8192; // 512ms @ 16kHz — full inference window
const HOP_FRAMES: usize = 2048; // 128ms @ 16kHz — new audio consumed per inference
const CONTEXT_FRAMES: usize = WINDOW_FRAMES - HOP_FRAMES; // 6144 = 384ms real previous audio
const HUBERT_EXTRA_PAD: usize = 320; // forces HuBERT to emit 26 frames instead of 25 for an 8192-sample window

const XFADE_OUT: usize = 480; // 10ms @ 48kHz SOLA crossfade length
const SOLA_SEARCH: usize = 960; // 20ms @ 48kHz SOLA alignment search range

const VAD_RMS: f32 = 0.0015;
const VAD_HANG_HOPS: u32 = 4;
const VAD_REL: f32 = 0.2;
const SPEECH_RMS_RELEASE: f32 = 0.9;
const VOICED_MIN: f32 = 0.45;
const GATE_RELEASE_S: f32 = 0.060;

const RMVPE_THRESHOLD: f32 = 0.022;
const MEL_N_MELS: usize = 128;
const MEL_N_FFT: usize = 1024;
const MEL_HOP: usize = 160;
const MEL_F_MIN: f64 = 30.0;
const MEL_F_MAX: f64 = 8000.0;

/// The full RVC voice-conversion pipeline for one loaded model: three ONNX
/// sessions plus every stateful buffer the sliding-window loop needs.
/// Owns nothing shared — one instance per active conversion, mirroring
/// `pipeline.py::RVCPipeline`.
pub struct Pipeline {
    hubert: ContentVecSession,
    rmvpe: RmvpeSession,
    synth: SynthSession,
    retrieval: Option<RetrievalIndex>,
    model_sr: u32,
    params: RvcParams,
    mel_basis: Vec<f32>,
    rng: rand::rngs::ThreadRng,

    context_buf: Vec<f32>,
    new_buf: Vec<f32>,
    out_buf: Vec<f32>,
    vad_hang: u32,
    gate_was_open: bool,
    speech_rms: f32,
    f0_ref: Option<f32>,
    f0_meds: VecDeque<f32>,
    prev_mask_tail: Vec<f32>,
    env_last: f32,
    env_tail: Vec<f32>,
    ctx_frozen: bool,
}

impl Pipeline {
    pub fn new(
        hubert: ContentVecSession,
        rmvpe: RmvpeSession,
        synth: SynthSession,
        model_sr: u32,
        params: RvcParams,
        retrieval: Option<RetrievalIndex>,
    ) -> Self {
        let env_tail_len = SOLA_SEARCH * HUBERT_SR as usize / OUTPUT_SR as usize;
        Pipeline {
            hubert,
            rmvpe,
            synth,
            retrieval,
            model_sr,
            params,
            mel_basis: mel_filterbank(
                MEL_N_FFT / 2 + 1,
                MEL_F_MIN,
                MEL_F_MAX,
                MEL_N_MELS,
                HUBERT_SR,
            ),
            rng: rand::rng(),
            context_buf: Vec::new(),
            new_buf: Vec::new(),
            // Seeded with XFADE_OUT zeros so the retroactive-crossfade guard
            // (len >= XFADE_OUT) always passes in steady state.
            out_buf: vec![0.0; XFADE_OUT],
            vad_hang: 0,
            gate_was_open: false,
            speech_rms: 0.0,
            f0_ref: None,
            f0_meds: VecDeque::with_capacity(15),
            prev_mask_tail: vec![0.0; XFADE_OUT],
            env_last: 0.0,
            env_tail: vec![0.0; env_tail_len],
            ctx_frozen: false,
        }
    }

    /// `audio`: mono float32 `[-1,1]` at `sr` (always 16kHz in this
    /// daemon's real usage — kept as a parameter for fidelity with the
    /// Python reference, which does too). `pitch_offset`: semitones.
    /// Returns float32 at `OUTPUT_SR`, `OUTPUT_SR/sr`x longer than `audio`.
    /// Uses a sliding window: each inference covers `WINDOW_FRAMES` of
    /// audio but advances only `HOP_FRAMES` — the overlap is real previous
    /// audio (`context_buf`), not zero-padding.
    pub fn convert(
        &mut self,
        audio: &[f32],
        sr: u32,
        pitch_offset: f32,
    ) -> Result<Vec<f32>, EngineError> {
        let n_out = audio.len() * OUTPUT_SR as usize / sr as usize;
        self.new_buf.extend_from_slice(audio);

        // DEVIATION FROM pipeline.py: Python buffers *two* look-ahead hops
        // at cold start (right after silence) instead of one, because the
        // extra real future audio measurably fixes short words garbling
        // right after a pause (see docs/voice-changer-rvc-pipeline.md). That
        // varies the synthesizer's phone-feature frame count between cold
        // start and steady state (T_f 78 vs 64 for this windowing), which a
        // *dynamic*-shape ONNX graph handles fine but this engine's
        // *static*-shape synthesizer export cannot (a second export at the
        // cold-start T_f would fix this properly — not done here). Always
        // requiring exactly one look-ahead hop keeps T_f constant at the
        // cost of that cold-start quality nicety; Python's `_cold_start`
        // flag (which only ever gated this buffering decision — verified
        // by grepping every other use site) is dropped entirely rather
        // than kept as now-inert state.
        loop {
            let required = 2 * HOP_FRAMES;
            if self.new_buf.len() < required {
                break;
            }
            let new_chunk: Vec<f32> = self.new_buf[..HOP_FRAMES].to_vec();
            let look_ahead: Vec<f32> = self.new_buf[HOP_FRAMES..required].to_vec();
            self.new_buf.drain(..HOP_FRAMES);

            let chunk_rms = rms(&new_chunk);
            let la_rms = rms(&look_ahead);
            let vad_thr = VAD_RMS.max(VAD_REL * self.speech_rms);
            let level_ok = chunk_rms >= vad_thr || la_rms >= vad_thr;
            let voiced_ok = !level_ok
                && self.gate_was_open
                && chunk_rms >= VAD_RMS
                && voicedness(&new_chunk, sr) >= VOICED_MIN;

            let gate_open;
            let hangover_hop;
            if level_ok || voiced_ok {
                if level_ok {
                    let cur = chunk_rms.max(la_rms);
                    if cur > self.speech_rms {
                        self.speech_rms = cur;
                    } else if cur >= 0.5 * self.speech_rms {
                        self.speech_rms =
                            SPEECH_RMS_RELEASE * self.speech_rms + (1.0 - SPEECH_RMS_RELEASE) * cur;
                    }
                }
                self.vad_hang = VAD_HANG_HOPS;
                gate_open = true;
                hangover_hop = false;
            } else if self.vad_hang > 0 {
                self.vad_hang -= 1;
                gate_open = true;
                hangover_hop = true;
            } else {
                gate_open = false;
                hangover_hop = false;
            }

            if !gate_open {
                self.handle_silence_hop(sr, n_out);
                continue;
            }
            self.gate_was_open = true;

            // Unfreeze: 5ms fade-out on the frozen context's tail so the
            // splice into the new phrase reads as a glottal stop, not a click.
            if self.ctx_frozen && self.context_buf.len() >= 80 {
                let n = self.context_buf.len();
                for (i, s) in self.context_buf[n - 80..].iter_mut().enumerate() {
                    *s *= 1.0 - i as f32 / 79.0;
                }
            }
            self.ctx_frozen = false;

            let mut window = if self.context_buf.len() < CONTEXT_FRAMES {
                let mut padded = vec![0.0f32; CONTEXT_FRAMES - self.context_buf.len()];
                padded.extend_from_slice(&self.context_buf);
                padded
            } else {
                self.context_buf.clone()
            };
            window.extend_from_slice(&new_chunk);

            let full_out = self.run_inference(&window, pitch_offset, Some(&look_ahead))?;
            let n_hop_out = HOP_FRAMES * OUTPUT_SR as usize / sr as usize;

            let n_take = n_hop_out + XFADE_OUT + SOLA_SEARCH;
            let mut hop_ext = if full_out.len() >= n_take {
                full_out[full_out.len() - n_take..].to_vec()
            } else {
                let mut v = full_out;
                v.resize(n_take, 0.0);
                v
            };

            soft_limit(&mut hop_ext, self.params.limiter_thr);

            let tail: Option<Vec<f32>> = (self.out_buf.len() >= XFADE_OUT)
                .then(|| self.out_buf[self.out_buf.len() - XFADE_OUT..].to_vec());
            let k = match &tail {
                Some(t) if t.iter().any(|&v| v != 0.0) => {
                    let seg = &hop_ext[..XFADE_OUT + SOLA_SEARCH];
                    sola_offset(seg, t)
                }
                _ => 0,
            };
            let aligned = &hop_ext[k..];
            let hop_raw = &aligned[XFADE_OUT..XFADE_OUT + n_hop_out];

            let (env_rel, new_env_last) =
                gate_envelope(&new_chunk, self.env_last, GATE_RELEASE_S, sr);
            self.env_last = new_env_last;

            // The hop's content lags input time by (SOLA_SEARCH - k) output
            // samples once SOLA-aligned, so the envelope mask must be
            // shifted by the same amount or it lands early and shaves
            // attacks — sourced from the previous hop's envelope tail where
            // the shift reaches back before this hop's own envelope.
            let n_tail = SOLA_SEARCH * sr as usize / OUTPUT_SR as usize;
            let shift = (SOLA_SEARCH - k) * sr as usize / OUTPUT_SR as usize;
            let mut ext = self.env_tail.clone();
            ext.extend_from_slice(&env_rel);
            let start = (n_tail as isize - shift as isize).max(0) as usize;
            let env_shifted = &ext[start..start + env_rel.len()];
            self.env_tail = env_rel[env_rel.len() - n_tail..].to_vec();

            let first_hang = hangover_hop && self.vad_hang >= VAD_HANG_HOPS - 1;
            let harsh = hangover_hop && !first_hang;
            let knee = if harsh {
                (0.5 * self.speech_rms).max(0.002)
            } else {
                0.002
            };
            let mask: Vec<f32> = env_shifted
                .iter()
                .map(|&v| (v / knee).clamp(0.0, 1.0).powi(2))
                .collect();
            let repeat = (OUTPUT_SR / sr).max(1) as usize;
            let mut mask_out: Vec<f32> = mask
                .iter()
                .flat_map(|&v| std::iter::repeat_n(v, repeat))
                .collect();
            if mask_out.len() < n_hop_out {
                let last = *mask_out.last().unwrap_or(&0.0);
                mask_out.resize(n_hop_out, last);
            }
            mask_out.truncate(n_hop_out);
            let hop: Vec<f32> = hop_raw
                .iter()
                .zip(mask_out.iter())
                .map(|(&h, &m)| h * m)
                .collect();

            if let Some(tail) = &tail {
                let m = self.out_buf.len();
                for i in 0..XFADE_OUT {
                    let fade_out = 1.0 - i as f32 / (XFADE_OUT as f32 - 1.0);
                    let fade_in = 1.0 - fade_out;
                    self.out_buf[m - XFADE_OUT + i] =
                        tail[i] * fade_out + aligned[i] * self.prev_mask_tail[i] * fade_in;
                }
            }
            self.out_buf.extend_from_slice(&hop);
            self.prev_mask_tail = mask_out[mask_out.len() - XFADE_OUT..].to_vec();

            if chunk_rms >= vad_thr || voiced_ok {
                self.context_buf.extend_from_slice(&new_chunk);
                if self.context_buf.len() > CONTEXT_FRAMES {
                    let excess = self.context_buf.len() - CONTEXT_FRAMES;
                    self.context_buf.drain(..excess);
                }
            } else {
                self.ctx_frozen = true;
            }
        }

        if self.out_buf.len() >= XFADE_OUT + n_out {
            let out: Vec<f32> = self.out_buf[..n_out].to_vec();
            self.out_buf.drain(..n_out);
            Ok(out)
        } else {
            Ok(vec![0.0; n_out])
        }
    }

    /// The gate-closed path: fade the reserve tail to silence on a
    /// speech→silence edge, freeze the context (don't dilute it with
    /// near-silence), and emit near-silence while repaying any latency
    /// debt built up during a cold-start stall.
    fn handle_silence_hop(&mut self, sr: u32, n_out: usize) {
        let n_hop_out = HOP_FRAMES * OUTPUT_SR as usize / sr as usize;
        if self.gate_was_open && self.out_buf.len() >= XFADE_OUT {
            let n = self.out_buf.len();
            for (i, s) in self.out_buf[n - XFADE_OUT..].iter_mut().enumerate() {
                *s *= 1.0 - i as f32 / (XFADE_OUT as f32 - 1.0);
            }
        }
        self.gate_was_open = false;
        self.f0_ref = None;
        self.prev_mask_tail = vec![0.0; XFADE_OUT];
        self.env_last *= (-(HOP_FRAMES as f32) / (GATE_RELEASE_S * sr as f32)).exp();
        self.env_tail = vec![self.env_last; self.env_tail.len()];

        let excess =
            (self.out_buf.len() as isize - XFADE_OUT as isize - n_out as isize).max(0) as usize;
        let zeros_n = n_hop_out.saturating_sub(excess);
        self.out_buf.extend(std::iter::repeat_n(0.0, zeros_n));
        self.ctx_frozen = true;
    }

    /// One window's worth of ContentVec → RMVPE → retrieval → synthesizer
    /// → resample → envelope-mix. Port of `pipeline.py::_run_inference`.
    /// Unlike the Python reference, this has no `sr` parameter: Python's
    /// version threads it through to `_extract_f0`'s autocorrelation
    /// fallback for when RMVPE isn't loaded, a path this Rust engine has no
    /// equivalent of (RMVPE is always required here) — so it would be dead
    /// weight, not fidelity, to carry it through unused.
    fn run_inference(
        &mut self,
        window: &[f32],
        pitch_offset: f32,
        look_ahead: Option<&[f32]>,
    ) -> Result<Vec<f32>, EngineError> {
        let mean = window.iter().sum::<f32>() / window.len() as f32;
        let mut audio: Vec<f32> = window.iter().map(|&v| v - mean).collect();

        // Active-frame RMS: only frames at >=20% of the window's loudest
        // frame count toward the level estimate, so a mostly-silent
        // phrase-onset window doesn't get diluted ~10x and underdrive the
        // model.
        let n_frames_160 = audio.len() / 160;
        let fr_rms: Vec<f32> = (0..n_frames_160)
            .map(|i| rms(&audio[i * 160..(i + 1) * 160]))
            .collect();
        let max_fr = fr_rms.iter().cloned().fold(0.0f32, f32::max);
        let thr = (max_fr * 0.2).max(1e-5);
        let active: Vec<f32> = fr_rms.iter().cloned().filter(|&v| v >= thr).collect();
        let active_rms = if !active.is_empty() {
            active.iter().sum::<f32>() / active.len() as f32
        } else {
            rms(&audio)
        };

        let target_rms = self.params.target_rms;
        let mut norm_gain = 1.0f32;
        if active_rms > 1e-4 {
            norm_gain = (target_rms / active_rms).min(4.0);
            for v in audio.iter_mut() {
                *v *= norm_gain;
            }
        }
        let peak = audio.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        if peak > 0.70 {
            let g = 0.70 / peak;
            norm_gain *= g;
            for v in audio.iter_mut() {
                *v *= g;
            }
        }

        let right_pad: Vec<f32> = match look_ahead {
            Some(la) if la.len() >= HOP_FRAMES => {
                let la_mean = la.iter().sum::<f32>() / la.len() as f32;
                la.iter().map(|&v| (v - la_mean) * norm_gain).collect()
            }
            _ => vec![0.0; HOP_FRAMES],
        };
        let mut audio_padded = audio.clone();
        audio_padded.extend_from_slice(&right_pad);

        // ── ContentVec features ─────────────────────────────────────────
        let hubert_input = vtln_warp(&audio_padded, self.params.vtln_alpha);
        let mut hubert_padded = hubert_input;
        hubert_padded.extend(std::iter::repeat_n(0.0, HUBERT_EXTRA_PAD));
        let (mut feats, t_f) = self.hubert.extract_features(&hubert_padded)?;

        // FAISS-style retrieval blend (RVC WebUI "index rate").
        if self.params.index_rate > 0.0 {
            if let Some(idx) = &self.retrieval {
                retrieval_blend(idx, &mut feats, t_f, 768, 8, self.params.index_rate);
            }
        }

        // ── F0 (RMVPE) ───────────────────────────────────────────────────
        let (mel, mel_frames) = mel_spectrogram(
            &audio_padded,
            &self.mel_basis,
            MEL_N_MELS,
            MEL_N_FFT,
            MEL_HOP,
        );
        let (salience, _) = self.rmvpe.infer_salience(&mel, mel_frames)?;
        let (mut f0, f0_conf) =
            crate::vc_dsp::rmvpe_decode(&salience, mel_frames, 360, RMVPE_THRESHOLD);

        fill_f0_gaps(&mut f0, 8);
        if pitch_offset != 0.0 {
            let mult = 2f32.powf(pitch_offset / 12.0);
            for v in f0.iter_mut() {
                if *v > 0.0 {
                    *v *= mult;
                }
            }
        }

        let mut f0_meds_vec: Vec<f32> = self.f0_meds.iter().copied().collect();
        f0_phrase_final_floor(&mut f0, Some(&f0_conf), &mut f0_meds_vec);
        self.f0_meds = f0_meds_vec.into();
        while self.f0_meds.len() > 15 {
            self.f0_meds.pop_front();
        }

        if f0.len() != t_f {
            f0 = crate::vc_dsp::resize1d(&f0, t_f);
        }

        f0_median_filter(&mut f0, self.params.filter_radius);
        f0_continuity_clamp(&mut f0, &mut self.f0_ref);
        let f0_coarse = f0_to_coarse(&f0);

        // ── Synthesizer ──────────────────────────────────────────────────
        let audio_out = self
            .synth
            .infer(&feats, t_f, &f0_coarse, &f0, 0, &mut self.rng)?;
        let mut out_np: Vec<f32> = audio_out
            .into_iter()
            .map(|v| if v.is_finite() { v } else { 0.0 })
            .collect();

        // Trim at model_sr to strip the right-pad's contribution — the
        // model produces slightly more audio than the pre-right-pad window
        // duration maps to, since T_f's frames were partly derived from it.
        let n_out_model = WINDOW_FRAMES * self.model_sr as usize / HUBERT_SR as usize;
        out_np.truncate(n_out_model);
        if out_np.len() < n_out_model {
            out_np.resize(n_out_model, 0.0);
        }

        out_np = resample(&out_np, self.model_sr, OUTPUT_SR);

        let n_out_target = WINDOW_FRAMES * OUTPUT_SR as usize / HUBERT_SR as usize;
        out_np.truncate(n_out_target);
        if out_np.len() < n_out_target {
            out_np.resize(n_out_target, 0.0);
        }

        Ok(mix_rms(&audio, &out_np, self.params.rms_mix_rate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vc::inference::engine::init_runtime;

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

    fn base_models_dir() -> std::path::PathBuf {
        std::env::var("LAM_TEST_MODELS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(std::env::var("HOME").expect("HOME not set"))
                    .join(".config/arctis_manager/models")
            })
    }

    fn synth_onnx_path() -> std::path::PathBuf {
        std::env::var("LAM_TEST_SYNTH_ONNX_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(std::env::var("HOME").expect("HOME not set"))
                    .join(".config/arctis_manager/rvc_models/DvaOverwatch_350e.onnx")
            })
    }

    fn load_pipeline() -> Pipeline {
        init_runtime(&dylib_path()).expect("init_runtime");
        let models = base_models_dir();
        let hubert = ContentVecSession::load(&models.join("content_vec_best.onnx"))
            .expect("load content_vec_best.onnx");
        let rmvpe = RmvpeSession::load(&models.join("rmvpe.onnx")).expect("load rmvpe.onnx");
        let synth_path = synth_onnx_path();
        let synth = SynthSession::load(&synth_path)
            .unwrap_or_else(|e| panic!("failed to load {}: {e}", synth_path.display()));
        Pipeline::new(hubert, rmvpe, synth, 48000, RvcParams::default(), None)
    }

    /// Not run by default — needs a real onnxruntime shared library, the
    /// real published base models, and a real synthesizer exported by
    /// `export_onnx.py`. Run manually with
    /// `LAM_ORT_DYLIB_PATH=... cargo test --bin lam-daemon -- --ignored live_convert_stays_silent_on_true_silence --nocapture`
    /// A pure-digital-zero input should never open the VAD gate, regardless
    /// of how many hops are fed — this is a real end-to-end smoke test of
    /// the whole chain (all three ONNX sessions + every DSP function),
    /// checking a property that doesn't depend on acoustic quality (which
    /// this test suite cannot judge — see this module's header comment).
    #[test]
    #[ignore]
    fn live_convert_stays_silent_on_true_silence() {
        let mut pipeline = load_pipeline();
        let hop = vec![0.0f32; HOP_FRAMES];
        for _ in 0..6 {
            let out = pipeline.convert(&hop, HUBERT_SR, 0.0).expect("convert");
            assert_eq!(out.len(), HOP_FRAMES * 3); // OUTPUT_SR/HUBERT_SR = 3
            assert!(
                out.iter().all(|&v| v == 0.0),
                "silence in should stay silence out"
            );
        }
    }

    /// Not run by default — see `live_convert_stays_silent_on_true_silence`.
    /// Feeds a loud, periodic (voiced-like) tone for enough hops to clear
    /// the cold-start requirement and open the VAD gate, then checks the
    /// engine actually ran real synthesis (non-silent, finite output) —
    /// the full ContentVec → RMVPE → synthesizer → resample → mix chain,
    /// exercised end to end. Does not (cannot, without listening) assert
    /// anything about output *quality* — see this module's header comment.
    #[test]
    #[ignore]
    fn live_convert_produces_sound_on_a_loud_tone() {
        let mut pipeline = load_pipeline();
        let sr = HUBERT_SR as f32;
        let mut t = 0.0f32;
        let mut any_nonzero = false;
        let mut all_finite = true;
        // 2 hops buffered before the first inference fires, plus a few more
        // to get well past it.
        for _ in 0..8 {
            let hop: Vec<f32> = (0..HOP_FRAMES)
                .map(|i| {
                    let s = 0.3 * (2.0 * std::f32::consts::PI * 150.0 * t).sin();
                    t += 1.0 / sr;
                    let _ = i;
                    s
                })
                .collect();
            let out = pipeline.convert(&hop, HUBERT_SR, 0.0).expect("convert");
            assert_eq!(out.len(), HOP_FRAMES * 3);
            if out.iter().any(|&v| v != 0.0) {
                any_nonzero = true;
            }
            if !out.iter().all(|v| v.is_finite()) {
                all_finite = false;
            }
        }
        assert!(
            any_nonzero,
            "a loud, sustained tone should eventually open the VAD gate and produce sound"
        );
        assert!(all_finite, "output must never contain NaN/Inf");
    }
}
