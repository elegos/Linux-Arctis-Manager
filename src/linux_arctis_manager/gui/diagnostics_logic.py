"""Pure logic behind the diagnostics page (no Qt): parsing the daemon's
``GetDiagnostics`` report and building the pre-filled GitHub issue link."""

import json
from urllib.parse import urlencode

ISSUE_NEW_URL = 'https://github.com/elegos/Linux-Arctis-Manager/issues/new'
# GitHub rejects very long query strings; stay well below the ~8 KB limit.
ISSUE_URL_MAX_LENGTH = 6000


def parse_report(raw: str) -> list[dict]:
    """The report's device list, or an empty list for anything malformed."""
    try:
        data = json.loads(raw)
    except (TypeError, ValueError):
        return []
    if not isinstance(data, dict) or not isinstance(data.get('devices'), list):
        return []
    return [d for d in data['devices'] if isinstance(d, dict)]


def unsupported_devices(devices: list[dict]) -> list[dict]:
    return [d for d in devices if d.get('status') == 'unsupported']


def _ids(device: dict) -> str:
    return f"{device.get('vendor_id', '????')}:{device.get('product_id', '????')}"


def describe_device(device: dict) -> str:
    """One-paragraph plain-text description, suitable for the clipboard."""
    name = ' '.join(p for p in (device.get('manufacturer'), device.get('product')) if p)
    parts = [f'{_ids(device)}' + (f' {name}' if name else '')]
    parts.append(f"node {device.get('node', '?')}")
    if device.get('interface') is not None:
        parts.append(f"interface {device['interface']}")
    parts.append(f"status: {device.get('status', 'unknown')}")
    return ', '.join(parts)


def _hardware_text(device: dict, app_version: str, with_descriptor: bool) -> str:
    lines = [describe_device(device), f'App: {app_version}']
    if with_descriptor and device.get('report_descriptor'):
        lines.append(f"HID report descriptor: {device['report_descriptor']}")
    return '\n'.join(lines)


def issue_url(device: dict, app_version: str) -> str:
    """Link to the new-device issue form, pre-filled from ``device``.

    The HID descriptor is dropped when the link would get too long; the debug
    bundle carries it anyway."""
    def build(with_descriptor: bool) -> str:
        query = {
            'template': 'new-device.yml',
            'title': f'[device] {_ids(device)} {device.get("product") or ""}'.rstrip(),
            'model': device.get('product') or '',
            'hardware': _hardware_text(device, app_version, with_descriptor),
        }
        return f'{ISSUE_NEW_URL}?{urlencode(query)}'

    url = build(True)
    return url if len(url) <= ISSUE_URL_MAX_LENGTH else build(False)
