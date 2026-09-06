#!/usr/bin/env python3
"""Insert a formatted %changelog entry at the top of an rpm spec's %changelog.

rpm changelog entries are newest-first, so the entry goes right after the
"%changelog" line, not appended after the existing ones - same convention
already used by the hand-written entries in
packaging/fedora/linux-arctis-manager.spec.
"""

import sys


def main() -> int:
    spec_path, entry_path = sys.argv[1], sys.argv[2]
    with open(entry_path, encoding="utf-8") as f:
        entry = f.read().rstrip("\n")

    marker = "%changelog\n"
    with open(spec_path, encoding="utf-8") as f:
        text = f.read()
    if marker not in text:
        print(f"::error::no '{marker.strip()}' line found in {spec_path}", file=sys.stderr)
        return 1

    text = text.replace(marker, f"{marker}{entry}\n\n", 1)
    with open(spec_path, "w", encoding="utf-8") as f:
        f.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
