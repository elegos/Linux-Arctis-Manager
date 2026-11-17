"""scripts/bump_version.py writes one version into every file that carries it."""

import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "bump_version.py"

FILES = {
    "VERSION": "3.0.0-beta3\n",
    "pyproject.toml": '[project]\nname = "linux-arctis-manager"\nversion = "3.0.0-beta3"\n',
    "uv.lock": 'version = 1\n\n[[package]]\nname = "other"\nversion = "1.0.0"\n\n'
    '[[package]]\nname = "linux-arctis-manager"\nversion = "3.0.0b3"\nsource = { editable = "." }\n',
    "packaging/arch/PKGBUILD": "pkgbase=linux-arctis-manager\n_upstreamver=3.0.0-beta3\n"
    "pkgver=${_upstreamver//-/}\npkgrel=2\nsha256sums=('abc')\n",
    "packaging/fedora/linux-arctis-manager.spec": "Name: x\nVersion:        3.0.0~beta3\nRelease:        1%{?dist}\n",
    "packaging/debian/changelog": "linux-arctis-manager (3.0.0~beta3-1) unstable; urgency=medium\n\n  * x\n",
    "CHANGELOG.md": "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- thing\n\n## [3.0.0-beta3]\n\n- old\n",
}


@pytest.fixture
def root(tmp_path: Path) -> Path:
    for name, text in FILES.items():
        p = tmp_path / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    return tmp_path


def _run(root: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, "-I", str(SCRIPT), "--root", str(root), *args], capture_output=True, text=True, check=False)


def _read(root: Path, name: str) -> str:
    return (root / name).read_text()


def test_bump_writes_every_file(root: Path) -> None:
    assert _run(root, "3.0.0-beta4").returncode == 0
    assert _read(root, "VERSION") == "3.0.0-beta4\n"
    assert 'version = "3.0.0-beta4"' in _read(root, "pyproject.toml")
    lock = _read(root, "uv.lock")
    assert 'name = "linux-arctis-manager"\nversion = "3.0.0b4"' in lock
    assert 'name = "other"\nversion = "1.0.0"' in lock
    pkgbuild = _read(root, "packaging/arch/PKGBUILD")
    assert "_upstreamver=3.0.0-beta4\n" in pkgbuild and "pkgrel=1\n" in pkgbuild
    assert "sha256sums=('SKIP')" in pkgbuild
    assert "Version:        3.0.0~beta4\n" in _read(root, "packaging/fedora/linux-arctis-manager.spec")
    assert _read(root, "packaging/debian/changelog").startswith("linux-arctis-manager (3.0.0~beta4-1) ")
    cl = _read(root, "CHANGELOG.md")
    assert "## [3.0.0-beta4]\n\n### Added" in cl and "## [Unreleased]" not in cl and "## [3.0.0-beta3]" in cl


def test_stable_version_maps_to_each_format(root: Path) -> None:
    assert _run(root, "3.1.0").returncode == 0
    assert 'version = "3.1.0"' in _read(root, "uv.lock")
    assert "pkgver=${_upstreamver//-/}" in _read(root, "packaging/arch/PKGBUILD")
    assert "Version:        3.1.0\n" in _read(root, "packaging/fedora/linux-arctis-manager.spec")


def test_bump_is_idempotent_apart_from_the_changelog(root: Path) -> None:
    _run(root, "3.0.0-beta4")
    snapshot = {n: _read(root, n) for n in FILES}
    r = _run(root, "3.0.0-beta4")
    assert r.returncode == 0
    assert {n: _read(root, n) for n in FILES} == snapshot


@pytest.mark.parametrize("bad", ["v3.0.0", "3.0", "3.0.0-beta", "3.0.0~beta4", "3.0.0b4"])
def test_rejects_malformed_versions_without_touching_anything(root: Path, bad: str) -> None:
    r = _run(root, bad)
    assert r.returncode != 0
    assert {n: _read(root, n) for n in FILES} == FILES


def test_fails_without_an_unreleased_section_and_changes_nothing(root: Path) -> None:
    (root / "CHANGELOG.md").write_text("# Changelog\n\n## [3.0.0-beta3]\n")
    r = _run(root, "3.0.0-beta4")
    assert r.returncode != 0 and "Unreleased" in r.stderr
    assert _read(root, "VERSION") == FILES["VERSION"]


def test_update_checksum_from_a_local_archive(root: Path, tmp_path: Path) -> None:
    import hashlib

    archive = tmp_path / "tag.tar.gz"
    archive.write_bytes(b"source archive")
    r = _run(root, "--update-checksum", "--archive", str(archive))
    assert r.returncode == 0, r.stderr
    digest = hashlib.sha256(b"source archive").hexdigest()
    assert f"sha256sums=('{digest}')" in _read(root, "packaging/arch/PKGBUILD")


def test_no_changelog_flag_leaves_the_changelog_alone(root: Path) -> None:
    assert _run(root, "3.0.0-beta4", "--no-changelog").returncode == 0
    assert _read(root, "CHANGELOG.md") == FILES["CHANGELOG.md"]
    assert _read(root, "VERSION") == "3.0.0-beta4\n"
