#!/usr/bin/env python3
"""Extract the first section's body from CHANGELOG.md (Keep a Changelog format).

Used by release.yaml to seed a tag's draft-release body with whatever's under
the top-most "## [...]" heading — usually [Unreleased], already renamed to
the version if the maintainer bumped it before tagging. The heading itself is
dropped since the release UI already shows the tag/version.
"""

import re
import sys


def main() -> int:
    text = open("CHANGELOG.md", encoding="utf-8").read()
    sections = re.split(r"^## \[.*\].*$", text, flags=re.MULTILINE)
    if len(sections) < 2:
        print("::error::no '## [...]' section found in CHANGELOG.md", file=sys.stderr)
        return 1
    print(sections[1].strip())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
