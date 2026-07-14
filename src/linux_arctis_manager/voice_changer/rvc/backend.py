from __future__ import annotations

from abc import ABC, abstractmethod
from pathlib import Path


class RVCBackend(ABC):
    """Abstract real-time voice conversion backend."""

    @abstractmethod
    def name(self) -> str:
        """Human-readable backend identifier (e.g. 'PyTorch CUDA')."""

    @abstractmethod
    def is_available(self) -> bool:
        """Return True if the required runtime libraries and GPU are present."""

    @abstractmethod
    def load_model(self, path: Path, hubert_model: str = 'torchaudio',
                   vtln_alpha: float = 1.0) -> None:
        """Load a .pth model file. Raises on failure."""

    @abstractmethod
    def unload_model(self) -> None:
        """Release the loaded model and free GPU memory."""

    @abstractmethod
    def convert(self, audio: 'np.ndarray', sr: int, pitch_offset: float) -> 'np.ndarray':
        """
        Convert a chunk of audio samples.

        audio        – float32 numpy array, shape (N,), range [-1, 1]
        sr           – sample rate (typically 16000 or 48000)
        pitch_offset – semitones to shift the converted voice

        Returns float32 numpy array same length as input.
        """
