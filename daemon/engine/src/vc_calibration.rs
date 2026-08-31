// Guided voice calibration.
//
// Direct port of `voice_changer/rvc/calibration.py`'s `CalibrationSession`:
// record_start/record_stop (the f32-stereo→mono downmix, peak detection),
// `propose_variants`, and — since [E10-S6b] — render_start/_render, now that
// the [E10-S6a] inference engine exists in Rust to run them through.
//
// The synthesizer's native sample rate (needed to construct `Pipeline`) is
// auto-detected from the `.onnx`'s own metadata (`SynthSession::
// native_sample_rate()`, stamped in by `export_onnx.py`'s
// `_stamp_sample_rate`) — see `RenderModel::sample_rate_hint`'s doc comment
// for the manual-entry fallback an `.onnx` exported before that existed
// needs.
//
// Wired from `dbus.rs`'s `CalibrationStartRender`, which picks a pitch
// pre-scan round (`propose_pitch_variants`) vs. a dynamics round
// (`propose_variants`) based on the request's `round` field.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::vc::inference::engine::{init_runtime, ContentVecSession, RmvpeSession, SynthSession};
use crate::vc::inference::pipeline::{Pipeline, HOP_FRAMES, OUTPUT_SR};
use crate::vc::inference::resample::resample;
use crate::vc::inference::retrieval::RetrievalIndex;
use crate::vc::wav_io::write_mono_pcm16;
use crate::vc_base_models::{CONTENTVEC_FILENAME, RMVPE_FILENAME};
use crate::vc_rvc_config::RvcParams;

pub const RECORD_SAMPLE_RATE: u32 = 16000;
const MAX_RECORD_SECS: u64 = 120;
// Matches `RvcParams::default().target_rms` — see the call site's comment.
const INPUT_NORMALIZE_TARGET_RMS: f32 = 0.06;
const INPUT_NORMALIZE_MAX_GAIN: f32 = 8.0;
/// f32 stereo = 8 bytes/frame.
const BYTES_PER_FRAME: u64 = 8;

pub fn user_cache_base_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("arctis_manager")
}

pub fn calibration_dir(cache_base: &Path) -> PathBuf {
    cache_base.join("calibration")
}

// ── Variant proposal (pure) ─────────────────────────────────────────────────

/// Three labeled parameter candidates around `base` — or, when refining,
/// half-steps around a previously chosen candidate on the two most audible
/// axes (target RMS drive, RMS mix rate). Direct port of
/// `calibration.py`'s `propose_variants`.
pub fn propose_variants(
    base: &RvcParams,
    refine_around: Option<&RvcParams>,
) -> Vec<(String, RvcParams)> {
    if let Some(x) = refine_around {
        let lo = RvcParams {
            target_rms: (x.target_rms * 0.85).max(0.04),
            rms_mix_rate: (x.rms_mix_rate - 0.15).max(0.0),
            ..x.clone()
        };
        let hi = RvcParams {
            target_rms: (x.target_rms * 1.15).min(0.20),
            rms_mix_rate: (x.rms_mix_rate + 0.15).min(1.0),
            ..x.clone()
        };
        return vec![
            ("A".to_owned(), x.clone()),
            ("B".to_owned(), lo),
            ("C".to_owned(), hi),
        ];
    }

    let faithful = RvcParams {
        rms_mix_rate: (base.rms_mix_rate - 0.35).max(0.0),
        target_rms: (base.target_rms * 0.7).max(0.04),
        limiter_thr: base.limiter_thr.min(0.80),
        ..base.clone()
    };
    let forward = RvcParams {
        rms_mix_rate: (base.rms_mix_rate + 0.25).min(1.0),
        target_rms: (base.target_rms * 1.3).min(0.20),
        vtln_alpha: if base.vtln_alpha != 1.0 { 1.0 } else { 0.88 },
        limiter_thr: 1.0,
        ..base.clone()
    };
    vec![
        ("A".to_owned(), base.clone()),
        ("B".to_owned(), faithful),
        ("C".to_owned(), forward),
    ]
}

