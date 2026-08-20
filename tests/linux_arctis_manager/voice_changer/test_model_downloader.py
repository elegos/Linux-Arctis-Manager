from __future__ import annotations

import hashlib
import sys
from pathlib import Path
from types import ModuleType
from unittest.mock import MagicMock, patch

import pytest

from linux_arctis_manager.voice_changer.rvc.model_downloader import (
    CONTENTVEC,
    RMVPE,
    _CONTENTVEC_LEGACY,
    _ModelSpec,
    _sha256,
    base_models_status,
    download_base_models,
    model_path,
)


# ── helpers ───────────────────────────────────────────────────────────────────

def _patch_models_dir(tmp_path: Path):
    """Patch _MODELS_DIR and the CONTENTVEC legacy path to live under tmp_path."""
    return [
        patch('linux_arctis_manager.voice_changer.rvc.model_downloader._MODELS_DIR', tmp_path),
        patch('linux_arctis_manager.voice_changer.rvc.model_downloader._CONTENTVEC_LEGACY',
              tmp_path / 'contentvec_500.bin'),
    ]


# ── _sha256 ───────────────────────────────────────────────────────────────────

def test_sha256_matches_known_value(tmp_path: Path) -> None:
    f = tmp_path / 'data.bin'
    f.write_bytes(b'hello world')
    expected = hashlib.sha256(b'hello world').hexdigest()
    assert _sha256(f) == expected


# ── model_path ────────────────────────────────────────────────────────────────

def test_model_path_returns_canonical_path_when_present(tmp_path: Path) -> None:
    (tmp_path / 'rmvpe.pt').write_bytes(b'fake')
    with patch('linux_arctis_manager.voice_changer.rvc.model_downloader._MODELS_DIR', tmp_path):
        result = model_path(RMVPE)
    assert result == tmp_path / 'rmvpe.pt'


def test_model_path_returns_none_when_absent(tmp_path: Path) -> None:
    patches = _patch_models_dir(tmp_path)
    with patches[0], patches[1]:
        assert model_path(RMVPE) is None


def test_model_path_contentvec_falls_back_to_legacy(tmp_path: Path) -> None:
    legacy = tmp_path / 'contentvec_500.bin'
    legacy.write_bytes(b'legacy')
    patches = _patch_models_dir(tmp_path)
    with patches[0], patches[1]:
        result = model_path(CONTENTVEC)
    assert result == legacy


def test_model_path_contentvec_prefers_canonical_over_legacy(tmp_path: Path) -> None:
    canonical = tmp_path / 'content_vec_best.bin'
    canonical.write_bytes(b'canonical')
    legacy = tmp_path / 'contentvec_500.bin'
    legacy.write_bytes(b'legacy')
    patches = _patch_models_dir(tmp_path)
    with patches[0], patches[1]:
        result = model_path(CONTENTVEC)
    assert result == canonical


# ── base_models_status ────────────────────────────────────────────────────────

def test_base_models_status_all_missing(tmp_path: Path) -> None:
    patches = _patch_models_dir(tmp_path)
    with patches[0], patches[1]:
        status = base_models_status()
    assert status == {'rmvpe': False, 'contentvec': False}


def test_base_models_status_all_present(tmp_path: Path) -> None:
    (tmp_path / 'rmvpe.pt').write_bytes(b'r')
    (tmp_path / 'content_vec_best.bin').write_bytes(b'c')
    patches = _patch_models_dir(tmp_path)
    with patches[0], patches[1]:
        status = base_models_status()
    assert status == {'rmvpe': True, 'contentvec': True}


def test_base_models_status_partial(tmp_path: Path) -> None:
    (tmp_path / 'rmvpe.pt').write_bytes(b'r')
    patches = _patch_models_dir(tmp_path)
    with patches[0], patches[1]:
        status = base_models_status()
    assert status == {'rmvpe': True, 'contentvec': False}


# ── download_base_models ──────────────────────────────────────────────────────

def test_download_skips_already_present_models(tmp_path: Path) -> None:
    (tmp_path / 'rmvpe.pt').write_bytes(b'exists')
    (tmp_path / 'content_vec_best.bin').write_bytes(b'exists')
    patches = _patch_models_dir(tmp_path)
    with patches[0], patches[1], patch('urllib.request.urlretrieve') as mock_retrieve:
        download_base_models()
    mock_retrieve.assert_not_called()


def test_download_calls_urlretrieve_for_missing_models(tmp_path: Path) -> None:
    rmvpe_data = b'rmvpe-data'
    cv_data = b'cv-data'
    sha_rmvpe = hashlib.sha256(rmvpe_data).hexdigest()
    sha_cv = hashlib.sha256(cv_data).hexdigest()

    def fake_retrieve(url: str, dest: str | Path, reporthook=None) -> None:
        data = rmvpe_data if 'rmvpe' in url else cv_data
        Path(dest).write_bytes(data)

    patches = _patch_models_dir(tmp_path)
    with (
        patches[0],
        patches[1],
        patch('urllib.request.urlretrieve', side_effect=fake_retrieve),
        patch('linux_arctis_manager.voice_changer.rvc.model_downloader.RMVPE',
              _ModelSpec('rmvpe.pt', sha_rmvpe)),
        patch('linux_arctis_manager.voice_changer.rvc.model_downloader.CONTENTVEC',
              _ModelSpec('content_vec_best.bin', sha_cv)),
    ):
        download_base_models()

    assert (tmp_path / 'rmvpe.pt').exists()
    assert (tmp_path / 'content_vec_best.bin').exists()


