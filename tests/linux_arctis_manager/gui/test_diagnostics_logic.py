from urllib.parse import parse_qs, urlparse

from linux_arctis_manager.gui.diagnostics_logic import (
    ISSUE_URL_MAX_LENGTH,
    describe_device,
    issue_url,
    parse_report,
    unsupported_devices,
)

DEVICE = {
    'node': 'hidraw3', 'vendor_id': '1038', 'product_id': '2202',
    'manufacturer': 'SteelSeries', 'product': 'Arctis Nova X', 'interface': 5,
    'usb_audio': True, 'audio_accessory': False,
    'status': 'unsupported', 'config_name': None, 'report_descriptor': '06c0ff',
}


def test_parse_report_returns_devices():
    assert parse_report('{"version": 1, "devices": [{"node": "hidraw1"}]}') == [{'node': 'hidraw1'}]


def test_parse_report_tolerates_garbage():
    assert parse_report('') == []
    assert parse_report('not json') == []
    assert parse_report('{}') == []
    assert parse_report('[1, 2]') == []


def test_unsupported_devices_filters_by_status():
    devices = [DEVICE, {**DEVICE, 'status': 'supported'}, {**DEVICE, 'status': 'ignored'}]
    assert unsupported_devices(devices) == [DEVICE]


def test_describe_device_mentions_ids_and_status():
    text = describe_device(DEVICE)
    assert '1038:2202' in text
    assert 'Arctis Nova X' in text
    assert 'hidraw3' in text
    assert 'interface 5' in text
    assert 'unsupported' in text


def test_describe_device_handles_missing_strings():
    text = describe_device({**DEVICE, 'product': None, 'manufacturer': None, 'interface': None})
    assert '1038:2202' in text


def _query(url: str) -> dict[str, str]:
    return {k: v[0] for k, v in parse_qs(urlparse(url).query).items()}


def test_issue_url_targets_new_device_form_and_prefills_it():
    url = issue_url(DEVICE, 'Linux Arctis Manager 3.0.0')
    assert url.startswith('https://github.com/elegos/Linux-Arctis-Manager/issues/new?')
    q = _query(url)
    assert q['template'] == 'new-device.yml'
    assert q['model'] == 'Arctis Nova X'
    assert '1038:2202' in q['title']
    assert '1038:2202' in q['hardware']
    assert '06c0ff' in q['hardware']
    assert 'Linux Arctis Manager 3.0.0' in q['hardware']


def test_issue_url_drops_the_descriptor_when_too_long():
    big = {**DEVICE, 'report_descriptor': 'ab' * 10000}
    url = issue_url(big, 'v')
    assert len(url) <= ISSUE_URL_MAX_LENGTH
    q = _query(url)
    assert '1038:2202' in q['hardware']
    assert 'abab' not in q['hardware']


def test_issue_url_without_product_name_leaves_model_empty():
    q = _query(issue_url({**DEVICE, 'product': None}, 'v'))
    assert 'model' not in q or q['model'] == ''


def test_describe_device_takes_translated_labels_and_can_skip_the_status():
    text = describe_device(DEVICE, node_label='nodo', interface_label='interfaccia', include_status=False)
    assert 'nodo hidraw3' in text and 'interfaccia 5' in text
    assert 'node' not in text and 'status' not in text
