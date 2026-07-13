from __future__ import annotations

import logging

from linux_arctis_manager.voice_changer.rvc.backend import RVCBackend

logger = logging.getLogger('BackendRegistry')


class BackendRegistry:
    """
    Discovers and returns the best available RVC backend.
    Preference order: PyTorch (GPU) > OpenVINO (Intel NPU/GPU) > None.
    """

    @staticmethod
    def best_backend() -> RVCBackend | None:
        from linux_arctis_manager.voice_changer.rvc.pytorch_impl import PyTorchRVCBackend
        from linux_arctis_manager.voice_changer.rvc.openvino_impl import OpenVINORVCBackend

        for cls in (PyTorchRVCBackend, OpenVINORVCBackend):
            try:
                backend = cls()
                if backend.is_available():
                    logger.info('BackendRegistry: selected %s', backend.name())
                    return backend
            except Exception as e:
                logger.debug('BackendRegistry: %s unavailable: %s', cls.__name__, e)

        logger.info('BackendRegistry: no compatible GPU backend found')
        return None

    @staticmethod
    def available_backends() -> list[str]:
        """Return names of all available backends (for capability reporting)."""
        from linux_arctis_manager.voice_changer.rvc.pytorch_impl import PyTorchRVCBackend
        from linux_arctis_manager.voice_changer.rvc.openvino_impl import OpenVINORVCBackend

        result = []
        for cls in (PyTorchRVCBackend, OpenVINORVCBackend):
            try:
                b = cls()
                if b.is_available():
                    result.append(b.name())
            except Exception:
                pass
        return result
