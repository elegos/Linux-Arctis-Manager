"""Parse a Keep a Changelog style body into a flat list of change strings.

Shared by every per-platform changelog formatter (format_rpm_changelog.py
today; a future Debian stanza formatter per docs/release-pipeline.md,
"Changelog handling") - each formatter owns only its own output syntax, this
module owns turning the maintainer-edited GitHub Release draft body (heading
levels vary across this project's own history: some old CHANGELOG.md
sections use "## Added", current ones use "### Added") into the flat,
heading-free bullet list every packaging changelog format actually wants.

Handles, without silently dropping content:
- any ATX heading level ("#" through "######")
- "-", "*", or "+" as the bullet marker (GitHub's editor doesn't enforce
  Keep a Changelog's own "-" convention if a maintainer hand-edits the draft)
- a bullet's text wrapped onto following physical lines with no marker of
  its own - joined back into the one logical item
- nested sub-bullets - flattened into the same list, not merged into their
  parent (kept as their own item, since dropping the nesting loses no text)
"""

import re

_HEADING_RE = re.compile(r"^#{1,6}\s")
_BULLET_RE = re.compile(r"^[-*+]\s+(.*)$")


def parse_changelog_body(body: str) -> list[str]:
    items: list[str] = []

    for raw_line in body.splitlines():
        line = raw_line.strip()

        if not line or _HEADING_RE.match(line):
            continue

        bullet = _BULLET_RE.match(line)
        if bullet:
            items.append(bullet.group(1).strip())
        elif items:
            items[-1] = f"{items[-1]} {line}"
        else:
            # Stray non-heading text before any bullet - keep it rather than
            # silently dropping real changelog content.
            items.append(line)

    return items
