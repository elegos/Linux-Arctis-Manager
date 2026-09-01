// Live RVC voice-conversion chain — [E10-S8], the real-time mic-in ->
// inference -> speaker-out path a live call actually uses, distinct from
// `vc_calibration.rs`'s offline render (one recording in/out). Direct port
// of `voice_changer/rvc/rvc_chain.py`'s architecture onto the [E10-S6a] Rust
// inference engine: `pw-record` (capture) -> `Pipeline::convert` (inference)
// -> `pacat` (playback into a null sink), same three-stage shape as the
// Python `capture_thread`/`convert_thread`/`playback_thread`, collapsed here
// into one `spawn_blocking` loop since Rust doesn't need separate OS threads
// to overlap I/O with CPU-bound inference the way the GIL-bound Python
// original did.
//
// Output handoff mirrors `vc_ladspa_chain.rs`: `apply_rvc` returns the
// source name `mic_router` should point `Arctis_Manager_Mic` at (VC takes
// priority over NC — see `mic_router.rs`) once the chain is up.
//
// Capture requests stereo f32 and downmixes by averaging, not a direct mono
// request — mirrors `vc_calibration.rs::record_start`'s documented finding
// that `pw-record --channels 1` sums (not averages) a native PipeWire
// filter-chain node's channels, hard-clipping speech.
//
// Known limitation, shared with `vc_calibration.rs`'s render path: FAISS-style
// retrieval (`index_rate > 0`) is only loaded once, at chain build time, based
// on whatever `index_rate` was in effect then — turning it on later via
// `SetRVCLiveParams` without an initial `index_rate > 0` silently has no
// effect (`Pipeline`'s retrieval index isn't swappable after construction).
// A real fix needs a rebuild-on-index-rate-transition rule, not done here.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child as StdChild, Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::state::SignalEvent;
use crate::vc::inference::engine::{init_runtime, ContentVecSession, RmvpeSession, SynthSession};
use crate::vc::inference::pipeline::{GateCalibration, Pipeline, HOP_FRAMES, OUTPUT_SR};
use crate::vc::inference::retrieval::RetrievalIndex;
use crate::vc_base_models::{CONTENTVEC_FILENAME, RMVPE_FILENAME};
use crate::vc_config::RvcConfig;
use crate::vc_rvc_config::RvcParams;

/// Null sink the converted voice is played into.
pub const RVC_SINK: &str = "Arctis_VC_Sink";
const RVC_SINK_DESC: &str = "Arctis Manager VC Output";
/// The source `mic_router` should point `Arctis_Manager_Mic` at once this
/// chain is running — matches the legacy Python service's
/// `f'{ARCTIS_VC_SINK}.monitor'` (`core.py::_update_mic_routing`).
pub const RVC_MIC_SOURCE: &str = "Arctis_VC_Sink.monitor";

/// ContentVec/RMVPE input rate — matches `pipeline.rs`'s private `HUBERT_SR`
/// and `vc_calibration.rs`'s `RECORD_SAMPLE_RATE` (the same physical
/// constant, each module keeping its own copy rather than a shared import,
/// consistent with how `pipeline.rs` already does this).
const CAPTURE_SR: u32 = 16000;
/// Stereo f32 frames per hop: `HOP_FRAMES` mono samples, 2 channels, 4 bytes.
const CAPTURE_HOP_BYTES: usize = HOP_FRAMES * 2 * 4;
/// How much leading audio to accumulate before attempting a one-time VAD
/// gate calibration from it — see the call site in `run_live_loop`.
const GATE_CALIBRATION_WINDOW_SAMPLES: usize = CAPTURE_SR as usize * 3 / 2; // 1.5s

/// Everything needed to (re)build the live chain for one selected model.
/// Same shape as `vc_calibration.rs::RenderModel`, kept as a separate type
/// since the two modules' call sites/lifecycles are independent.
pub struct LiveModel {
    /// The exported `.onnx` synthesizer (not the `.pth` checkpoint).
    pub onnx_path: PathBuf,
    /// Fallback native sample rate — see `RenderModel::sample_rate_hint`.
    pub sample_rate_hint: Option<u32>,
    pub index_path: Option<PathBuf>,
}

