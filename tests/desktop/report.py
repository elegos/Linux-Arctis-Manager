"""Builds the HTML pages of the desktop tests (standard library only): the
gallery of one run, and the comparison of the latest run of every desktop."""

import sys
from html import escape
from pathlib import Path
from urllib.parse import quote

_PAGE = """<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>{title}</title>
<style>
  body {{ font-family: sans-serif; margin: 2rem; background: #808080; color: #000; }}
  figure {{ margin: 0 0 2rem; }}
  img {{ max-width: 100%; border: 1px solid #000; }}
  table {{ border-collapse: collapse; }}
  th, td {{ border: 1px solid #000; padding: .5rem; vertical-align: top; }}
  .failed {{ color: #fff; background: #a00; padding: .5rem; }}
</style>
<h1>{title}</h1>
{body}
</html>
"""


def build_report(out_dir: Path, desktop: str, failed: bool = False) -> Path:
    parts = []
    if failed:
        parts.append('<p class="failed">Run FAILED: see the logs below.</p>')

    shots = sorted(out_dir.glob('*.png'))
    if not shots:
        parts.append('<p>No screenshots were produced.</p>')
    parts.extend(
        f'<figure><figcaption>{escape(shot.stem)}</figcaption>'
        f'<a href="{quote(shot.name)}"><img src="{quote(shot.name)}" alt="{escape(shot.stem)}"></a></figure>'
        for shot in shots
    )

    logs = sorted(out_dir.glob('*.log'))
    if logs:
        items = ''.join(f'<li><a href="{quote(log.name)}">{escape(log.name)}</a></li>' for log in logs)
        parts.append(f'<h2>Logs</h2><ul>{items}</ul>')

    report = out_dir / 'index.html'
    report.write_text(_PAGE.format(title=escape(f'Desktop test: {desktop}'), body='\n'.join(parts)))
    return report


def build_comparison(root: Path) -> Path:
    """Screenshots as rows, desktops as columns. `root` holds <desktop>/<run>/."""
    runs = {}
    for desktop in sorted(p for p in root.iterdir() if p.is_dir()):
        stamps = sorted(p for p in desktop.iterdir() if p.is_dir())
        if stamps:
            runs[desktop.name] = stamps[-1]

    names = sorted({shot.name for run in runs.values() for shot in run.glob('*.png')})
    head = ''.join(
        f'<th>{escape(desktop)}<br><a href="{quote(f"{desktop}/{run.name}/index.html")}">{escape(run.name)}</a></th>'
        for desktop, run in runs.items()
    )
    rows = []
    for name in names:
        cells = []
        for desktop, run in runs.items():
            if (run / name).exists():
                url = quote(f'{desktop}/{run.name}/{name}')
                cells.append(f'<td><a href="{url}"><img src="{url}" alt="{escape(name)}"></a></td>')
            else:
                cells.append('<td>missing</td>')
        rows.append(f'<tr><th>{escape(Path(name).stem)}</th>{"".join(cells)}</tr>')

    report = root / 'index.html'
    body = f'<table><tr><th></th>{head}</tr>{"".join(rows)}</table>'
    report.write_text(_PAGE.format(title='Desktop comparison', body=body))
    return report


if __name__ == '__main__':
    run_dir = Path(sys.argv[1])
    build_report(run_dir, sys.argv[2], failed=sys.argv[3] != '0')
    print(build_comparison(run_dir.parent.parent))