/// Wide, evenly-spaced pitch-offset (semitone) candidates for a register
/// pre-scan round — meant to run *before* [`propose_variants`]'s dynamics
/// round, picking the right octave before fine-tuning drive/envelope.
/// RVC's pitch shift is a pure multiplicative transposition of the F0 curve
/// fed to the synthesizer (see `pipeline.rs::run_inference`: applied after
/// RMVPE extraction, before the synthesizer call — content features from
/// ContentVec are never shifted), so unlike dynamics tuning there's no
/// small local step that reliably brackets a good value: a source voice a
/// full register away from the model's trained target (e.g. bass-baritone
/// against a soprano-trained model) needs a full octave-plus shift, not a
/// few semitones — confirmed live: ±the model's own trained register span
/// mattered far more than any dynamics parameter for perceived quality.
/// `refine` mirrors `propose_variants`' first-round/refine-round split:
/// `false` for a first wide pass centered on `anchor` (typically 0), `true`
/// for narrow half-step brackets around a previously-picked value.
pub fn propose_pitch_variants(anchor: f32, refine: bool) -> Vec<(String, f32)> {
    let steps: &[f32] = if refine {
        &[-3.0, -1.5, 0.0, 1.5, 3.0]
    } else {
        &[-12.0, -5.0, 0.0, 7.0, 12.0, 19.0]
    };
    steps
        .iter()
        .enumerate()
        .map(|(i, &step)| {
            let label = char::from(b'A' + i as u8).to_string();
            (label, anchor + step)
        })
        .collect()
}

// ── Audio helpers (pure) ─────────────────────────────────────────────────────

/// Downmix raw interleaved f32 stereo PCM bytes to mono i16 samples (average
/// of the two channels, clamped to [-1, 1]), and the peak absolute mono
/// amplitude (0..1). Non-finite samples (NaN/Inf) are treated as silence.
/// A trailing partial frame (not a multiple of 8 bytes) is dropped.
pub fn downmix_stereo_f32_to_mono(buf: &[u8]) -> (Vec<i16>, f32) {
    let usable = buf.len() - (buf.len() % BYTES_PER_FRAME as usize);
    let mut peak = 0.0f32;
    let mut mono = Vec::with_capacity(usable / BYTES_PER_FRAME as usize);
    for frame in buf[..usable].as_chunks::<{ BYTES_PER_FRAME as usize }>().0 {
        let l = f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        let r = f32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };
        let avg = ((l + r) / 2.0).clamp(-1.0, 1.0);
        peak = peak.max(avg.abs());
        mono.push((avg * 32767.0) as i16);
    }
    (mono, peak)
}

/// Write mono 16-bit PCM samples as a canonical 44-byte-header WAV file.
pub fn write_mono_wav(path: &Path, sample_rate: u32, samples: &[i16]) -> std::io::Result<()> {
    let data_len = samples.len() * 2;
    let mut buf = Vec::with_capacity(44 + data_len);

    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size (PCM)
    buf.extend_from_slice(&1u16.to_le_bytes()); // audio format: PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // channels: mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 2; // mono, 16-bit
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }

    std::fs::write(path, &buf)
}