/// Diagnostic snapshot for `GetRVCMetrics`. No per-hop quality auto-tuner
/// exists on this engine yet (`pipeline.rs`'s module doc: intentionally not
/// ported from the Python `_DEBUG_WAVS`/auto-tuner deque), so this reports
/// real operational metrics instead of a fake stub.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct RvcLiveMetrics {
    pub active: bool,
    pub hops_processed: u64,
    pub avg_hop_ms: f32,
    pub last_error: Option<String>,
}

/// Params/pitch the running inference loop reads each hop — mutated live by
/// `SetRVCLiveParams` (params only; pitch is set once at `apply_rvc` time,
/// same split as the Python reference's `RVCParams` vs. `pitch_offset` arg).
struct SharedState {
    params: RvcParams,
    pitch_offset: f32,
}

/// Live RVC chain state held in `Arc<Mutex<RvcLiveRuntime>>` — mirrors
/// `vc_ladspa_chain::VcLadspaRuntime`. Plain `std::sync::Mutex`-wrapped
/// fields (not `tokio::sync::Mutex`) so the blocking inference thread can
/// touch them without needing a runtime handle.
#[derive(Default)]
pub struct RvcLiveRuntime {
    running: Option<Arc<AtomicBool>>,
    capture_child: Option<Arc<StdMutex<StdChild>>>,
    playback_child: Option<Arc<StdMutex<StdChild>>>,
    join: Option<tokio::task::JoinHandle<()>>,
    null_sink_module: Option<u32>,
    shared: Option<Arc<StdMutex<SharedState>>>,
    metrics: Option<Arc<StdMutex<RvcLiveMetrics>>>,
    baked_source: String,
    baked_model_path: PathBuf,
}

