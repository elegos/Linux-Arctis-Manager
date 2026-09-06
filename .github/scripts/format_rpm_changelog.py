#!/usr/bin/env python3
"""Format a GitHub Release draft body as an rpm %changelog entry.

Used by release.yaml's publish-stable/publish-unstable jobs: the maintainer's
edited GitHub Release draft body is the authoritative changelog text (see
docs/release-pipeline.md, "Changelog handling"). rpm's %changelog has no
concept of headings, only a flat bulleted list per entry - changelog_body.py
does the actual heading-stripping/flattening, shared with every other
platform's formatter.

Output is a single entry block (no surrounding blank lines), meant to be
prepended right after the "%changelog" line - rpm changelog entries are
newest-first, same convention already used by the existing hand-written
entries in packaging/fedora/linux-arctis-manager.spec.
"""

import argparse
import sys
from datetime import date

from changelog_body import parse_changelog_body


def format_entry(maintainer: str, version: str, release: str, entry_date: date, body: str) -> str:
    items = parse_changelog_body(body)
    if not items:
        print("::error::no changelog list items found in release body", file=sys.stderr)
        raise SystemExit(1)

    header = f"* {entry_date.strftime('%a %b %d %Y')} {maintainer} - {version}-{release}"
    lines = [f"- {item}" for item in items]
    return "\n".join([header, *lines])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--maintainer", required=True, help="'Name <email>', e.g. from git log's %%an/%%ae")
    parser.add_argument("--version", required=True, help="rpm Version: (VERSION file, '-' already replaced with '~')")
    parser.add_argument("--release", required=True, help="rpm Release: numeric part, no %%{?dist}")
    parser.add_argument("--date", help="override entry date (YYYY-MM-DD); defaults to today")
    args = parser.parse_args()

    entry_date = date.fromisoformat(args.date) if args.date else date.today()
    body = sys.stdin.read()

    print(format_entry(args.maintainer, args.version, args.release, entry_date, body))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
