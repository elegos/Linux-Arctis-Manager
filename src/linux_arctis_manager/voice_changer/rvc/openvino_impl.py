from __future__ import annotations

import logging
from pathlib import Path

from linux_arctis_manager.voice_changer.rvc.backend import RVCBackend

logger = logging.getLogger('OpenVINORVCBackend')


class OpenVINORVCBackend(RVCBackend):
    """
    RVC inference using Intel OpenVINO.

    Supports Intel CPUs, iGPUs, Arc GPUs, and NPUs via openvino.Core.
    Requires: pip install openvino
    Models must be exported from PyTorch to OpenVINO IR (.xml/.bin) first.
    """

    def __init__(self) -> None:
        self._model = None
        self._device_name = ''

    def name(self) -> str:
        if not self.is_available():
            return 'OpenVINO (unavailable)'
        device = self._best_device()
        return f'OpenVINO ({device})'

    def is_available(self) -> bool:
        try:
            import openvino
            return bool(self._best_device())
        except ImportError:
            return False

    def _best_device(self) -> str:
        """Return the best available OpenVINO device (NPU > GPU > CPU)."""
        try:
            from openvino import Core
            core = Core()
            devices = core.available_devices
            for preferred in ('NPU', 'GPU', 'CPU'):
                if preferred in devices:
                    return preferred
        except Exception:
            pass
        return ''

    def load_model(self, path: Path) -> None:
        from openvino import Core
        core = Core()
        self._device_name = self._best_device()
        xml_path = path.with_suffix('.xml')
        if not xml_path.exists():
            raise FileNotFoundError(
                f'OpenVINO model not found: {xml_path}\n'
                f'Export the .pth model to OpenVINO IR format first.'
            )
        self._model = core.compile_model(str(xml_path), self._device_name)
        logger.info('Loaded OpenVINO RVC model from %s on %s', xml_path, self._device_name)

    def unload_model(self) -> None:
        self._model = None
        logger.info('OpenVINO RVC model unloaded')

    def convert(self, audio: 'np.ndarray', sr: int, pitch_offset: float) -> 'np.ndarray':
        if self._model is None:
            return audio
        # Full pipeline analogous to PyTorchRVCBackend but using OpenVINO IR models
        # for HuBERT, RMVPE, and the generator.
        logger.debug('OpenVINO RVC convert: passthrough (full model integration pending)')
        return audio
