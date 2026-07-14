import logging
import subprocess
import sys
from pathlib import Path
from typing import Callable

logger = logging.getLogger('ai_deps')

AI_ENV_PATH = Path.home() / '.local' / 'share' / 'arctis_manager' / 'ai_env'


def _ai_env_pip() -> Path:
    return AI_ENV_PATH / 'bin' / 'pip'


def _ai_env_site_packages() -> Path:
    v = sys.version_info
    return AI_ENV_PATH / 'lib' / f'python{v.major}.{v.minor}' / 'site-packages'


def ai_env_exists() -> bool:
    """Return True if an AI env has been created (even if incomplete)."""
    return (AI_ENV_PATH / 'bin' / 'python').exists()


def activate_ai_env() -> bool:
    """Insert AI env site-packages into sys.path if the env exists. Returns True if activated."""
    site = _ai_env_site_packages()
    if site.exists():
        site_str = str(site)
        if site_str not in sys.path:
            sys.path.insert(0, site_str)
        return True
    return False


def detect_gpu() -> dict:
    """Probe for GPU hardware. Returns {'type': 'nvidia'|'amd'|'intel'|None, 'name': str}."""
    # NVIDIA
    try:
        r = subprocess.run(
            ['nvidia-smi', '--query-gpu=name', '--format=csv,noheader'],
            capture_output=True, text=True, timeout=5,
        )
        if r.returncode == 0 and r.stdout.strip():
            return {'type': 'nvidia', 'name': r.stdout.strip().split('\n')[0].strip()}
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # AMD (ROCm)
    try:
        if Path('/dev/kfd').exists():
            r = subprocess.run(
                ['rocm-smi', '--showproductname'],
                capture_output=True, text=True, timeout=5,
            )
            name = 'AMD GPU (ROCm)'
            if r.returncode == 0:
                for line in r.stdout.splitlines():
                    if ':' in line and 'GPU' in line:
                        candidate = line.split(':', 1)[1].strip()
                        if candidate:
                            name = candidate
                            break
            return {'type': 'amd', 'name': name}
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Intel
    try:
        r = subprocess.run(['lspci'], capture_output=True, text=True, timeout=5)
        if r.returncode == 0:
            for line in r.stdout.splitlines():
                lower = line.lower()
                if 'intel' in lower and any(k in lower for k in ('vga', '3d', 'display')):
                    return {'type': 'intel', 'name': line.split(':', 2)[-1].strip()}
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    return {'type': None, 'name': ''}


def install_ai_deps(backend: str, progress_cb: Callable[[str], None]) -> bool:
    """Create AI venv using this process's Python and install the appropriate packages.

    backend: 'auto' | 'nvidia' | 'amd' | 'intel' | 'cpu'
    progress_cb: called with each status line during installation.
    Returns True on success.
    """
    if backend == 'auto':
        info = detect_gpu()
        backend = info.get('type') or 'cpu'

    # Use daemon's sys.executable to avoid Python ABI mismatch between venv and runtime.
    progress_cb('Creating AI environment...')
    try:
        r = subprocess.run(
            [sys.executable, '-m', 'venv', str(AI_ENV_PATH)],
            capture_output=True, text=True, timeout=120,
        )
        if r.returncode != 0:
            progress_cb(f'Error creating venv: {r.stderr.strip()}')
            return False
        progress_cb('Environment created.')
    except Exception as e:
        progress_cb(f'Error: {e}')
        return False

    pip = _ai_env_pip()
    try:
        subprocess.run(
            [str(pip), 'install', '--upgrade', 'pip'],
            capture_output=True, timeout=120,
        )
    except Exception:
        pass

    if backend == 'nvidia':
        packages = ['numpy', 'torch', 'torchaudio']
        extra = ['--extra-index-url', 'https://download.pytorch.org/whl/cu124']
        progress_cb('Installing PyTorch (CUDA)...')
    elif backend == 'amd':
        packages = ['numpy', 'torch', 'torchaudio']
        extra = ['--extra-index-url', 'https://download.pytorch.org/whl/rocm6.2']
        progress_cb('Installing PyTorch (ROCm)...')
    elif backend == 'intel':
        packages = ['numpy', 'openvino']
        extra = []
        progress_cb('Installing OpenVINO...')
    else:
        packages = ['numpy', 'torch', 'torchaudio']
        extra = []
        progress_cb('Installing PyTorch (CPU)...')

    try:
        proc = subprocess.Popen(
            [str(pip), 'install'] + packages + extra,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        assert proc.stdout is not None
        for line in proc.stdout:
            stripped = line.rstrip()
            if stripped:
                progress_cb(stripped)
        proc.wait()
        if proc.returncode != 0:
            progress_cb(f'pip install failed (exit {proc.returncode})')
            return False
    except Exception as e:
        progress_cb(f'Install error: {e}')
        return False

    progress_cb('Done.')
    return True