impl RvcLiveRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when the inference loop's `running` flag is still set — cleared
    /// by `teardown_rvc` and (defensively) by the loop itself on exit.
    pub fn is_active(&self) -> bool {
        self.running
            .as_ref()
            .map(|r| r.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn metrics_snapshot(&self) -> RvcLiveMetrics {
        self.metrics
            .as_ref()
            .and_then(|m| m.lock().ok())
            .map(|m| m.clone())
            .unwrap_or_default()
    }

    /// Live param push (no rebuild) — `false` when no chain is running.
    /// Never touches `pitch_offset`, matching the legacy Python
    /// `SetRVCLiveParams`'s own scope.
    pub fn set_live_params(&self, params: RvcParams) -> bool {
        let Some(shared) = &self.shared else {
            return false;
        };
        let Ok(mut s) = shared.lock() else {
            return false;
        };
        s.params = params;
        true
    }
}

// ── Public apply / teardown ───────────────────────────────────────────────────

/// Apply (or update) the live RVC chain. Returns [`RVC_MIC_SOURCE`] on
/// success, `None` on failure (chain is torn down/left inactive either
/// way). Every failure — here and inside the background inference loop —
/// is also sent as a [`SignalEvent::VCLiveChainError`] over `signal_tx`,
/// so a `SetVCSettings` that returns `true` synchronously but then fails
/// asynchronously (a bad capture source, missing cuDNN, a stale `.onnx`,
/// ...) doesn't leave the user with no signal beyond the daemon log —
/// found live via exactly that: a feedback-loop capture source that left
/// the chain "running" with total, silent, unexplained silence.
#[allow(clippy::too_many_arguments)]
pub async fn apply_rvc(
    cfg: &RvcConfig,
    model: LiveModel,
    source_id: String,
    base_models_dir: PathBuf,
    dylib_path: PathBuf,
    runtime: &mut RvcLiveRuntime,
    signal_tx: broadcast::Sender<SignalEvent>,
) -> Option<&'static str> {
    // Live update: same source + same model already running, just push new
    // tuning params/pitch without tearing anything down.
    if runtime.is_active()
        && runtime.baked_source == source_id
        && runtime.baked_model_path == model.onnx_path
    {
        if let Some(shared) = &runtime.shared {
            if let Ok(mut s) = shared.lock() {
                s.params = cfg.params.clone();
                s.pitch_offset = cfg.pitch_offset;
            }
        }
        return Some(RVC_MIC_SOURCE);
    }

    teardown_rvc(runtime).await;

    // Stale sink from a previous run (possibly a different rate/config) —
    // remove before recreating, mirroring `rvc_chain.py`'s own cleanup.
    if let Some(existing_id) = find_existing_sink_module().await {
        let _ = crate::audio::unload_module_by_id(existing_id).await;
    }

    let sink_args = format!(
        "sink_name={RVC_SINK} node.description=\"{RVC_SINK_DESC}\" channels=1 rate={OUTPUT_SR}"
    );
    let Some(sink_module) = crate::audio::load_module_pub("module-null-sink", &sink_args).await
    else {
        report_error(&signal_tx, format!("failed to create {RVC_SINK}"));
        return None;
    };

    // `--target <name>` was found live to sometimes mis-resolve to a
    // different node entirely (see `audio::resolve_source_numeric_id`'s
    // doc comment) — the numeric object index resolves reliably, name is
    // only a fallback for when that lookup itself fails.
    let capture_target = match crate::audio::resolve_source_numeric_id(&source_id).await {
        Some(id) => id.to_string(),
        None => source_id.clone(),
    };

    let mut capture = match StdCommand::new("pw-record")
        .args([
            "--target",
            &capture_target,
            // `pw-record`'s default media role ("Music") was found live to
            // be overridden by PipeWire's pulse-compat `module-stream-restore`
            // — it remembers, *per role* (not per app/target), where a
            // stream last connected, and silently re-links a new stream to
            // that remembered node instead of the explicit `--target`
            // whenever the role matches (reproduced: a stale "music" role
            // -> `Arctis_Manager_Mic` association from earlier in this same
            // debugging session overrode every subsequent `--target`,
            // including the numeric-id fix above, until the role itself
            // was changed). "production" is a real, distinct PulseAudio
            // media role with its own restore bucket, unlikely to already
            // carry a stale entry.
            "--media-role",
            "production",
            "--rate",
            &CAPTURE_SR.to_string(),
            "--channels",
            "2",
            "--format",
            "f32",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            report_error(&signal_tx, format!("pw-record failed: {e}"));
            let _ = crate::audio::unload_module_by_id(sink_module).await;
            return None;
        }
    };
    if capture.stdout.is_none() {
        report_error(&signal_tx, "pw-record: no stdout pipe".to_owned());
        let _ = capture.kill();
        let _ = crate::audio::unload_module_by_id(sink_module).await;
        return None;
    }
    if let Some(stderr) = capture.stderr.take() {
        spawn_stderr_logger(stderr, "pw-record");
    }

    // Belt-and-suspenders against the WirePlumber link-resolution issue
    // above: even with a correct numeric target and a dedicated media
    // role, the stream was still found live to sometimes land on the
    // wrong node — see `audio::ensure_source_output_linked`'s doc comment
    // for the full story and why this runs as a background task rather
    // than inline here (its own delay needs to occur *after* `mic_router`'s
    // near-simultaneous follow-up touch in this call's caller, which
    // hasn't happened yet at this point in `apply_rvc`).
    tokio::spawn(crate::audio::ensure_source_output_linked(
        "production",
        capture_target.clone(),
    ));

    let mut playback = match StdCommand::new("pacat")
        .args([
            "--device",
            RVC_SINK,
            "--raw",
            "--channels",
            "1",
            "--rate",
            &OUTPUT_SR.to_string(),
            "--format",
            "s16le",
            "--latency-msec=200",
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            report_error(&signal_tx, format!("pacat failed: {e}"));
            let _ = capture.kill();
            let _ = crate::audio::unload_module_by_id(sink_module).await;
            return None;
        }
    };
    if let Some(stderr) = playback.stderr.take() {
        spawn_stderr_logger(stderr, "pacat");
    }

    let running = Arc::new(AtomicBool::new(true));
    let shared = Arc::new(StdMutex::new(SharedState {
        params: cfg.params.clone(),
        pitch_offset: cfg.pitch_offset,
    }));
    let metrics = Arc::new(StdMutex::new(RvcLiveMetrics::default()));
    let capture_arc = Arc::new(StdMutex::new(capture));
    let playback_arc = Arc::new(StdMutex::new(playback));
    let model_path = model.onnx_path.clone();

    let join = {
        let running = Arc::clone(&running);
        let capture_arc = Arc::clone(&capture_arc);
        let playback_arc = Arc::clone(&playback_arc);
        let shared = Arc::clone(&shared);
        let metrics = Arc::clone(&metrics);
        let signal_tx = signal_tx.clone();
        tokio::task::spawn_blocking(move || {
            run_live_loop(
                running,
                capture_arc,
                playback_arc,
                shared,
                metrics,
                base_models_dir,
                dylib_path,
                model,
                signal_tx,
            );
        })
    };

    runtime.running = Some(running);
    runtime.capture_child = Some(capture_arc);
    runtime.playback_child = Some(playback_arc);
    runtime.join = Some(join);
    runtime.null_sink_module = Some(sink_module);
    runtime.shared = Some(shared);
    runtime.metrics = Some(metrics);
    runtime.baked_source = source_id;
    runtime.baked_model_path = model_path;

    info!(
        "rvc_live: chain started (source={:?}, model={})",
        runtime.baked_source,
        runtime.baked_model_path.display()
    );
    Some(RVC_MIC_SOURCE)
}

