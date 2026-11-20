"""Look for newer releases of the application on GitHub."""

import logging
from dataclasses import dataclass
from typing import Any

import requests
from packaging.version import InvalidVersion, Version

RELEASES_API_URL = 'https://api.github.com/repos/elegos/Linux-Arctis-Manager/releases'
RELEASES_PAGE_URL = 'https://github.com/elegos/Linux-Arctis-Manager/releases'

_logger = logging.getLogger('UpdateCheck')


@dataclass(frozen=True)
class UpdateInfo:
    stable: str | None       # latest published stable release
    prerelease: str | None   # latest published pre-release
    update_available: bool
    target: str | None       # the version to move to, when an update is available


def fetch_releases(timeout: float = 10.0) -> list[dict[str, Any]] | None:
    """Return the GitHub releases, or None when they can't be fetched."""
    try:
        response = requests.get(
            RELEASES_API_URL,
            params={'per_page': 30},
            headers={'Accept': 'application/vnd.github+json'},
            timeout=timeout,
        )
        response.raise_for_status()
        data = response.json()
        return data if isinstance(data, list) else None
    except (requests.RequestException, ValueError) as e:
        _logger.info('Update check failed: %s', e)
        return None


def _parse(tag: str) -> Version | None:
    try:
        return Version(tag.removeprefix('v'))
    except InvalidVersion:
        return None


def _latest(tags: list[str]) -> str | None:
    parsed = [(v, t) for t in tags if (v := _parse(t)) is not None]
    return max(parsed, key=lambda p: p[0])[1].removeprefix('v') if parsed else None


def parse_releases(releases: list[dict[str, Any]]) -> tuple[str | None, str | None]:
    """Return (latest stable, latest pre-release), ignoring drafts and unparseable tags."""
    published = [r for r in releases if not r.get('draft')]
    stable = _latest([r['tag_name'] for r in published if not r.get('prerelease')])
    pre = _latest([r['tag_name'] for r in published if r.get('prerelease')])
    return stable, pre


def check_for_update(current: str, releases: list[dict[str, Any]]) -> UpdateInfo:
    """Stable builds only look at stable releases; pre-release builds look at both."""
    stable, pre = parse_releases(releases)
    cur = _parse(current)
    if cur is None:
        return UpdateInfo(stable, pre, False, None)

    candidates = [c for c in ([stable, pre] if cur.is_prerelease else [stable]) if c]
    best = max(candidates, key=Version, default=None)
    if best is not None and Version(best) > cur:
        return UpdateInfo(stable, pre, True, best)
    return UpdateInfo(stable, pre, False, None)
