from pathlib import Path

import pytest

from linux_arctis_manager.voice_changer.rvc.backend import RVCBackend, RVCParams


def test_rvc_params_defaults():
    params = RVCParams()
    assert params.hubert_model == 'torchaudio'
    assert params.vtln_alpha == 1.0
    assert params.rms_mix_rate == 0.25
    assert params.filter_radius == 3
    assert params.target_rms == 0.06
    assert params.limiter_thr == 0.80
    assert params.index_rate == 0.0


def test_rvc_params_overrides():
    params = RVCParams(hubert_model='contentvec', filter_radius=5)
    assert params.hubert_model == 'contentvec'
    assert params.filter_radius == 5


def test_rvc_backend_is_abstract():
    with pytest.raises(TypeError):
        RVCBackend()  # type: ignore[abstract]


class _StubBackend(RVCBackend):
    def name(self) -> str:
        return 'Stub'

    def is_available(self) -> bool:
        return True

    def load_model(self, path: Path, params: RVCParams | None = None) -> None:
        pass

    def unload_model(self) -> None:
        pass

    def convert(self, audio, sr: int, pitch_offset: float):
        return audio


def test_default_update_params_returns_false():
    backend = _StubBackend()
    assert backend.update_params(RVCParams()) is False


def test_default_get_metrics_returns_none():
    backend = _StubBackend()
    assert backend.get_metrics() is None


def test_concrete_backend_behavior():
    backend = _StubBackend()
    assert backend.name() == 'Stub'
    assert backend.is_available() is True
    assert backend.convert([1.0, 2.0], 16000, 0.0) == [1.0, 2.0]