/// Stop the live chain (if any) and release the null sink. Idempotent.
pub async fn teardown_rvc(runtime: &mut RvcLiveRuntime) {
    if let Some(running) = runtime.running.take() {
        running.store(false, Ordering::Relaxed);
    }
    if let Some(child) = runtime.capture_child.take() {
        if let Ok(mut c) = child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
    if let Some(child) = runtime.playback_child.take() {
        if let Ok(mut c) = child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
    if let Some(join) = runtime.join.take() {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), join).await;
    }
    if let Some(id) = runtime.null_sink_module.take() {
        let _ = crate::audio::unload_module_by_id(id).await;
    }
    runtime.shared = None;
    runtime.metrics = None;
    runtime.baked_source.clear();
    runtime.baked_model_path = PathBuf::new();
}

// ── Inference loop (blocking) ─────────────────────────────────────────────────

/// Runs on a `spawn_blocking` thread — real ONNX inference is synchronous
/// work, same reasoning as `vc_calibration.rs`'s render task, except this
/// loop runs indefinitely (until `running` is cleared) rather than for a
/// bounded number of variants. Blocked reads from `capture` are unblocked by
/// `teardown_rvc` killing the child, which turns the read into an `Err`.
// One parameter per piece of state the loop needs — same reasoning as
// `dbus.rs::start_dbus_service`'s own `#[allow]`.
#[allow(clippy::too_many_arguments)]
fn run_live_loop(
    running: Arc<AtomicBool>,
    capture: Arc<StdMutex<StdChild>>,
    playback: Arc<StdMutex<StdChild>>,
    shared: Arc<StdMutex<SharedState>>,
    metrics: Arc<StdMutex<RvcLiveMetrics>>,
    base_models_dir: PathBuf,
    dylib_path: PathBuf,
    model: LiveModel,
    signal_tx: broadcast::Sender<SignalEvent>,
) {
    if let Err(e) = init_runtime(&dylib_path) {
        set_metric_error(&running, &metrics, &signal_tx, format!("init_runtime: {e}"));
        return;
    }
    let hubert = match ContentVecSession::load(&base_models_dir.join(CONTENTVEC_FILENAME)) {
        Ok(s) => s,
        Err(e) => {
            set_metric_error(
                &running,
                &metrics,
                &signal_tx,
                format!("load ContentVec: {e}"),
            );
            return;
        }
    };
    let rmvpe = match RmvpeSession::load(&base_models_dir.join(RMVPE_FILENAME)) {
        Ok(s) => s,
        Err(e) => {
            set_metric_error(&running, &metrics, &signal_tx, format!("load RMVPE: {e}"));
            return;
        }
    };
    let synth = match SynthSession::load(&model.onnx_path) {
        Ok(s) => s,
        Err(e) => {
            set_metric_error(
                &running,
                &metrics,
                &signal_tx,
                format!("load synthesizer: {e}"),
            );
            return;
        }
    };
    let Some(sample_rate) = synth.native_sample_rate().or(model.sample_rate_hint) else {
        set_metric_error(
            &running,
            &metrics,
            &signal_tx,
            format!(
                "{} has no embedded sample rate (re-export with a newer export_onnx.py, \
                 or set the model's sample rate manually once)",
                model.onnx_path.display()
            ),
        );
        return;
    };

    let (initial_params, _) = match shared.lock() {
        Ok(g) => (g.params.clone(), g.pitch_offset),
        Err(_) => (RvcParams::default(), 0.0),
    };
    let retrieval = if initial_params.index_rate > 0.0 {
        match &model.index_path {
            Some(p) => match RetrievalIndex::load(p) {
                Ok(idx) => Some(idx),
                Err(e) => {
                    warn!("rvc_live: load retrieval index: {e} — continuing without it");
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };

    let mut pipeline = Pipeline::new(hubert, rmvpe, synth, sample_rate, initial_params, retrieval);

    let mut buf = vec![0u8; CAPTURE_HOP_BYTES];
    let mut hop_count = 0u64;
    let mut ema_ms = 0f32;
    // One-time VAD gate calibration from this session's own real leading
    // noise floor — see `Pipeline::set_gate_calibration`'s doc comment for
    // why the hardcoded defaults (tuned assuming already-normalized input)
    // can leave the gate never opening on a raw, unboosted live capture.
    // `None` once done (calibrated or given up), so this only ever runs once.
    let mut gate_calibration_buf: Option<Vec<f32>> =
        Some(Vec::with_capacity(GATE_CALIBRATION_WINDOW_SAMPLES));
    info!("rvc_live: inference loop started (model_sr={sample_rate})");

    while running.load(Ordering::Relaxed) {
        let read_result = {
            let mut child = match capture.lock() {
                Ok(c) => c,
                Err(_) => break,
            };
            let Some(stdout) = child.stdout.as_mut() else {
                break;
            };
            stdout.read_exact(&mut buf)
        };
        if read_result.is_err() || !running.load(Ordering::Relaxed) {
            break;
        }

        let mono = downmix_stereo_f32_bytes(&buf);

        if let Some(acc) = gate_calibration_buf.as_mut() {
            acc.extend_from_slice(&mono);
            if acc.len() >= GATE_CALIBRATION_WINDOW_SAMPLES {
                match decide_gate_calibration(acc) {
                    Some(gate) => {
                        info!(
                            "rvc_live: gate calibrated from measured noise floor \
                             (vad_rms={:.5}, knee_floor={:.5})",
                            gate.vad_rms, gate.knee_floor
                        );
                        pipeline.set_gate_calibration(gate);
                    }
                    None => {
                        info!(
                            "rvc_live: no clean leading noise floor in the first {:.1}s \
                             — keeping default gate thresholds",
                            GATE_CALIBRATION_WINDOW_SAMPLES as f32 / CAPTURE_SR as f32
                        );
                    }
                }
                gate_calibration_buf = None;
            }
        }

        let (params, pitch_offset) = match shared.lock() {
            Ok(g) => (g.params.clone(), g.pitch_offset),
            Err(_) => (RvcParams::default(), 0.0),
        };
        pipeline.set_params(params);

        let t0 = Instant::now();
        let out = match pipeline.convert(&mono, CAPTURE_SR, pitch_offset) {
            Ok(o) => o,
            Err(e) => {
                warn!("rvc_live: convert: {e}");
                continue;
            }
        };
        let dt_ms = t0.elapsed().as_secs_f32() * 1000.0;

        let pcm: Vec<u8> = out
            .iter()
            .flat_map(|&v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
            .collect();
        {
            let mut child = match playback.lock() {
                Ok(c) => c,
                Err(_) => break,
            };
            let Some(stdin) = child.stdin.as_mut() else {
                break;
            };
            if stdin.write_all(&pcm).is_err() {
                break;
            }
        }

        hop_count += 1;
        ema_ms = if hop_count == 1 {
            dt_ms
        } else {
            0.9 * ema_ms + 0.1 * dt_ms
        };
        if let Ok(mut m) = metrics.lock() {
            m.active = true;
            m.hops_processed = hop_count;
            m.avg_hop_ms = ema_ms;
            m.last_error = None;
        }
    }

    if let Ok(mut m) = metrics.lock() {
        m.active = false;
    }
    info!("rvc_live: inference loop exiting (hops={hop_count})");
}

/// Reports a fatal setup-phase failure (session/model load) and — crucially
/// — clears `running` so `RvcLiveRuntime::is_active()` stops lying once
/// this thread has actually died. Without this, `apply_rvc`'s "same
/// source + same model already running, just push params" fast path would
/// believe a dead chain was still alive after any setup failure (a missing
/// cuDNN, a corrupt `.onnx`, ...), silently no-op-ing every later
/// `SetVCSettings`/param push instead of ever retrying — found live via a
/// real cuDNN-missing failure that left the sidetone preview permanently
/// silent with no way to recover short of a full daemon restart.
fn set_metric_error(
    running: &Arc<AtomicBool>,
    metrics: &Arc<StdMutex<RvcLiveMetrics>>,
    signal_tx: &broadcast::Sender<SignalEvent>,
    msg: String,
) {
    running.store(false, Ordering::Relaxed);
    if let Ok(mut m) = metrics.lock() {
        m.active = false;
        m.last_error = Some(msg.clone());
    }
    report_error(signal_tx, msg);
}

/// Logs and sends [`SignalEvent::VCLiveChainError`] — `broadcast::Sender::send`
/// is synchronous (no runtime needed), so this is safe to call from both
/// `apply_rvc`'s async pre-thread failure paths and `run_live_loop`'s
/// blocking thread (via [`set_metric_error`]) alike.
fn report_error(signal_tx: &broadcast::Sender<SignalEvent>, msg: String) {
    error!("rvc_live: {msg}");
    let _ = signal_tx.send(SignalEvent::VCLiveChainError { message: msg });
}

/// Forwards `child`'s stderr to `warn!`, one line at a time, on its own OS
/// thread for the process's whole lifetime — same shape as the legacy
/// Python `RVCVoiceChanger._log_stderr`. Previously discarded
/// (`Stdio::null()`), which meant any real `pw-record`/`pacat` failure or
/// warning (buffer xruns, a rejected format, a permission error, ...) was
/// invisible: hops kept "processing" (the silence-branch, near-zero cost)
/// with no error surfaced anywhere, indistinguishable in the daemon's own
/// logs/metrics from a healthy chain that simply never heard real speech.
fn spawn_stderr_logger(stderr: std::process::ChildStderr, label: &'static str) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            if !line.is_empty() {
                warn!("rvc_live: {label}: {line}");
            }
        }
    });
}

