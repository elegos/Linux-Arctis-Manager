#!/usr/bin/env python3
"""Single definition of "what a release ships", shared by release.yaml's jobs.

  release_assets.py list <dir>   NUL-separated paths of the files to attach
  release_assets.py sums <dir>   `sha256sum`-style lines (basename) for them

Both modes apply the same filter, so the signed SHA256SUMS covers exactly the
assets the release carries (tarballs, distro packages, install.sh) and nothing
else. SHA256SUMS / SHA256SUMS.sig are never listed in `sums` (a checksum file
can't contain itself), but they are in `list` since they are shipped.

Dropped: debug symbol packages and source packages. Those aren't something end
users install, and source packages are redundant with the tag's own source
archive. (rpm debuginfo/debugsource are already dropped in build-pkg.yaml; the
patterns are repeated here as a safety net.)
"""

import fnmatch
import hashlib
import sys
from pathlib import Path

EXCLUDED = (
    "*-debug-*.pkg.tar.zst",
    "*dbgsym*.deb",
    "*debuginfo*",
    "*debugsource*",
    "*.src.rpm",
    "*.src.tar.*",
    "*.dsc",
    "*.orig.tar.*",
    "*.debian.tar.*",
    "*_source.changes",
)
CHECKSUM_FILES = ("SHA256SUMS", "SHA256SUMS.sig")


def shipped(root: Path) -> list[Path]:
    return sorted(
        p
        for p in root.rglob("*")
        if p.is_file() and not any(fnmatch.fnmatch(p.name.lower(), pat) for pat in EXCLUDED)
    )


def sums(root: Path) -> list[str]:
    files = [p for p in shipped(root) if p.name not in CHECKSUM_FILES]
    names = [p.name for p in files]
    dupes = sorted({n for n in names if names.count(n) > 1})
    if dupes:
        # Release assets are addressed by filename, so two would collide.
        print(f"::error::duplicate release asset name(s): {', '.join(dupes)}", file=sys.stderr)
        raise SystemExit(1)
    return [f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}" for p in sorted(files, key=lambda p: p.name)]


def main(argv: list[str]) -> int:
    if len(argv) != 3 or argv[1] not in ("list", "sums"):
        print(__doc__, file=sys.stderr)
        return 2
    root = Path(argv[2])
    if argv[1] == "list":
        sys.stdout.write("".join(f"{p}\0" for p in shipped(root)))
    else:
        print("\n".join(sums(root)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
