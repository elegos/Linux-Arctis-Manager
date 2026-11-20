#!/usr/bin/env python3
"""Write one release version into every file that carries it.

    scripts/bump_version.py 3.0.0-beta4        # bump (run before tagging)
    scripts/bump_version.py --update-checksum  # after the tag is pushed
    scripts/bump_version.py 3.0.0-beta3 --no-changelog  # re-sync to a VERSION already bumped

VERSION is the source of truth (the daemon's build.rs and the CI packaging
jobs read it directly). The rest are kept in step so a plain checkout builds
the version it claims to be:

  VERSION, pyproject.toml, uv.lock          3.0.0-beta4 / PEP 440 (3.0.0b4)
  packaging/arch/PKGBUILD                   _upstreamver, pkgrel=1, sha256sums=SKIP
  packaging/fedora/*.spec                   Version: 3.0.0~beta4
  packaging/debian/changelog                top entry 3.0.0~beta4-1
  CHANGELOG.md                              [Unreleased] -> [3.0.0-beta4]

The tag's source archive only exists once the tag does, so the PKGBUILD
checksum is a second step: --update-checksum downloads
archive/refs/tags/v<VERSION>.tar.gz (or reads --archive FILE) and pins its
sha256 in the PKGBUILD.

Everything is computed before anything is written, so a failure changes
nothing. Re-running with the same version is a no-op.
"""

import argparse
import hashlib
import re
import sys
import urllib.request
from pathlib import Path

VERSION_RE = re.compile(r"^\d+\.\d+\.\d+(?:-(alpha|beta|rc)\d+)?$")
ARCHIVE_URL = "https://github.com/elegos/Linux-Arctis-Manager/archive/refs/tags/v{version}.tar.gz"
PEP440_PRE = {"alpha": "a", "beta": "b", "rc": "rc"}


class BumpError(Exception):
    pass


def pep440(version: str) -> str:
    return re.sub(r"-(alpha|beta|rc)(\d+)$", lambda m: PEP440_PRE[m[1]] + m[2], version)


def sub_once(text: str, pattern: str, repl: str, what: str, flags: int = re.MULTILINE) -> str:
    new, n = re.subn(pattern, lambda _m: repl, text, count=1, flags=flags)
    if n != 1:
        raise BumpError(f"{what}: pattern not found")
    return new


def bump_changelog(text: str, version: str) -> str:
    if re.search(r"^## \[Unreleased\]", text, re.MULTILINE):
        return sub_once(text, r"^## \[Unreleased\].*$", f"## [{version}]", "CHANGELOG.md")
    if re.search(rf"^## \[{re.escape(version)}\]", text, re.MULTILINE):
        return text  # already bumped
    raise BumpError(f"CHANGELOG.md: no '## [Unreleased]' section to turn into [{version}]")


def planned_edits(root: Path, version: str, changelog: bool = True) -> dict[Path, str]:
    tilde = version.replace("-", "~")

    def read(rel: str) -> str:
        return (root / rel).read_text(encoding="utf-8")

    edits = {
        "VERSION": f"{version}\n",
        "pyproject.toml": sub_once(read("pyproject.toml"), r'^version = ".*"', f'version = "{version}"', "pyproject.toml"),
        "uv.lock": sub_once(
            read("uv.lock"),
            r'(name = "linux-arctis-manager"\nversion = )".*"',
            f'name = "linux-arctis-manager"\nversion = "{pep440(version)}"',
            "uv.lock",
        ),
        "packaging/fedora/linux-arctis-manager.spec": sub_once(
            read("packaging/fedora/linux-arctis-manager.spec"), r"^Version:.*$", f"Version:        {tilde}", "spec"
        ),
        "packaging/debian/changelog": sub_once(
            read("packaging/debian/changelog"),
            r"^(linux-arctis-manager) \(.*?\)",
            f"linux-arctis-manager ({tilde}-1)",
            "debian/changelog",
            flags=0,
        ),
    }
    if changelog:
        edits["CHANGELOG.md"] = bump_changelog(read("CHANGELOG.md"), version)

    pkgbuild = read("packaging/arch/PKGBUILD")
    new_pkgbuild = sub_once(pkgbuild, r"^_upstreamver=.*$", f"_upstreamver={version}", "PKGBUILD _upstreamver")
    if new_pkgbuild != pkgbuild:
        # A new version: the old checksum is for the old archive.
        new_pkgbuild = sub_once(new_pkgbuild, r"^pkgrel=.*$", "pkgrel=1", "PKGBUILD pkgrel")
        new_pkgbuild = sub_once(new_pkgbuild, r"^sha256sums=.*$", "sha256sums=('SKIP')", "PKGBUILD sha256sums")
    edits["packaging/arch/PKGBUILD"] = new_pkgbuild

    return {root / rel: text for rel, text in edits.items()}


def update_checksum(root: Path, archive: Path | None) -> None:
    version = (root / "VERSION").read_text(encoding="utf-8").strip()
    if archive is not None:
        data = archive.read_bytes()
    else:
        url = ARCHIVE_URL.format(version=version)
        try:
            with urllib.request.urlopen(url, timeout=60) as resp:  # noqa: S310 (fixed https URL)
                data = resp.read()
        except OSError as exc:
            raise BumpError(f"could not fetch {url} (is the tag pushed?): {exc}") from exc
    digest = hashlib.sha256(data).hexdigest()
    path = root / "packaging/arch/PKGBUILD"
    path.write_text(
        sub_once(path.read_text(encoding="utf-8"), r"^sha256sums=.*$", f"sha256sums=('{digest}')", "PKGBUILD sha256sums"),
        encoding="utf-8",
    )
    print(f"PKGBUILD sha256sums for v{version}: {digest}")


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=(__doc__ or "").split("\n\n")[0])
    ap.add_argument("version", nargs="?", help="new version, e.g. 3.0.0-beta4 (no leading v)")
    ap.add_argument("--update-checksum", action="store_true", help="pin the tag archive's sha256 in the PKGBUILD")
    ap.add_argument("--archive", type=Path, help="with --update-checksum: use this local file instead of downloading")
    ap.add_argument("--no-changelog", action="store_true", help="leave CHANGELOG.md alone (re-sync files to an already-bumped VERSION)")
    ap.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1], help=argparse.SUPPRESS)
    args = ap.parse_args(argv)
    try:
        if args.update_checksum:
            update_checksum(args.root, args.archive)
            return 0
        if not args.version or not VERSION_RE.match(args.version):
            raise BumpError(f"version must look like 3.0.0, 3.0.0-beta4, 3.0.0-rc1 (got {args.version!r})")
        for path, text in planned_edits(args.root, args.version, changelog=not args.no_changelog).items():
            if path.read_text(encoding="utf-8") != text:
                path.write_text(text, encoding="utf-8")
                print(f"updated {path.relative_to(args.root)}")
    except BumpError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
