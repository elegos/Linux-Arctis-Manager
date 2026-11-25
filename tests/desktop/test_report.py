from pathlib import Path

from tests.desktop.report import build_comparison, build_report


def test_report_lists_screenshots_in_name_order(tmp_path: Path):
    (tmp_path / '02-dark--panel.png').write_bytes(b'')
    (tmp_path / '01-light--desktop.png').write_bytes(b'')

    html = build_report(tmp_path, 'cinnamon').read_text()

    assert html.index('01-light--desktop.png') < html.index('02-dark--panel.png')
    assert 'cinnamon' in html


def test_report_escapes_names(tmp_path: Path):
    (tmp_path / 'a<b>.png').write_bytes(b'')

    html = build_report(tmp_path, 'cinnamon').read_text()

    assert 'a<b>' not in html
    assert 'a%3Cb%3E.png' in html


def test_report_states_when_there_are_no_screenshots(tmp_path: Path):
    html = build_report(tmp_path, 'cinnamon', failed=True).read_text()

    assert 'No screenshots' in html
    assert 'FAILED' in html


def test_report_links_logs(tmp_path: Path):
    (tmp_path / 'session.log').write_text('x')

    assert 'href="session.log"' in build_report(tmp_path, 'cinnamon').read_text()


def _run(root: Path, desktop: str, stamp: str, *shots: str) -> Path:
    out = root / desktop / stamp
    out.mkdir(parents=True)
    for shot in shots:
        (out / f'{shot}.png').write_bytes(b'')
    return out


def test_comparison_puts_the_latest_run_of_each_desktop_side_by_side(tmp_path: Path):
    _run(tmp_path, 'cinnamon', '20260101-000000', 'tray--dark--panel')
    _run(tmp_path, 'cinnamon', '20260102-000000', 'tray--dark--panel')
    _run(tmp_path, 'kde', '20260101-000000', 'tray--dark--panel')

    html = build_comparison(tmp_path).read_text()

    assert html.index('<th>cinnamon') < html.index('<th>kde')
    assert 'cinnamon/20260102-000000/tray--dark--panel.png' in html
    assert 'cinnamon/20260101-000000' not in html
    assert 'kde/20260101-000000/tray--dark--panel.png' in html
    assert html.count('tray--dark--panel</th>') == 1


def test_comparison_marks_missing_screenshots(tmp_path: Path):
    _run(tmp_path, 'cinnamon', '20260101-000000', 'tray--dark--panel')
    _run(tmp_path, 'kde', '20260101-000000', 'gui--dark--window')

    assert build_comparison(tmp_path).read_text().count('<td>missing</td>') == 2
