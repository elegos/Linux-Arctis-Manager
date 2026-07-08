from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path

logger = logging.getLogger('SteamLibrary')

_STEAM_ROOT_CANDIDATES = [
    Path.home() / '.local' / 'share' / 'Steam',
    Path.home() / '.steam' / 'steam',
    Path.home() / '.steam' / 'root',
]


@dataclass
class SteamGame:
    app_id: int
    name: str
    install_dir: str
    library_path: Path


def _find_steam_root() -> Path | None:
    for p in _STEAM_ROOT_CANDIDATES:
        if p.is_dir():
            return p
    return None


def _find_library_paths(steam_root: Path) -> list[Path]:
    try:
        import vdf as vdflib
    except ImportError:
        logger.warning('vdf package not available; Steam library detection disabled')
        return [steam_root / 'steamapps']

    vdf_path = steam_root / 'steamapps' / 'libraryfolders.vdf'
    paths = [steam_root / 'steamapps']
    if not vdf_path.exists():
        return paths
    try:
        with open(vdf_path) as f:
            data = vdflib.load(f)
        folders = data.get('libraryfolders', data.get('LibraryFolders', {}))
        for key, value in folders.items():
            if not key.isdigit():
                continue
            folder_path = value if isinstance(value, str) else value.get('path', '')
            if folder_path:
                lib = Path(folder_path) / 'steamapps'
                if lib.is_dir():
                    paths.append(lib)
    except Exception as e:
        logger.warning(f'Error reading Steam library folders: {e}')
    return paths


def list_installed_games() -> list[SteamGame]:
    """Return all installed Steam games, sorted by name."""
    steam_root = _find_steam_root()
    if steam_root is None:
        return []
    games: list[SteamGame] = []
    for lib_path in _find_library_paths(steam_root):
        for acf in lib_path.glob('appmanifest_*.acf'):
            try:
                import vdf as vdflib
                with open(acf) as f:
                    data = vdflib.load(f)
                state = data.get('AppState', {})
                app_id = int(state.get('appid', 0))
                name = state.get('name', '')
                install_dir = state.get('installdir', '')
                if app_id and name:
                    games.append(SteamGame(app_id=app_id, name=name, install_dir=install_dir, library_path=lib_path))
            except Exception as e:
                logger.debug(f'Error reading {acf}: {e}')
    return sorted(games, key=lambda g: g.name.lower())


def get_game_executables(app_id: int) -> list[str]:
    """Return candidate executable names for a Steam app_id (best-effort)."""
    steam_root = _find_steam_root()
    if steam_root is None:
        return []
    for lib_path in _find_library_paths(steam_root):
        acf = lib_path / f'appmanifest_{app_id}.acf'
        if not acf.exists():
            continue
        try:
            import vdf as vdflib
            with open(acf) as f:
                data = vdflib.load(f)
            install_dir = data.get('AppState', {}).get('installdir', '')
            game_path = lib_path / 'common' / install_dir
            if not game_path.is_dir():
                continue
            names: list[str] = []
            for p in game_path.iterdir():
                if p.is_file():
                    if p.suffix in ('.exe', '.sh', '') or p.suffix == '.x86_64':
                        names.append(p.name)
            return names
        except Exception:
            pass
    return []