// ── Session state machine ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CalibrationState {
    Idle,
    Recording,
    Recorded,
    Rendering,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderResult {
    pub label: String,
    pub params: RvcParams,
    /// Semitones. Not part of [`RvcParams`] — a dynamics round holds this
    /// fixed across every variant, a pitch pre-scan round (`propose_pitch_variants`)
    /// holds `params` fixed and varies this instead.
    pub pitch_offset: f32,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationStatus {
    pub state: CalibrationState,
    pub error: String,
    pub recording: String,
    pub results: Vec<RenderResult>,
    pub peak: f32,
}

/// One recording (+ eventually render) cycle. Owned by the D-Bus VC service
/// (held as `Arc<Mutex<CalibrationSession>>`), same as the Python reference.
pub struct CalibrationSession {
    state: CalibrationState,
    error: String,
    recording_path: Option<PathBuf>,
    results: Vec<RenderResult>,
    peak: f32,
    rec_proc: std::sync::Arc<Mutex<Option<Child>>>,
    rec_buf: std::sync::Arc<Mutex<Vec<u8>>>,
    drain_handle: Option<tokio::task::JoinHandle<()>>,
}

impl Default for CalibrationSession {
    fn default() -> Self {
        Self::new()
    }
}

impl CalibrationSession {
    pub fn new() -> Self {
        Self {
            state: CalibrationState::Idle,
            error: String::new(),
            recording_path: None,
            results: Vec::new(),
            peak: 0.0,
            rec_proc: std::sync::Arc::new(Mutex::new(None)),
            rec_buf: std::sync::Arc::new(Mutex::new(Vec::new())),
            drain_handle: None,
        }
    }

    pub fn status(&self) -> CalibrationStatus {
        CalibrationStatus {
            state: self.state,
            error: self.error.clone(),
            recording: self
                .recording_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            results: self.results.clone(),
            peak: self.peak,
        }
    }

    /// Start capturing `source_id` via `pw-record` (not `parec`: NC/VC
    /// sources are native PipeWire filter-chain nodes the PulseAudio compat
    /// layer does not always expose). Stereo f32 is requested and downmixed
    /// by averaging in `record_stop` — asking `pw-record` for mono makes it
    /// *sum* the source channels (measured 2-3× gain), hard-clipping speech.
    pub async fn record_start(&mut self, cache_base: &Path, source_id: &str) -> bool {
        if self.state == CalibrationState::Recording {
            return false;
        }
        let dir = calibration_dir(cache_base);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.state = CalibrationState::Error;
            self.error = format!("cannot create calibration dir: {e}");
            return false;
        }

        let mut child = match Command::new("pw-record")
            .args([
                "--target",
                source_id,
                "--rate",
                &RECORD_SAMPLE_RATE.to_string(),
                "--channels",
                "2",
                "--format",
                "f32",
                "-",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                self.state = CalibrationState::Error;
                self.error = format!("pw-record failed: {e}");
                error!("calibration record_start: {e}");
                return false;
            }
        };

        let Some(mut stdout) = child.stdout.take() else {
            self.state = CalibrationState::Error;
            self.error = "pw-record: no stdout pipe".to_owned();
            return false;
        };

        self.rec_buf = std::sync::Arc::new(Mutex::new(Vec::new()));
        self.rec_proc = std::sync::Arc::new(Mutex::new(Some(child)));

        let buf = std::sync::Arc::clone(&self.rec_buf);
        let proc = std::sync::Arc::clone(&self.rec_proc);
        self.drain_handle = Some(tokio::spawn(async move {
            let limit = (MAX_RECORD_SECS * RECORD_SAMPLE_RATE as u64 * BYTES_PER_FRAME) as usize;
            let mut chunk = [0u8; 4096];
            loop {
                match stdout.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let hit_limit = {
                            let mut b = buf.lock().await;
                            b.extend_from_slice(&chunk[..n]);
                            b.len() >= limit
                        };
                        if hit_limit {
                            if let Some(mut c) = proc.lock().await.take() {
                                let _ = c.kill().await;
                                let _ = c.wait().await;
                            }
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }));

        self.state = CalibrationState::Recording;
        self.error.clear();
        info!("calibration recording started from {source_id:?}");
        true
    }

    /// Stop recording, downmix to mono, and write `original.wav`. Returns
    /// the recording path on success.
    pub async fn record_stop(&mut self, cache_base: &Path) -> Option<PathBuf> {
        if self.state != CalibrationState::Recording {
            return None;
        }

        if let Some(mut child) = self.rec_proc.lock().await.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        if let Some(handle) = self.drain_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        }

        let dir = calibration_dir(cache_base);
        let path = dir.join("original.wav");
        let raw = self.rec_buf.lock().await.clone();
        let (mono, peak) = downmix_stereo_f32_to_mono(&raw);

        if let Err(e) = write_mono_wav(&path, RECORD_SAMPLE_RATE, &mono) {
            self.state = CalibrationState::Error;
            self.error = format!("failed to write recording: {e}");
            error!("calibration record_stop: {e}");
            return None;
        }

        self.peak = peak;
        self.recording_path = Some(path.clone());
        self.state = CalibrationState::Recorded;
        info!(
            "calibration recording stopped: {:.1}s, peak {peak:.3} -> {}",
            mono.len() as f32 / RECORD_SAMPLE_RATE as f32,
            path.display()
        );
        Some(path)
    }

    /// Start rendering `variants` through the recording in a background
    /// task and return immediately — `true` if a render was actually
    /// started (the session was `Recorded`/`Done`/`Error` and has a
    /// recording), `false` otherwise. Poll [`Self::status`] for progress;
    /// state transitions to `Done` (with `results` populated) or `Error`
    /// (with `error` set) when the background task finishes.
    ///
    /// An associated function taking `Arc<Mutex<Self>>` rather than a
    /// `&mut self` method: the background task needs to write the result
    /// back into the *same* shared session once rendering finishes, not
    /// just the borrow this call received — mirrors how `record_start`'s
    /// drain task already holds its own `Arc` clones (`rec_proc`/`rec_buf`)
    /// rather than borrowing `self`.
    pub async fn render_start(
        session: Arc<Mutex<CalibrationSession>>,
        model: RenderModel,
        base_models_dir: PathBuf,
        dylib_path: PathBuf,
        out_dir: PathBuf,
        variants: Vec<RenderVariant>,
    ) -> bool {
        let recording_path = {
            let mut s = session.lock().await;
            if !matches!(
                s.state,
                CalibrationState::Recorded | CalibrationState::Done | CalibrationState::Error
            ) {
                return false;
            }
            let Some(p) = s.recording_path.clone() else {
                return false;
            };
            s.state = CalibrationState::Rendering;
            s.error.clear();
            s.results.clear();
            p
        };

        info!(
            "calibration render started: {} variant(s) from {}",
            variants.len(),
            recording_path.display()
        );

        tokio::spawn(async move {
            let outcome = tokio::task::spawn_blocking(move || {
                render_blocking(
                    &recording_path,
                    &model,
                    &base_models_dir,
                    &dylib_path,
                    &out_dir,
                    &variants,
                )
            })
            .await;

            let mut s = session.lock().await;
            match outcome {
                Ok(Ok(results)) => {
                    info!("calibration render done: {} variant(s)", results.len());
                    s.results = results;
                    s.state = CalibrationState::Done;
                }
                Ok(Err(e)) => {
                    error!("calibration render failed: {e}");
                    s.state = CalibrationState::Error;
                    s.error = e;
                }
                Err(join_err) => {
                    error!("calibration render task panicked: {join_err}");
                    s.state = CalibrationState::Error;
                    s.error = format!("render task panicked: {join_err}");
                }
            }
        });

        true
    }
}

/// Everything [`CalibrationSession::render_start`] needs to load the RVC
/// inference engine for a render, beyond the base models (ContentVec/RMVPE,
/// resolved from `base_models_dir` by their well-known filenames — see
/// `vc_base_models.rs`) and the `libonnxruntime.so` to run them on
/// (resolved by the caller via `vc_onnxruntime_detect::find_onnxruntime_dylib`,
/// same as `DetectOnnxRuntime` does).
pub struct RenderModel {
    /// The exported `.onnx` synthesizer (not the `.pth` checkpoint).
    pub path: PathBuf,
    /// Fallback native sample rate, used only when the `.onnx` has no
    /// `sample_rate` metadata (exported before `export_onnx.py` started
    /// stamping it) — see `SynthSession::native_sample_rate`. Normally
    /// `None`; when the auto-detect also comes back empty and no hint was
    /// supplied, rendering fails with a clear error rather than guessing.
    pub sample_rate_hint: Option<u32>,
    pub index_path: Option<PathBuf>,
}

/// One labeled render candidate: parameter tuning + pitch shift together,
/// so the same rendering machinery below serves both a dynamics round
/// (`propose_variants`: pitch fixed, params varying) and a pitch pre-scan
/// round (`propose_pitch_variants`: params fixed, pitch varying).
pub type RenderVariant = (String, RvcParams, f32);

/// Runs on a blocking-pool thread ([`CalibrationSession::render_start`]) —
/// real ONNX inference is synchronous, CPU/GPU-bound work, not
/// async-runtime-friendly. Port of `calibration.py`'s `_render`: for each
/// variant, a *fresh* `Pipeline` (ContentVec + RMVPE + synthesizer reloaded
/// from scratch) converts the whole recording hop by hop at the live
/// chain's exact 128 ms cadence, writes `variant_<label>.wav`, and moves on
/// — matching the Python reference's own per-variant reload rather than
/// sharing sessions across variants (simpler, and this module's own doc
/// comment already accepts "occasional latency hiccup" for calibration
/// renders; the sessions' *weights* don't vary across variants, but
/// `Pipeline` owns them outright and isn't designed to hand them back).
fn render_blocking(
    recording_path: &Path,
    model: &RenderModel,
    base_models_dir: &Path,
    dylib_path: &Path,
    out_dir: &Path,
    variants: &[RenderVariant],
) -> Result<Vec<RenderResult>, String> {
    init_runtime(dylib_path).map_err(|e| e.to_string())?;

    let (raw, sr) = crate::vc::wav_io::read_mono_f32(recording_path)
        .map_err(|e| format!("read recording: {e}"))?;
    let input_16k = resample(&raw, sr, RECORD_SAMPLE_RATE);
    // Normalize once, shared across every variant: a quiet *recording*
    // (not a per-variant tuning choice) is what makes the VAD gate/output
    // envelope mask crush genuine-but-quiet speech — see
    // `vc_dsp::normalize_input_level`'s doc comment. Target matches
    // `RvcParams::default().target_rms`, the same level the per-window
    // normalization inside `run_inference` already aims for downstream.
    let input_16k = crate::vc_dsp::normalize_input_level(
        &input_16k,
        INPUT_NORMALIZE_TARGET_RMS,
        INPUT_NORMALIZE_MAX_GAIN,
    );
    // `vc_dsp::detect_leading_noise_floor`/`calibrate_gate_from_noise_floor`
    // exist and are unit-tested, but are deliberately NOT wired in here yet:
    // live-verified (then reverted before commit) that stacking a
    // noise-floor-derived `knee_floor` on top of `normalize_input_level`'s
    // gain overshoots — the margin is computed against the *already
    // boosted* signal, so the resulting floor sits above real (also
    // boosted) quiet trailing speech, worse than the fixed default this
    // was meant to improve on. Needs a real design decision (measure
    // against the pre-gain recording instead? make the two mutually
    // exclusive rather than additive? smaller margins?) before it's safe
    // to combine with the gain above — tracked as a follow-up, not solved
    // here.

    std::fs::create_dir_all(out_dir).map_err(|e| format!("create output dir: {e}"))?;

    let mut results = Vec::with_capacity(variants.len());
    for (label, params, pitch_offset) in variants {
        let hubert = ContentVecSession::load(&base_models_dir.join(CONTENTVEC_FILENAME))
            .map_err(|e| format!("variant {label}: load ContentVec: {e}"))?;
        let rmvpe = RmvpeSession::load(&base_models_dir.join(RMVPE_FILENAME))
            .map_err(|e| format!("variant {label}: load RMVPE: {e}"))?;
        let synth = SynthSession::load(&model.path)
            .map_err(|e| format!("variant {label}: load synthesizer: {e}"))?;
        let sample_rate = synth
            .native_sample_rate()
            .or(model.sample_rate_hint)
            .ok_or_else(|| {
                format!(
                    "variant {label}: {} has no embedded sample rate (re-export with a newer \
                 export_onnx.py, or set the model's sample rate manually once)",
                    model.path.display()
                )
            })?;
        // Retrieval blend is a real, known performance problem for anything
        // but a tiny `.index` (brute-force k-NN over the full vector set —
        // see the [E10-S6a] retrieval.rs follow-up in CHANGELOG.md), so it's
        // only loaded for a variant that actually turns it on, not eagerly
        // for every render.
        let retrieval = if params.index_rate > 0.0 {
            match &model.index_path {
                Some(p) => Some(
                    RetrievalIndex::load(p)
                        .map_err(|e| format!("variant {label}: load retrieval index: {e}"))?,
                ),
                None => None,
            }
        } else {
            None
        };

        let mut pipeline =
            Pipeline::new(hubert, rmvpe, synth, sample_rate, params.clone(), retrieval);

        let mut out_all: Vec<f32> = Vec::new();
        let mut pos = 0usize;
        while pos < input_16k.len() {
            let end = (pos + HOP_FRAMES).min(input_16k.len());
            let mut hop = input_16k[pos..end].to_vec();
            hop.resize(HOP_FRAMES, 0.0);
            let out = pipeline
                .convert(&hop, RECORD_SAMPLE_RATE, *pitch_offset)
                .map_err(|e| format!("variant {label}: convert: {e}"))?;
            out_all.extend_from_slice(&out);
            pos = end;
        }
        // A handful of trailing silence hops to let the SOLA/xfade/look-ahead
        // buffering (which lags real input by a few hops) drain fully —
        // matches pipeline.rs's own offline harness and Python's `_render`.
        for _ in 0..8 {
            let out = pipeline
                .convert(&[0.0f32; HOP_FRAMES], RECORD_SAMPLE_RATE, *pitch_offset)
                .map_err(|e| format!("variant {label}: convert (flush): {e}"))?;
            out_all.extend_from_slice(&out);
        }

        let out_path = out_dir.join(format!("variant_{}.wav", label.to_lowercase()));
        write_mono_pcm16(&out_path, &out_all, OUTPUT_SR)
            .map_err(|e| format!("variant {label}: write wav: {e}"))?;

        results.push(RenderResult {
            label: label.clone(),
            params: params.clone(),
            pitch_offset: *pitch_offset,
            path: out_path.display().to_string(),
        });
    }
    Ok(results)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_frame(l: f32, r: f32) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[0..4].copy_from_slice(&l.to_le_bytes());
        out[4..8].copy_from_slice(&r.to_le_bytes());
        out
    }

    // ── propose_variants ─────────────────────────────────────────────────

    #[test]
    fn propose_variants_first_round_labels_and_count() {
        let base = RvcParams::default();
        let variants = propose_variants(&base, None);
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].0, "A");
        assert_eq!(variants[1].0, "B");
        assert_eq!(variants[2].0, "C");
        assert_eq!(variants[0].1, base);
    }

    #[test]
    fn propose_variants_faithful_softens_drive_and_mix() {
        let base = RvcParams::default();
        let variants = propose_variants(&base, None);
        let faithful = &variants[1].1;
        assert!(faithful.rms_mix_rate < base.rms_mix_rate);
        assert!(faithful.target_rms < base.target_rms);
        assert!(faithful.limiter_thr <= 0.80);
    }

    #[test]
    fn propose_variants_forward_raises_drive_and_mix() {
        let base = RvcParams::default();
        let variants = propose_variants(&base, None);
        let forward = &variants[2].1;
        assert!(forward.rms_mix_rate > base.rms_mix_rate);
        assert!(forward.target_rms > base.target_rms);
        assert_eq!(forward.limiter_thr, 1.0);
    }

    #[test]
    fn propose_variants_forward_nudges_vtln_when_already_neutral() {
        let base = RvcParams {
            vtln_alpha: 1.0,
            ..Default::default()
        };
        let variants = propose_variants(&base, None);
        assert_eq!(variants[2].1.vtln_alpha, 0.88);
    }

    #[test]
    fn propose_variants_forward_resets_vtln_when_already_shifted() {
        let base = RvcParams {
            vtln_alpha: 0.9,
            ..Default::default()
        };
        let variants = propose_variants(&base, None);
        assert_eq!(variants[2].1.vtln_alpha, 1.0);
    }

    #[test]
    fn propose_variants_refine_brackets_the_pick() {
        let picked = RvcParams {
            target_rms: 0.10,
            rms_mix_rate: 0.5,
            ..Default::default()
        };
        let variants = propose_variants(&RvcParams::default(), Some(&picked));
        assert_eq!(variants[0].1, picked);
        assert!(variants[1].1.target_rms < picked.target_rms);
        assert!(variants[1].1.rms_mix_rate < picked.rms_mix_rate);
        assert!(variants[2].1.target_rms > picked.target_rms);
        assert!(variants[2].1.rms_mix_rate > picked.rms_mix_rate);
    }

    #[test]
    fn propose_variants_refine_clamps_target_rms_floor() {
        let picked = RvcParams {
            target_rms: 0.04,
            ..Default::default()
        };
        let variants = propose_variants(&RvcParams::default(), Some(&picked));
        assert_eq!(variants[1].1.target_rms, 0.04);
    }

    // ── propose_pitch_variants ───────────────────────────────────────────

    #[test]
    fn propose_pitch_variants_first_pass_is_wide_and_labeled() {
        let variants = propose_pitch_variants(0.0, false);
        assert_eq!(variants.len(), 6);
        let labels: Vec<&str> = variants.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, vec!["A", "B", "C", "D", "E", "F"]);
        let offsets: Vec<f32> = variants.iter().map(|(_, o)| *o).collect();
        assert_eq!(offsets, vec![-12.0, -5.0, 0.0, 7.0, 12.0, 19.0]);
        // strictly increasing — the GUI lists them in a stable, meaningful order
        assert!(offsets.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn propose_pitch_variants_refine_is_narrow_and_centered_on_anchor() {
        let variants = propose_pitch_variants(12.0, true);
        assert_eq!(variants.len(), 5);
        let offsets: Vec<f32> = variants.iter().map(|(_, o)| *o).collect();
        assert_eq!(offsets, vec![9.0, 10.5, 12.0, 13.5, 15.0]);
        assert!(
            offsets.contains(&12.0),
            "anchor itself must be one candidate"
        );
    }

    #[test]
    fn propose_pitch_variants_first_pass_always_includes_zero() {
        // Zero-shift (no correction) must always be an option, even off-anchor,
        // so a user whose voice already matches the model isn't forced to pick
        // a shifted candidate.
        let variants = propose_pitch_variants(0.0, false);
        assert!(variants.iter().any(|(_, o)| *o == 0.0));
    }

    // ── downmix_stereo_f32_to_mono ───────────────────────────────────────

    #[test]
    fn downmix_averages_channels() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&stereo_frame(0.5, -0.5)); // avg 0.0
        buf.extend_from_slice(&stereo_frame(1.0, 1.0)); // avg 1.0
        let (mono, peak) = downmix_stereo_f32_to_mono(&buf);
        assert_eq!(mono.len(), 2);
        assert_eq!(mono[0], 0);
        assert_eq!(mono[1], 32767);
        assert!((peak - 1.0).abs() < 1e-6);
    }

    #[test]
    fn downmix_clamps_out_of_range() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&stereo_frame(2.0, 2.0)); // avg 2.0, clamped to 1.0
        let (mono, peak) = downmix_stereo_f32_to_mono(&buf);
        assert_eq!(mono[0], 32767);
        assert_eq!(peak, 1.0);
    }

    #[test]
    fn downmix_treats_non_finite_as_silence() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&stereo_frame(f32::NAN, f32::INFINITY));
        let (mono, peak) = downmix_stereo_f32_to_mono(&buf);
        assert_eq!(mono[0], 0);
        assert_eq!(peak, 0.0);
    }

    #[test]
    fn downmix_drops_trailing_partial_frame() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&stereo_frame(0.1, 0.1));
        buf.push(0xAB); // 1 stray byte, not a full frame
        let (mono, _peak) = downmix_stereo_f32_to_mono(&buf);
        assert_eq!(mono.len(), 1);
    }

    #[test]
    fn downmix_empty_buffer_has_zero_peak() {
        let (mono, peak) = downmix_stereo_f32_to_mono(&[]);
        assert!(mono.is_empty());
        assert_eq!(peak, 0.0);
    }

    // ── write_mono_wav ───────────────────────────────────────────────────

    #[test]
    fn write_mono_wav_produces_valid_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.wav");
        write_mono_wav(&path, 16000, &[0, 100, -100, 32767, -32768]).unwrap();

        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[0..4], b"RIFF");
        assert_eq!(&data[8..12], b"WAVE");
        assert_eq!(&data[12..16], b"fmt ");
        assert_eq!(u16::from_le_bytes([data[20], data[21]]), 1); // PCM
        assert_eq!(u16::from_le_bytes([data[22], data[23]]), 1); // mono
        assert_eq!(
            u32::from_le_bytes([data[24], data[25], data[26], data[27]]),
            16000
        );
        assert_eq!(u16::from_le_bytes([data[34], data[35]]), 16); // bits/sample
        assert_eq!(&data[36..40], b"data");
        let data_len = u32::from_le_bytes([data[40], data[41], data[42], data[43]]);
        assert_eq!(data_len as usize, 5 * 2);
        assert_eq!(data.len(), 44 + 5 * 2);
    }

    #[test]
    fn write_mono_wav_empty_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.wav");
        write_mono_wav(&path, 16000, &[]).unwrap();
        let data = std::fs::read(&path).unwrap();
        assert_eq!(data.len(), 44);
    }

    // ── Session state gating (no real pw-record involved) ───────────────

    #[test]
    fn new_session_starts_idle() {
        let session = CalibrationSession::new();
        let status = session.status();
        assert_eq!(status.state, CalibrationState::Idle);
        assert_eq!(status.peak, 0.0);
        assert!(status.recording.is_empty());
    }

    #[tokio::test]
    async fn record_stop_before_start_is_a_noop() {
        let mut session = CalibrationSession::new();
        let dir = tempfile::tempdir().unwrap();
        assert!(session.record_stop(dir.path()).await.is_none());
        assert_eq!(session.status().state, CalibrationState::Idle);
    }

    // ── live: real end-to-end render ([E10-S6b]) ─────────────────────────
    // Needs a real onnxruntime shared library, the real published base
    // models, a real synthesizer exported by `export_onnx.py`, and a real
    // input recording — same live-testing pattern as pipeline.rs's own
    // `#[ignore]`d tests. Constructs the session directly in `Recorded`
    // state (private-field access from this child module) rather than
    // driving a real `pw-record` capture, since only the render half is
    // under test here.

    /// Not run by default. Run manually with
    /// `LAM_ORT_DYLIB_PATH=... LAM_TEST_INPUT_WAV=/path/to/recording.wav \
    ///  [LAM_TEST_SYNTH_ONNX_PATH=...] [LAM_TEST_SYNTH_SR=48000] \
    ///  cargo test --bin lam-daemon -- --ignored live_render_start_produces_variant_wavs --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_render_start_produces_variant_wavs() {
        let dylib_path = std::env::var("LAM_ORT_DYLIB_PATH").expect(
            "set LAM_ORT_DYLIB_PATH to a real onnxruntime shared library \
             (e.g. `pip install onnxruntime` then point at its \
             onnxruntime/capi/libonnxruntime.so.*)",
        );
        let input_wav = std::env::var("LAM_TEST_INPUT_WAV")
            .expect("set LAM_TEST_INPUT_WAV to a real mono/stereo PCM16 WAV recording");
        let synth_path = std::env::var("LAM_TEST_SYNTH_ONNX_PATH").unwrap_or_else(|_| {
            format!(
                "{}/.config/arctis_manager/rvc_models/DvaOverwatch_350e.onnx",
                std::env::var("HOME").expect("HOME not set")
            )
        });
        let synth_sr: u32 = std::env::var("LAM_TEST_SYNTH_SR")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(48000);
        let base_models_dir = std::env::var("LAM_TEST_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").expect("HOME not set"))
                    .join(".config/arctis_manager/models")
            });

        let mut session = CalibrationSession::new();
        session.state = CalibrationState::Recorded;
        session.recording_path = Some(PathBuf::from(&input_wav));
        let session = Arc::new(Mutex::new(session));

        let out_dir = tempfile::tempdir().unwrap();
        let variants: Vec<RenderVariant> = vec![
            ("A".to_owned(), RvcParams::default(), 12.0),
            ("B".to_owned(), RvcParams::default(), 19.0),
        ];

        let started = CalibrationSession::render_start(
            Arc::clone(&session),
            RenderModel {
                path: PathBuf::from(&synth_path),
                sample_rate_hint: Some(synth_sr),
                index_path: None,
            },
            base_models_dir,
            PathBuf::from(&dylib_path),
            out_dir.path().to_path_buf(),
            variants,
        )
        .await;
        assert!(started, "render_start should accept a Recorded session");

        let status = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let s = session.lock().await.status();
                if matches!(s.state, CalibrationState::Done | CalibrationState::Error) {
                    return s;
                }
                drop(s);
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        })
        .await
        .expect("render did not finish within 120s");

        assert_eq!(
            status.state,
            CalibrationState::Done,
            "render error: {}",
            status.error
        );
        assert_eq!(status.results.len(), 2);
        for (result, expected_label, expected_pitch) in [
            (&status.results[0], "A", 12.0f32),
            (&status.results[1], "B", 19.0f32),
        ] {
            assert_eq!(result.label, expected_label);
            assert_eq!(result.pitch_offset, expected_pitch);
            let path = std::path::Path::new(&result.path);
            assert!(path.is_file(), "{} should exist", result.path);
            let meta = std::fs::metadata(path).unwrap();
            assert!(
                meta.len() > 44,
                "{} should contain real audio, not just a WAV header",
                result.path
            );
            eprintln!(
                "variant {}: {} ({} bytes)",
                result.label,
                result.path,
                meta.len()
            );
        }
    }
}
