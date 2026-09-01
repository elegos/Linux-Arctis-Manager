#!/usr/bin/env python3
"""Resolve the install-test CI matrix for wheel-install-test.yaml.

The *set* of distros tested is a static snapshot of DistroWatch's page-hit
top 10 (see the comment block below) intersected with "has an image we can
actually trust in CI" — some of the top 10 have no official/maintained
container image at all and are skipped, noted here rather than silently.

The *version* of each versioned distro (Fedora, Ubuntu, Linux Mint) is
resolved dynamically against endoflife.date at run time, the same idea as
an install script checking "what's currently supported" instead of a
maintainer hand-updating version numbers every release. Debian ships its
own floating stable/oldstable tags, so no lookup is needed there. Rolling
distros (Arch, CachyOS, Bazzite) have no "version" to resolve at all.
"""

import datetime
import json
import re
import sys
import urllib.request

# --- DistroWatch top 10 (page-hit ranking, last 30 days, snapshot 2026-09-01) ---
# CachyOS, Mint, MX Linux, Pop!_OS, Debian, Fedora, Zorin, Ubuntu, EndeavourOS,
# Bazzite.
#
# Skipped (no official/reliably-maintained container image to test against):
#   - MX Linux: no container image, official or otherwise.
#   - Pop!_OS: only unofficial, low-maintenance community images.
#   - Zorin: no dedicated OS image (only unrelated same-named Docker Hub
#     profiles / unrelated "webtop" desktop images).
#   - EndeavourOS: only one low-adoption community image, and it's plain
#     Arch/pacman underneath — already covered by the explicit Arch Linux
#     entry below, so it wouldn't add real package-manager coverage.
#
# Arch Linux itself isn't in the top 10, but is added explicitly (CachyOS's
# repos/kernel are custom enough that a plain-Arch job is worth having too).

ENDOFLIFE_TIMEOUT = 15


def _fetch_json(url: str):
    with urllib.request.urlopen(url, timeout=ENDOFLIFE_TIMEOUT) as resp:
        return json.load(resp)


def _today() -> datetime.date:
    return datetime.datetime.now(datetime.timezone.utc).date()


def _parse_date(value) -> datetime.date | None:
    if not value or not isinstance(value, str):
        return None
    return datetime.date.fromisoformat(value)


def _released_non_eol(cycles: list[dict]) -> list[dict]:
    today = _today()
    out = []
    for c in cycles:
        released = _parse_date(c.get("releaseDate"))
        if released is None or released > today:
            continue
        eol = c.get("eol")
        eol_date = _parse_date(eol) if isinstance(eol, str) else None
        if eol_date is not None and eol_date <= today:
            continue
        out.append(c)
    return out


def resolve_fedora() -> list[dict]:
    cycles = _released_non_eol(_fetch_json("https://endoflife.date/api/fedora.json"))
    cycles.sort(key=lambda c: int(c["cycle"]), reverse=True)
    return [
        {"distro": f"Fedora {c['cycle']}", "image": f"fedora:{c['cycle']}"}
        for c in cycles[:2]
    ]


def resolve_ubuntu() -> list[dict]:
    cycles = _released_non_eol(_fetch_json("https://endoflife.date/api/ubuntu.json"))
    cycles.sort(key=lambda c: _parse_date(c["releaseDate"]), reverse=True)

    latest_overall = cycles[0] if cycles else None
    latest_lts = next((c for c in cycles if c.get("lts")), None)

    picked, seen = [], set()
    for c in (latest_lts, latest_overall):
        if c is not None and c["cycle"] not in seen:
            seen.add(c["cycle"])
            picked.append(c)

    return [
        {"distro": f"Ubuntu {c['cycle']}", "image": f"ubuntu:{c['cycle']}"}
        for c in picked
    ]


def resolve_linuxmint() -> list[dict]:
    cycles = _released_non_eol(_fetch_json("https://endoflife.date/api/linuxmint.json"))
    # Exclude LMDE entries (cycle names like "lmde7") — linuxmintd's Docker
    # images follow the main-line "mintNN.N-amd64" naming only.
    cycles = [c for c in cycles if re.fullmatch(r"\d+(\.\d+)?", c["cycle"])]
    if not cycles:
        return []
    cycles.sort(key=lambda c: _parse_date(c["releaseDate"]), reverse=True)
    latest = cycles[0]["cycle"]
    return [{
        "distro": f"Linux Mint {latest}",
        "image": f"linuxmintd/mint{latest}-amd64:latest",
    }]


def static_entries() -> list[dict]:
    return [
        # Debian tracks its own current stable/oldstable via floating tags —
        # no version lookup needed on our side.
        {"distro": "Debian (stable)", "image": "debian:stable-slim"},
        {"distro": "Debian (oldstable)", "image": "debian:oldstable-slim"},
        # Rolling / atomic — no versioned releases to resolve.
        {"distro": "Arch Linux", "image": "archlinux:latest"},
        {"distro": "CachyOS", "image": "cachyos/cachyos:latest"},
        {"distro": "Bazzite", "image": "ghcr.io/ublue-os/bazzite:stable"},
    ]


def main() -> int:
    matrix = static_entries()
    for resolver in (resolve_fedora, resolve_ubuntu, resolve_linuxmint):
        try:
            matrix.extend(resolver())
        except Exception as e:  # network hiccup, API shape change, ...
            print(f"::error::{resolver.__name__} failed: {e}", file=sys.stderr)
            return 1

    print(json.dumps(matrix), file=sys.stderr)  # human-readable log
    print(f"matrix={json.dumps(matrix)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
