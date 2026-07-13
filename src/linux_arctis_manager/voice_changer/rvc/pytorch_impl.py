from __future__ import annotations

import logging
from pathlib import Path

from linux_arctis_manager.voice_changer.rvc.backend import RVCBackend

logger = logging.getLogger('PyTorchRVCBackend')


class PyTorchRVCBackend(RVCBackend):
    """
    RVC inference using PyTorch.

    Supports NVIDIA (CUDA) and AMD (ROCm) GPUs via torch.cuda.
    Requires: pip install torch  (with appropriate CUDA/ROCm index URL)
    """

    def __init__(self) -> None:
        self._model = None
        self._device = None

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
            import torch
            return torch.cuda.is_available()
        except ImportError:
            return False

    def load_model(self, path: Path) -> None:
        import torch
        self._device = torch.device('cuda')
        checkpoint = torch.load(path, map_location=self._device)
        # RVC .pth checkpoints embed the generator config in checkpoint['config']
        # and the weights in checkpoint['weight'] or as a state_dict directly.
        # The exact loading depends on the RVC model variant (v1/v2).
        # We store the raw checkpoint here; subclasses or a model factory should
        # build the nn.Module from the checkpoint config.
        self._model = checkpoint
        logger.info('Loaded RVC model from %s on %s', path, self._device)

    def unload_model(self) -> None:
        self._model = None
        try:
            import torch
            torch.cuda.empty_cache()
        except Exception:
            pass
        logger.info('RVC model unloaded')

    def convert(self, audio: 'np.ndarray', sr: int, pitch_offset: float) -> 'np.ndarray':
        import numpy as np
        if self._model is None:
            return audio
        # Full RVC inference pipeline:
        #   1. Resample to 16 kHz if needed
        #   2. Extract HuBERT content features
        #   3. Extract F0 pitch with RMVPE / crepe
        #   4. Shift F0 by pitch_offset semitones
        #   5. Run VITS/SoVITS generator
        #   6. Resample back to sr
        # This requires the HuBERT and RMVPE models to be loaded separately.
        # Returning passthrough until full RVC models are available.
        logger.debug('RVC convert: passthrough (full model integration pending)')
        return audio