/// Pure decision extracted from `run_live_loop` so it's unit-testable
/// without a real `Pipeline`/ONNX runtime: given the session's own leading
/// capture, what (if any) gate thresholds should replace the hardcoded
/// defaults? `None` when `vc_dsp::detect_leading_noise_floor` can't find a
/// clean leading-quiet stretch (buffer too short, or loud from the very
/// start) — the hardcoded defaults are kept in that case, same as before
/// this existed.
fn decide_gate_calibration(leading_samples: &[f32]) -> Option<GateCalibration> {
    let floor = crate::vc_dsp::detect_leading_noise_floor(leading_samples, CAPTURE_SR)?;
    let cal = crate::vc_dsp::calibrate_gate_from_noise_floor(floor);
    Some(GateCalibration {
        vad_rms: cal.vad_rms,
        knee_floor: cal.knee_floor,
    })
}

/// Average stereo f32 (native-endian-agnostic — always parsed as LE, matches
/// `pw-record`'s output) into mono, guarding against non-finite samples and
/// clamping — same approach as `vc_calibration.rs::downmix_stereo_f32_to_mono`,
/// producing `f32` rather than `i16` since this feeds `Pipeline::convert`
/// directly instead of a WAV file.
fn downmix_stereo_f32_bytes(buf: &[u8]) -> Vec<f32> {
    buf.as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            let l = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            let r = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
            let l = if l.is_finite() { l } else { 0.0 };
            let r = if r.is_finite() { r } else { 0.0 };
            ((l + r) * 0.5).clamp(-1.0, 1.0)
        })
        .collect()
}

