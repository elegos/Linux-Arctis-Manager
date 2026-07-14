from __future__ import annotations

import logging
from pathlib import Path

from linux_arctis_manager.voice_changer.rvc.backend import RVCBackend

logger = logging.getLogger('PyTorchRVCBackend')


class PyTorchRVCBackend(RVCBackend):
    """RVC inference using PyTorch (CUDA/ROCm GPU required)."""

    def __init__(self) -> None:
        self._pipeline = None

    def name(self) -> str:
        if not self.is_available():
            return 'PyTorch (unavailable)'
        try:
            import torch
            if torch.cuda.is_available():
                return f'PyTorch ({torch.cuda.get_device_name(0)})'
        except Exception:
            pass
        return 'PyTorch (CPU — too slow for real-time)'

    def is_available(self) -> bool:
        try:
            import numpy  # noqa: F401
            import torch
            return torch.cuda.is_available()
        except ImportError:
            return False

    def load_model(self, path: Path, hubert_model: str = 'torchaudio',
                   vtln_alpha: float = 1.0) -> None:
        import torch
        from linux_arctis_manager.voice_changer.rvc.pipeline import RVCPipeline
        self._pipeline = RVCPipeline()
        self._pipeline.load(path, torch.device('cuda'),
                            hubert_model=hubert_model, vtln_alpha=vtln_alpha)

    def unload_model(self) -> None:
        if self._pipeline is not None:
            self._pipeline.unload()
            self._pipeline = None

    def convert(self, audio: 'np.ndarray', sr: int, pitch_offset: float) -> 'np.ndarray':
        if self._pipeline is None:
            return audio
        return self._pipeline.convert(audio, sr, pitch_offset)
