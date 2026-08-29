// RVC per-model inference tuning parameters.
//
// Direct port of `voice_changer/rvc/backend.py`'s `RVCParams` dataclass.
// Mirrors the RVC WebUI advanced options; consumed by the (not yet built)
// inference engine ([E10-S6]) and by the calibration variant proposer
// (`vc_calibration.rs`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RvcParams {
    /// `"torchaudio"` | `"contentvec"`.
    pub hubert_model: String,
    /// <1 shifts formants up (male→female); 1 = off.
    pub vtln_alpha: f32,
    /// 0 = output follows input envelope, 1 = model's own envelope.
    pub rms_mix_rate: f32,
    /// F0 median filter length (odd; <3 = off).
    pub filter_radius: i32,
    /// Input drive into the model (higher = louder but risks saturation).
    pub target_rms: f32,
    /// Output soft-limiter knee (1.0 = off).
    pub limiter_thr: f32,
    /// FAISS feature-retrieval blend (0 = off; needs a `.index` file).
    pub index_rate: f32,
}

impl Default for RvcParams {
    fn default() -> Self {
        Self {
            hubert_model: "torchaudio".to_owned(),
            vtln_alpha: 1.0,
            rms_mix_rate: 0.25,
            filter_radius: 3,
            target_rms: 0.06,
            limiter_thr: 0.80,
            index_rate: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_default() {
        let p = RvcParams::default();
        let json = serde_json::to_string(&p).unwrap();
        let back: RvcParams = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn roundtrip_full() {
        let p = RvcParams {
            hubert_model: "contentvec".to_owned(),
            vtln_alpha: 0.88,
            rms_mix_rate: 0.6,
            filter_radius: 5,
            target_rms: 0.1,
            limiter_thr: 1.0,
            index_rate: 0.75,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: RvcParams = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