// ── Stale sink detection (pure parse + async lookup) ────────────────────────

async fn find_existing_sink_module() -> Option<u32> {
    let out = tokio::process::Command::new("pactl")
        .args(["-f", "json", "list", "sinks"])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())?;
    parse_existing_sink_module(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parse of `pactl -f json list sinks`, looking for `owner_module` of
/// the sink named [`RVC_SINK`] — same shape/robustness as
/// `mic_router.rs::parse_existing_module` (numeric or string `owner_module`,
/// non-matching entries skipped rather than aborting the scan).
fn parse_existing_sink_module(json: &str) -> Option<u32> {
    let sinks: serde_json::Value = serde_json::from_str(json).ok()?;
    for sink in sinks.as_array()? {
        let name = sink["properties"]["node.name"]
            .as_str()
            .or_else(|| sink["name"].as_str());
        if name != Some(RVC_SINK) {
            continue;
        }
        let owner = &sink["owner_module"];
        if let Some(id) = owner.as_u64() {
            return Some(id as u32);
        }
        if let Some(id) = owner.as_str().and_then(|s| s.parse().ok()) {
            return Some(id);
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_runtime_is_inactive() {
        let rt = RvcLiveRuntime::new();
        assert!(!rt.is_active());
        assert_eq!(rt.metrics_snapshot(), RvcLiveMetrics::default());
    }

    #[test]
    fn set_live_params_fails_when_inactive() {
        let rt = RvcLiveRuntime::new();
        assert!(!rt.set_live_params(RvcParams::default()));
    }

    #[test]
    fn set_metric_error_clears_running_flag() {
        // A fatal setup-phase failure must flip `running` to false, not
        // just report the error in metrics — otherwise `is_active()` keeps
        // reporting a dead loop as alive (the bug this test guards).
        let running = Arc::new(AtomicBool::new(true));
        let metrics = Arc::new(StdMutex::new(RvcLiveMetrics {
            active: true,
            ..Default::default()
        }));
        let (signal_tx, _rx) = broadcast::channel(4);
        set_metric_error(&running, &metrics, &signal_tx, "boom".to_owned());
        assert!(!running.load(Ordering::Relaxed));
        let snapshot = metrics.lock().unwrap().clone();
        assert!(!snapshot.active);
        assert_eq!(snapshot.last_error.as_deref(), Some("boom"));
    }

    #[test]
    fn mic_source_matches_sink_monitor_name() {
        assert_eq!(RVC_MIC_SOURCE, format!("{RVC_SINK}.monitor"));
    }

    // ── downmix_stereo_f32_bytes ────────────────────────────────────────

    #[test]
    fn downmix_averages_left_right() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&(-1.0f32).to_le_bytes());
        let mono = downmix_stereo_f32_bytes(&buf);
        assert_eq!(mono, vec![0.0]);
    }

    #[test]
    fn downmix_guards_non_finite_and_clamps() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&f32::NAN.to_le_bytes());
        buf.extend_from_slice(&2.0f32.to_le_bytes());
        let mono = downmix_stereo_f32_bytes(&buf);
        // NaN -> 0.0, so avg = (0.0 + 2.0) / 2 = 1.0, then clamped to 1.0 anyway.
        assert_eq!(mono, vec![1.0]);
    }

    // ── decide_gate_calibration ──────────────────────────────────────────

    /// Deterministic pseudo-noise (xorshift32) — a flat/constant buffer has
    /// zero frame-to-frame variance and isn't a realistic stand-in for real
    /// room tone, same reasoning as `vc_dsp.rs`'s own test helper of the
    /// same name/shape.
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
    fn decide_gate_calibration_uses_the_measured_floor_when_a_clean_quiet_prefix_exists() {
        let mut samples = noise(CAPTURE_SR as usize, 0.0008, 1); // 1s quiet room tone
        samples.extend(noise(CAPTURE_SR as usize / 2, 0.15, 2)); // 0.5s "speech"
        let gate = decide_gate_calibration(&samples).expect("should calibrate");
        // Matches `calibrate_gate_from_noise_floor`'s own margins over
        // whatever floor `detect_leading_noise_floor` measures here —
        // this test only checks the wiring, not the underlying math
        // (already covered by `vc_dsp.rs`'s own tests).
        assert!(
            gate.vad_rms > 0.0 && gate.vad_rms < 0.01,
            "{}",
            gate.vad_rms
        );
        assert!(gate.knee_floor > gate.vad_rms);
    }

    #[test]
    fn decide_gate_calibration_none_when_loud_from_the_start() {
        let samples = noise(CAPTURE_SR as usize, 0.2, 3); // no clean quiet prefix
        assert!(decide_gate_calibration(&samples).is_none());
    }

    #[test]
    fn decide_gate_calibration_none_for_a_too_short_buffer() {
        let samples = noise(50, 0.001, 4); // well under one 20ms window
        assert!(decide_gate_calibration(&samples).is_none());
    }

    // ── parse_existing_sink_module (pure) ───────────────────────────────

    #[test]
    fn parse_existing_sink_module_accepts_numeric_owner_module() {
        let json = r#"[{"name": "Arctis_VC_Sink", "owner_module": 123,
            "properties": {"node.name": "Arctis_VC_Sink"}}]"#;
        assert_eq!(parse_existing_sink_module(json), Some(123));
    }

    #[test]
    fn parse_existing_sink_module_accepts_string_owner_module() {
        let json = r#"[{"name": "Arctis_VC_Sink", "owner_module": "42",
            "properties": {"node.name": "Arctis_VC_Sink"}}]"#;
        assert_eq!(parse_existing_sink_module(json), Some(42));
    }

    #[test]
    fn parse_existing_sink_module_skips_non_matching_entries() {
        let json = r#"[
            {"properties": {}},
            {"name": "Arctis_Media", "owner_module": 1, "properties": {}},
            {"name": "Arctis_VC_Sink", "owner_module": 99,
             "properties": {"node.name": "Arctis_VC_Sink"}}
        ]"#;
        assert_eq!(parse_existing_sink_module(json), Some(99));
    }

    #[test]
    fn parse_existing_sink_module_none_when_absent() {
        let json = r#"[{"name": "something_else", "owner_module": 1, "properties": {}}]"#;
        assert_eq!(parse_existing_sink_module(json), None);
    }

    #[test]
    fn parse_existing_sink_module_none_on_bad_json() {
        assert_eq!(parse_existing_sink_module("not json"), None);
    }

    /// Not run by default — needs a real onnxruntime shared library, the
    /// real published base models, a real synthesizer exported by
    /// `export_onnx.py`, and a real, currently-capturable PipeWire source
    /// (a mic, or any node's `.monitor`). Runs the actual live chain for a
    /// few seconds against real audio and checks it produced real hops —
    /// the same kind of genuine end-to-end check `vc_export.rs`'s
    /// `live_install_export_deps_end_to_end` and `pipeline.rs`'s
    /// `live_convert_produces_sound_on_a_loud_tone` already are for their
    /// own pieces, now covering the actual D-Bus-reachable apply/teardown
    /// path (`SetVCSettings` with `mode: "rvc"` calls exactly this).
    /// Run manually with:
    /// `LAM_ORT_DYLIB_PATH=... LAM_TEST_SOURCE_ID=... \
    ///  cargo test --bin lam-daemon -- --ignored live_apply_rvc_runs_real_inference --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_apply_rvc_runs_real_inference() {
        use crate::vc::inference::engine::init_runtime;

        let dylib_path = std::env::var("LAM_ORT_DYLIB_PATH")
            .map(PathBuf::from)
            .expect("set LAM_ORT_DYLIB_PATH to a real onnxruntime shared library");
        let source_id = std::env::var("LAM_TEST_SOURCE_ID")
            .expect("set LAM_TEST_SOURCE_ID to a real capturable PipeWire source");
        let base_models_dir = std::env::var("LAM_TEST_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").expect("HOME not set"))
                    .join(".config/arctis_manager/models")
            });
        let synth_onnx_path = std::env::var("LAM_TEST_SYNTH_ONNX_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").expect("HOME not set"))
                    .join(".config/arctis_manager/rvc_models/DvaOverwatch_350e.onnx")
            });
        // `init_runtime` may only run once per process — this test owns
        // that call itself (unlike `apply_rvc`, which also calls it inside
        // the loop thread) only to fail fast with a clear message before
        // spawning anything if the dylib path is bad.
        init_runtime(&dylib_path).expect("init_runtime");

        let mut runtime = RvcLiveRuntime::new();
        let cfg = RvcConfig::default();
        let model = LiveModel {
            onnx_path: synth_onnx_path.clone(),
            sample_rate_hint: None,
            index_path: None,
        };

        let (signal_tx, _rx) = broadcast::channel(4);
        let result = apply_rvc(
            &cfg,
            model,
            source_id,
            base_models_dir,
            dylib_path,
            &mut runtime,
            signal_tx,
        )
        .await;
        assert_eq!(result, Some(RVC_MIC_SOURCE));
        assert!(runtime.is_active());

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let metrics = runtime.metrics_snapshot();
        eprintln!("live_apply_rvc_runs_real_inference: metrics={metrics:?}");
        assert!(
            metrics.hops_processed > 0,
            "expected real hops to be processed within 5s, got {metrics:?}"
        );
        assert!(
            metrics.last_error.is_none(),
            "unexpected error: {metrics:?}"
        );

        teardown_rvc(&mut runtime).await;
        assert!(!runtime.is_active());
    }
}
