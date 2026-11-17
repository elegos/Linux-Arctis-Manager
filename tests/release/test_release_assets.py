"""release_assets.py decides what a release ships and what SHA256SUMS covers."""

import hashlib
import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / ".github" / "scripts" / "release_assets.py"

SHIPPED = [
    "pkg-arch/linux-arctis-manager-3.0.0beta3-1-x86_64.pkg.tar.zst",
    "pkg-ubuntu24.04/linux-arctis-manager_3.0.0~beta3+ubuntu24.04-1_amd64.deb",
    "pkg-fedora/linux-arctis-manager-3.0.0~beta3-1.fc44.x86_64.rpm",
    "tarball-generic/lam-3.0.0-beta3-x86_64-generic.tar.gz",
    "installer/install.sh",
]
DROPPED = [
    "pkg-arch/linux-arctis-manager-debug-3.0.0beta3-1-x86_64.pkg.tar.zst",
    "pkg-ubuntu24.04/linux-arctis-manager-dbgsym_3.0.0~beta3+ubuntu24.04-1_amd64.deb",
    "pkg-fedora/linux-arctis-manager-debuginfo-3.0.0~beta3-1.fc44.x86_64.rpm",
    "pkg-fedora/linux-arctis-manager-debugsource-3.0.0~beta3-1.fc44.x86_64.rpm",
    "pkg-fedora/linux-arctis-manager-3.0.0~beta3-1.fc44.src.rpm",
]


def _tree(root: Path, names: list[str]) -> None:
    for n in names:
        p = root / n
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(n.encode())


def _run(*args: str) -> str:
    return subprocess.run(
        [sys.executable, "-I", str(SCRIPT), *args], check=True, capture_output=True, text=True
    ).stdout


def test_list_keeps_shipped_assets_and_drops_debug_and_source(tmp_path: Path) -> None:
    _tree(tmp_path, SHIPPED + DROPPED)
    listed = {Path(p).relative_to(tmp_path).as_posix() for p in _run("list", str(tmp_path)).split("\0") if p}
    assert listed == set(SHIPPED)


def test_sums_cover_every_shipped_asset_by_basename(tmp_path: Path) -> None:
    _tree(tmp_path, SHIPPED + DROPPED)
    lines = _run("sums", str(tmp_path)).splitlines()
    got = dict(reversed(line.split("  ", 1)) for line in lines)
    assert set(got) == {Path(n).name for n in SHIPPED}
    for n in SHIPPED:
        assert got[Path(n).name] == hashlib.sha256(n.encode()).hexdigest()


def test_sums_exclude_the_checksum_files_themselves(tmp_path: Path) -> None:
    _tree(tmp_path, [*SHIPPED, "tarball-signed/SHA256SUMS", "tarball-signed/SHA256SUMS.sig"])
    names = {line.split("  ", 1)[1] for line in _run("sums", str(tmp_path)).splitlines()}
    assert "SHA256SUMS" not in names and "SHA256SUMS.sig" not in names


def test_sums_fail_on_duplicate_basenames(tmp_path: Path) -> None:
    _tree(tmp_path, ["a/x.deb", "b/x.deb"])
    r = subprocess.run([sys.executable, "-I", str(SCRIPT), "sums", str(tmp_path)], capture_output=True, text=True, check=False)
    assert r.returncode != 0 and "x.deb" in r.stderr
