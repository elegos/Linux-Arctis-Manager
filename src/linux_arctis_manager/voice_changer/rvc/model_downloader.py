from __future__ import annotations

import hashlib
import logging
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

logger = logging.getLogger('ModelDownloader')

_BASE_URL   = 'https://github.com/elegos/Linux-Arctis-Manager-AI-Models/releases/download/v1'
_MODELS_DIR = Path.home() / '.config' / 'arctis_manager' / 'models'


@dataclass(frozen=True)
class _ModelSpec:
    filename: str
    sha256: str

    @property
    def url(self) -> str:
        return f'{_BASE_URL}/{self.filename}'

    @property
    def path(self) -> Path:
        return _MODELS_DIR / self.filename


RMVPE = _ModelSpec(
    filename='rmvpe.pt',
    sha256='6d62215f4306e3ca278246188607209f09af3dc77ed4232efdd069798c4ec193',
)

CONTENTVEC = _ModelSpec(
    filename='content_vec_best.bin',
    sha256='d8dd400e054ddf4e6be75dab5a2549db748cc99e756a097c496c099f65a4854e',
)

# Backward-compat: old installations stored ContentVec under a different name.
_CONTENTVEC_LEGACY = _MODELS_DIR / 'contentvec_500.bin'


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(65536), b''):
            h.update(chunk)
    return h.hexdigest()


def model_path(spec: _ModelSpec) -> Path | None:
    """Return the local path for *spec* if it exists (checks legacy paths too)."""
    if spec.path.exists():
        return spec.path
    if spec is CONTENTVEC and _CONTENTVEC_LEGACY.exists():
        return _CONTENTVEC_LEGACY
    return None


def base_models_status() -> dict:
    """Return {'rmvpe': bool, 'contentvec': bool} — True when the file is on disk."""
    return {
        'rmvpe':     model_path(RMVPE) is not None,
        'contentvec': model_path(CONTENTVEC) is not None,
    }


def _download_file(
    spec: _ModelSpec,
    progress_cb: Callable[[str], None] | None,
) -> None:
    _MODELS_DIR.mkdir(parents=True, exist_ok=True)
    tmp = spec.path.with_suffix('.tmp')
    try:
        def _hook(count: int, block_size: int, total_size: int) -> None:
            if progress_cb and total_size > 0:
                pct = min(100, count * block_size * 100 // total_size)
                progress_cb(f'{spec.filename}: {pct}%')

        logger.info('Downloading %s from %s', spec.filename, spec.url)
        urllib.request.urlretrieve(spec.url, tmp, reporthook=_hook)

        if progress_cb:
            progress_cb(f'Verifying {spec.filename}...')
        actual = _sha256(tmp)
        if actual != spec.sha256:
            raise ValueError(
                f'Checksum mismatch for {spec.filename}: '
                f'expected {spec.sha256}, got {actual}'
            )
        tmp.rename(spec.path)
        logger.info('%s saved and verified at %s', spec.filename, spec.path)
    except Exception:
        tmp.unlink(missing_ok=True)
        raise


def download_base_models(progress_cb: Callable[[str], None] | None = None) -> None:
    """Download RMVPE and ContentVec from the official release, verifying SHA-256."""
    for spec in (RMVPE, CONTENTVEC):
        if model_path(spec) is not None:
            logger.info('%s already present — skipping download', spec.filename)
            if progress_cb:
                progress_cb(f'{spec.filename}: already present.')
            continue
        if progress_cb:
            progress_cb(f'Downloading {spec.filename}...')
        _download_file(spec, progress_cb)