def test_download_raises_on_checksum_mismatch(tmp_path: Path) -> None:
    def fake_retrieve(url: str, dest: str | Path, reporthook=None) -> None:
        Path(dest).write_bytes(b'corrupted')

    patches = _patch_models_dir(tmp_path)
    with patches[0], patches[1], patch('urllib.request.urlretrieve', side_effect=fake_retrieve):
        with pytest.raises(ValueError, match='Checksum mismatch'):
            download_base_models()
    # Temp file must be cleaned up on failure
    assert not list(tmp_path.glob('*.tmp'))


def test_download_cleanup_tmp_on_network_error(tmp_path: Path) -> None:
    def fake_retrieve(url: str, dest: str | Path, reporthook=None) -> None:
        Path(dest).write_bytes(b'partial')
        raise OSError('network error')

    patches = _patch_models_dir(tmp_path)
    with patches[0], patches[1], patch('urllib.request.urlretrieve', side_effect=fake_retrieve):
        with pytest.raises(OSError):
            download_base_models()
    assert not list(tmp_path.glob('*.tmp'))


def test_download_progress_callback_receives_messages(tmp_path: Path) -> None:
    rmvpe_data = b'r'
    cv_data = b'c'
    sha_r = hashlib.sha256(rmvpe_data).hexdigest()
    sha_c = hashlib.sha256(cv_data).hexdigest()

    def fake_retrieve(url: str, dest: str | Path, reporthook=None) -> None:
        data = rmvpe_data if 'rmvpe' in url else cv_data
        Path(dest).write_bytes(data)
        if reporthook:
            reporthook(1, len(data), len(data))

    messages: list[str] = []
    patches = _patch_models_dir(tmp_path)
    with (
        patches[0],
        patches[1],
        patch('urllib.request.urlretrieve', side_effect=fake_retrieve),
        patch('linux_arctis_manager.voice_changer.rvc.model_downloader.RMVPE',
              _ModelSpec('rmvpe.pt', sha_r)),
        patch('linux_arctis_manager.voice_changer.rvc.model_downloader.CONTENTVEC',
              _ModelSpec('content_vec_best.bin', sha_c)),
    ):
        download_base_models(progress_cb=messages.append)

    assert any('rmvpe' in m for m in messages), f'No rmvpe message in {messages}'
    assert any('content_vec_best' in m for m in messages), f'No contentvec message in {messages}'


# ── ensure_model (rmvpe.py) — mock torch/numpy so no GPU required ─────────────

def _stub_heavy_imports() -> dict:
    """Return sys.modules overrides that prevent torch/numpy import errors."""
    stubs: dict[str, MagicMock] = {}
    for name in ('torch', 'torch.nn', 'torch.nn.functional', 'numpy', 'torchaudio',
                 'torchaudio.functional'):
        stubs[name] = MagicMock()
    return stubs


def test_ensure_model_returns_path_when_present(tmp_path: Path) -> None:
    rmvpe_file = tmp_path / 'rmvpe.pt'
    rmvpe_file.write_bytes(b'fake')

    with patch.dict(sys.modules, _stub_heavy_imports()):
        from linux_arctis_manager.voice_changer.rvc import rmvpe as rmvpe_mod
        with patch.object(rmvpe_mod, 'ensure_model', wraps=rmvpe_mod.ensure_model):
            with patch('linux_arctis_manager.voice_changer.rvc.model_downloader.model_path',
                       return_value=rmvpe_file):
                result = rmvpe_mod.ensure_model()
    assert result == rmvpe_file


def test_ensure_model_raises_when_missing(tmp_path: Path) -> None:
    with patch.dict(sys.modules, _stub_heavy_imports()):
        from linux_arctis_manager.voice_changer.rvc import rmvpe as rmvpe_mod
        with patch('linux_arctis_manager.voice_changer.rvc.model_downloader.model_path',
                   return_value=None):
            with pytest.raises(FileNotFoundError):
                rmvpe_mod.ensure_model()


# ── _ensure_contentvec (pipeline.py) — mock heavy imports ────────────────────

def test_ensure_contentvec_returns_path_when_present(tmp_path: Path) -> None:
    cv_file = tmp_path / 'content_vec_best.bin'
    cv_file.write_bytes(b'fake')

    heavy = _stub_heavy_imports()
    heavy['torchaudio.pipelines'] = MagicMock()
    with patch.dict(sys.modules, heavy):
        from linux_arctis_manager.voice_changer.rvc import pipeline as pl_mod
        with patch('linux_arctis_manager.voice_changer.rvc.model_downloader.model_path',
                   return_value=cv_file):
            result = pl_mod._ensure_contentvec()
    assert result == cv_file


def test_ensure_contentvec_raises_when_missing(tmp_path: Path) -> None:
    heavy = _stub_heavy_imports()
    heavy['torchaudio.pipelines'] = MagicMock()
    with patch.dict(sys.modules, heavy):
        from linux_arctis_manager.voice_changer.rvc import pipeline as pl_mod
        with patch('linux_arctis_manager.voice_changer.rvc.model_downloader.model_path',
                   return_value=None):
            with pytest.raises(FileNotFoundError):
                pl_mod._ensure_contentvec()
