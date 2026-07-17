from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path


@dataclass
class RVCParams:
    """Per-model inference tuning, mirroring the RVC WebUI advanced options."""
    hubert_model:  str   = 'torchaudio'  # 'torchaudio' | 'contentvec'
    vtln_alpha:    float = 1.0    # <1 shifts formants up (male→female); 1 = off
    rms_mix_rate:  float = 0.25   # 0 = output follows input envelope, 1 = model's own envelope
    filter_radius: int   = 3      # F0 median filter length (odd; <3 = off)
    target_rms:    float = 0.06   # input drive into the model (higher = louder but risks saturation)
    limiter_thr:   float = 0.80   # output soft-limiter knee (1.0 = off)
    index_rate:    float = 0.0    # FAISS feature-retrieval blend (0 = off; needs a .index file)


class RVCBackend(ABC):
    """Abstract real-time voice conversion backend."""

    @abstractmethod
    def name(self) -> str:
        """Human-readable backend identifier (e.g. 'PyTorch CUDA')."""

    @abstractmethod
    def is_available(self) -> bool:
        """Return True if the required runtime libraries and GPU are present."""

    @abstractmethod
    def load_model(self, path: Path, params: RVCParams | None = None) -> None:
        """Load a .pth model file with the given tuning params. Raises on failure."""

    @abstractmethod
    def unload_model(self) -> None:
        """Release the loaded model and free GPU memory."""

    def update_params(self, params: RVCParams) -> bool:
        """Swap tuning params live without reloading the model. Optional."""
        return False

    def get_metrics(self) -> dict | None:
        """Drain and return per-hop quality metrics for the auto-tuner. Optional."""
        return None

    @abstractmethod
    def convert(self, audio: 'np.ndarray', sr: int, pitch_offset: float) -> 'np.ndarray':
        """
        Convert a chunk of audio samples.

        audio        – float32 numpy array, shape (N,), range [-1, 1]
        sr           – sample rate (typically 16000 or 48000)
        pitch_offset – semitones to shift the converted voice

        Returns float32 numpy array same length as input.
        """
