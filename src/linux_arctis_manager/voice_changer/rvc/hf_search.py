from __future__ import annotations

import logging
import zipfile
from pathlib import Path
from typing import Callable

logger = logging.getLogger('hf_search')

_MODEL_EXTENSIONS = ('.pth', '.zip')
_HF_TOKEN_FILE = Path.home() / '.config' / 'arctis_manager' / 'hf_token'


def get_hf_token() -> str:
    try:
        t = _HF_TOKEN_FILE.read_text().strip()
        if t:
            return t
    except (FileNotFoundError, OSError):
        pass
    try:
        from huggingface_hub import get_token
        return get_token() or ''
    except Exception:
        return ''


def set_hf_token(token: str) -> bool:
    try:
        _HF_TOKEN_FILE.parent.mkdir(parents=True, exist_ok=True)
        _HF_TOKEN_FILE.write_text(token.strip())
        _HF_TOKEN_FILE.chmod(0o600)
        return True
    except Exception as e:
        logger.error('set_hf_token: %s', e)
        return False


def search_models(query: str, sort_by: str = 'downloads', limit: int = 20) -> list[dict]:
    """Search HuggingFace for RVC-tagged models.

    sort_by: 'downloads' | 'likes' | 'trendingScore'
    """
    from huggingface_hub import list_models
    try:
        q = query.strip()
        results = []
        for m in list_models(
            search=q or None,
            filter=None if q else 'rvc',  # tag filter only when browsing; named search is open
            sort=sort_by if sort_by in ('downloads', 'likes', 'trendingScore') else 'downloads',
            limit=limit,
        ):
            results.append({
                'repo_id':   m.id,
                'name':      m.id.split('/')[-1],
                'author':    getattr(m, 'author', '') or '',
                'downloads': getattr(m, 'downloads', 0) or 0,
                'likes':     getattr(m, 'likes', 0) or 0,
            })
        return results
    except Exception as e:
        logger.error('HF search failed: %s', e)
        return []


def list_repo_model_files(repo_id: str) -> list[str]:
    """Return downloadable model filenames (.pth and .zip) from a HuggingFace repo."""
    from huggingface_hub import list_repo_files
    try:
        return [
            f for f in list_repo_files(repo_id)
            if any(f.endswith(ext) for ext in _MODEL_EXTENSIONS)
        ]
    except Exception as e:
        logger.error('list_repo_files %s: %s', repo_id, e)
        return []


def _extract_pth_from_zip(zip_path: Path, dest_folder: Path,
                           progress_cb: Callable[[str], None]) -> list[str]:
    """Extract .pth files from a zip archive into dest_folder. Returns stem names extracted."""
    extracted: list[str] = []
    with zipfile.ZipFile(zip_path) as zf:
        pth_members = [
            n for n in zf.namelist()
            if n.endswith('.pth') and not Path(n).name.startswith('.')
            and '__MACOSX' not in n
        ]
        if not pth_members:
            progress_cb('No .pth files found inside zip.')
            return []
        for member in pth_members:
            target = dest_folder / Path(member).name
            progress_cb(f'Extracting: {Path(member).name}')
            with zf.open(member) as src, open(target, 'wb') as dst:
                dst.write(src.read())
            extracted.append(target.stem)
    return extracted


def _make_tqdm_class(progress_cb: Callable[[str], None]):
    """Return a tqdm-compatible class that forwards progress to progress_cb."""
    class _Tqdm:
        def __init__(self, iterable=None, total=None, **kwargs):
            self.total = total
            self.n = 0
            self._iterable = iterable

        def update(self, n: int = 1) -> None:
            self.n += n
            if self.total:
                pct = int(self.n / self.total * 100)
                mb = self.n / 1_048_576
                total_mb = self.total / 1_048_576
                progress_cb(f'{pct}%  ({mb:.1f} / {total_mb:.1f} MB)')
            else:
                progress_cb(f'{self.n / 1_048_576:.1f} MB')

        def __enter__(self): return self
        def __exit__(self, *a): self.close()
        def close(self): pass
        def set_postfix(self, **kw): pass
        def set_description(self, *a, **kw): pass
        def reset(self, total=None): pass
        def __iter__(self):
            if self._iterable:
                yield from self._iterable
    return _Tqdm


def download_model(
    repo_id: str,
    filename: str,
    dest_folder: Path,
    progress_cb: Callable[[str], None],
) -> tuple[bool, list[str]]:
    """Download a model file from HuggingFace into dest_folder.

    Uses hf_hub_download which handles auth, LFS, and CDN redirects correctly.
    For .zip files, the archive is extracted and deleted afterwards.
    Returns (success, list_of_model_stem_names).
    """
    from huggingface_hub import hf_hub_download

    dest_folder.mkdir(parents=True, exist_ok=True)
    dest_path = dest_folder / Path(filename).name

    token = get_hf_token() or None  # None lets hf_hub use its own token management

    progress_cb(f'Connecting to {repo_id}...')

    try:
        downloaded = hf_hub_download(
            repo_id=repo_id,
            filename=filename,
            local_dir=dest_folder,
            token=token,
            tqdm_class=_make_tqdm_class(progress_cb),
        )

        local_path = Path(downloaded)
        if local_path.resolve() != dest_path.resolve() and local_path.exists():
            local_path.rename(dest_path)

        if dest_path.suffix == '.zip':
            names = _extract_pth_from_zip(dest_path, dest_folder, progress_cb)
            dest_path.unlink(missing_ok=True)
            if not names:
                return False, []
            progress_cb(f'Extracted {len(names)} model(s).')
            return True, names

        progress_cb(f'Saved: {dest_path.name}')
        return True, [dest_path.stem]

    except Exception as e:
        dest_path.unlink(missing_ok=True)
        err = str(e)
        logger.error('download_model %s/%s: %s', repo_id, filename, e)
        if 'gated' in err.lower():
            if token:
                progress_cb('Access denied — model is gated. Accept the terms on huggingface.co first.')
            else:
                progress_cb('Access denied — model is gated. Accept terms on huggingface.co and set a token.')
        elif '401' in err or '403' in err or 'unauthorized' in err.lower():
            if token:
                progress_cb('Access denied — token may be expired or lacks access to this repo.')
            else:
                progress_cb('Access denied — set a token in the Authentication field, or run: huggingface-cli login')
        else:
            progress_cb(f'Error: {e}')
        return False, []


def delete_model(name: str, models_folder: Path) -> bool:
    """Delete a local RVC model file by stem name."""
    path = models_folder / f'{name}.pth'
    if path.exists():
        path.unlink()
        logger.info('Deleted RVC model: %s', path)
        return True
    logger.warning('Delete: model not found: %s', path)
    return False
