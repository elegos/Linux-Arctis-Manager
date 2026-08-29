// Guided voice calibration — recording half.
//
// Direct port of `voice_changer/rvc/calibration.py`'s `CalibrationSession`
// (record_start/record_stop, the f32-stereo→mono downmix, peak detection,
// and `propose_variants`).
//
// `render_start`/`_render` are NOT ported here: they require the actual RVC
// inference pipeline (ContentVec → RMVPE → synthesizer) to convert the
// recording through candidate parameter sets, and that pipeline does not
// exist in Rust yet — see [E10-S6] in docs/v3-backlog.md. `CalibrationState`
// still declares `Rendering`/`Done` for the eventual full D-Bus contract
// shape, but this module can only ever produce `Idle`/`Recording`/
// `Recorded`/`Error` until rendering lands.
//
// Not yet wired into dbus.rs — the `VcInterface` D-Bus service lands in a
// later phase ([E10-S5], see docs/voice-changing-feature.md).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::vc_rvc_config::RvcParams;

pub const RECORD_SAMPLE_RATE: u32 = 16000;
const MAX_RECORD_SECS: u64 = 120;
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

// ── Audio helpers (pure) ─────────────────────────────────────────────────────

/// Downmix raw interleaved f32 stereo PCM bytes to mono i16 samples (average
/// of the two channels, clamped to [-1, 1]), and the peak absolute mono
/// amplitude (0..1). Non-finite samples (NaN/Inf) are treated as silence.
/// A trailing partial frame (not a multiple of 8 bytes) is dropped.
pub fn downmix_stereo_f32_to_mono(buf: &[u8]) -> (Vec<i16>, f32) {
    let usable = buf.len() - (buf.len() % BYTES_PER_FRAME as usize);
    let mut peak = 0.0f32;
    let mut mono = Vec::with_capacity(usable / BYTES_PER_FRAME as usize);
    for frame in buf[..usable].chunks_exact(BYTES_PER_FRAME as usize) {
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
}
