import logging
from unittest.mock import MagicMock, PropertyMock, patch

import pytest
import usb.core

from linux_arctis_manager.config import DeviceConfiguration
from linux_arctis_manager.core import CoreEngine


# --- Helpers ---

def _make_config(
    product_string: str | None = None,
    product_ids: list[int] | None = None,
    vendor_id: int = 0x1038,
    name: str = 'Test Device',
) -> DeviceConfiguration:
    config = object.__new__(DeviceConfiguration)
    config.name = name
    config.vendor_id = vendor_id
    config.product_ids = product_ids if product_ids is not None else [0x1234]
    config.product_string = product_string
    config.settings = {}
    return config


def _make_engine(configs: list[DeviceConfiguration] | None = None) -> CoreEngine:
    engine = object.__new__(CoreEngine)
    engine.logger = logging.getLogger('test')
    engine.device_configurations = configs or []
    engine.usb_device = None
    engine.device_config = None
    engine.device_status = None
    engine.device_settings = None
    return engine


def _make_usb_device(product: str, pid: int, vid: int = 0x1038) -> MagicMock:
    dev = MagicMock()
    dev.product = product
    dev.idProduct = pid
    dev.idVendor = vid
    return dev


def _mock_configure_side_effects(engine: CoreEngine) -> None:
    """Stub out all configure_virtual_sinks side effects beyond the selection logic."""
    engine.teardown = MagicMock()
    engine.new_device_status = MagicMock(return_value=MagicMock())
    engine.kernel_detach = MagicMock()
    engine.init_device = MagicMock()
    engine.redirect_to_media_sink = MagicMock()
    engine.pa_audio_manager = MagicMock()


# --- _find_usb_device_for_config ---

def test_find_falls_back_to_vid_pid_when_no_product_string():
    engine = _make_engine()
    config = _make_config(product_string=None, product_ids=[0x1234])
    expected = _make_usb_device('Some Device', 0x1234)

    with patch('usb.core.find', return_value=expected) as mock_find:
        result = engine._find_usb_device_for_config(config)

    mock_find.assert_called_once_with(idVendor=0x1038, idProduct=0x1234)
    assert result is expected


def test_find_returns_device_by_product_string_single_match():
    engine = _make_engine()
    config = _make_config(product_string='Arctis Nova Pro Wireless', product_ids=[0x12e0])
    dev = _make_usb_device('Arctis Nova Pro Wireless', 0x12e0)

    with patch('usb.core.find', return_value=[dev]):
        result = engine._find_usb_device_for_config(config)

    assert result is dev


def test_find_returns_none_when_no_product_string_match():
    engine = _make_engine()
    config = _make_config(product_string='Arctis Nova Pro Wireless', product_ids=[0x12e0])
    dev = _make_usb_device('Some Other Device', 0x9999)

    with patch('usb.core.find', return_value=[dev]):
        result = engine._find_usb_device_for_config(config)

    assert result is None


def test_find_tiebreaks_multiple_matches_by_known_pid():
    engine = _make_engine()
    config = _make_config(product_string='Arctis Nova 7', product_ids=[0x2222])
    dev_unknown = _make_usb_device('Arctis Nova 7', 0x1111)
    dev_known = _make_usb_device('Arctis Nova 7', 0x2222)

    with patch('usb.core.find', return_value=[dev_unknown, dev_known]):
        result = engine._find_usb_device_for_config(config)

    assert result is dev_known


def test_find_multiple_matches_no_known_pid_returns_first():
    engine = _make_engine()
    config = _make_config(product_string='Arctis Nova 7', product_ids=[0x9999])
    dev1 = _make_usb_device('Arctis Nova 7', 0x1111)
    dev2 = _make_usb_device('Arctis Nova 7', 0x2222)

    with patch('usb.core.find', return_value=[dev1, dev2]):
        result = engine._find_usb_device_for_config(config)

    assert result is dev1


def test_find_usberror_on_scan_returns_none():
    engine = _make_engine()
    config = _make_config(product_string='Arctis Nova Pro Wireless')

    with patch('usb.core.find', side_effect=usb.core.USBError('USB error')):
        result = engine._find_usb_device_for_config(config)

    assert result is None


def test_find_skips_device_when_product_attr_raises():
    engine = _make_engine()
    config = _make_config(product_string='Arctis Nova Pro Wireless', product_ids=[0x12e0])

    bad_dev = MagicMock()
    type(bad_dev).product = PropertyMock(side_effect=ValueError('no descriptor'))
    bad_dev.idProduct = 0x9999

    good_dev = _make_usb_device('Arctis Nova Pro Wireless', 0x12e0)

    with patch('usb.core.find', return_value=[bad_dev, good_dev]):
        result = engine._find_usb_device_for_config(config)

    assert result is good_dev


# --- configure_virtual_sinks ---

def test_configure_prefers_known_pid_over_name_only_match():
    config_a = _make_config(product_string='Arctis Nova 7', product_ids=[0x9999], name='Config A')
    config_b = _make_config(product_string='Arctis Nova 7', product_ids=[0x2222], name='Config B')
    engine = _make_engine(configs=[config_a, config_b])
    _mock_configure_side_effects(engine)

    dev_a = _make_usb_device('Arctis Nova 7', 0x1111)  # PID not in any product_ids
    dev_b = _make_usb_device('Arctis Nova 7', 0x2222)  # PID in config_b.product_ids

    def fake_find(config):
        return dev_a if config is config_a else dev_b

    with patch.object(engine, '_find_usb_device_for_config', side_effect=fake_find), \
         patch('linux_arctis_manager.core.DeviceSettings'):
        engine.configure_virtual_sinks()

    assert engine.device_config is config_b


def test_configure_uses_fallback_when_only_unknown_pid_matched():
    config = _make_config(product_string='Arctis Nova 7', product_ids=[0x9999])
    engine = _make_engine(configs=[config])
    _mock_configure_side_effects(engine)

    dev = _make_usb_device('Arctis Nova 7', 0x1111)  # PID not in product_ids

    with patch.object(engine, '_find_usb_device_for_config', return_value=dev), \
         patch('linux_arctis_manager.core.DeviceSettings'):
        engine.configure_virtual_sinks()

    assert engine.device_config is config


# --- on_device_connected ---

def test_on_connected_known_pid_calls_configure():
    config = _make_config(product_string='Arctis Nova Pro Wireless', product_ids=[0x12e0])
    engine = _make_engine(configs=[config])

    with patch.object(engine, 'configure_virtual_sinks') as mock_configure:
        engine.on_device_connected(0x1038, 0x12e0)

    mock_configure.assert_called_once()


def test_on_connected_unknown_pid_matching_product_string_calls_configure_and_warns():
    config = _make_config(product_string='Arctis Nova Pro Wireless', product_ids=[0x12e0])
    engine = _make_engine(configs=[config])
    dev = _make_usb_device('Arctis Nova Pro Wireless', 0x9999)

    with patch('usb.core.find', return_value=dev), \
         patch.object(engine, 'configure_virtual_sinks') as mock_configure, \
         patch.object(engine.logger, 'warning') as mock_warn:
        engine.on_device_connected(0x1038, 0x9999)

    mock_configure.assert_called_once()
    mock_warn.assert_called_once()
    assert '0x9999' in mock_warn.call_args[0][0]


def test_on_connected_no_match_does_nothing():
    config = _make_config(product_string='Arctis Nova Pro Wireless', product_ids=[0x12e0])
    engine = _make_engine(configs=[config])
    dev = _make_usb_device('Some Other Device', 0x9999)

    with patch('usb.core.find', return_value=dev), \
         patch.object(engine, 'configure_virtual_sinks') as mock_configure:
        engine.on_device_connected(0x1038, 0x9999)

    mock_configure.assert_not_called()


def test_on_connected_wrong_vendor_does_nothing():
    config = _make_config(vendor_id=0x1038, product_ids=[0x12e0])
    engine = _make_engine(configs=[config])

    with patch.object(engine, 'configure_virtual_sinks') as mock_configure:
        engine.on_device_connected(0xAAAA, 0x12e0)

    mock_configure.assert_not_called()


# --- on_device_disconnected ---

def test_on_disconnected_product_string_device_still_present():
    config = _make_config(product_string='Arctis Nova Pro Wireless', product_ids=[0x12e0])
    engine = _make_engine(configs=[config])
    engine.usb_device = _make_usb_device('Arctis Nova Pro Wireless', 0x12e0)
    engine.device_config = config
    still_connected = _make_usb_device('Arctis Nova Pro Wireless', 0x12e0)

    with patch('usb.core.find', return_value=[still_connected]), \
         patch.object(engine, 'teardown') as mock_teardown:
        engine.on_device_disconnected(0x1038, 0x12e0)

    mock_teardown.assert_not_called()


def test_on_disconnected_product_string_device_gone_calls_teardown():
    config = _make_config(product_string='Arctis Nova Pro Wireless', product_ids=[0x12e0])
    engine = _make_engine(configs=[config])
    engine.usb_device = _make_usb_device('Arctis Nova Pro Wireless', 0x12e0)
    engine.device_config = config

    with patch('usb.core.find', return_value=[]), \
         patch.object(engine, 'teardown') as mock_teardown:
        engine.on_device_disconnected(0x1038, 0x12e0)

    mock_teardown.assert_called_once()


def test_on_disconnected_usberror_during_product_scan_calls_teardown():
    config = _make_config(product_string='Arctis Nova Pro Wireless', product_ids=[0x12e0])
    engine = _make_engine(configs=[config])
    engine.usb_device = _make_usb_device('Arctis Nova Pro Wireless', 0x12e0)
    engine.device_config = config

    with patch('usb.core.find', side_effect=usb.core.USBError('error')), \
         patch.object(engine, 'teardown') as mock_teardown:
        engine.on_device_disconnected(0x1038, 0x12e0)

    mock_teardown.assert_called_once()


def test_on_disconnected_no_product_string_falls_back_to_vid_pid():
    config = _make_config(product_string=None, product_ids=[0x12e0])
    engine = _make_engine(configs=[config])
    engine.usb_device = MagicMock()
    engine.device_config = config

    with patch('usb.core.find', return_value=None), \
         patch.object(engine, 'teardown') as mock_teardown:
        engine.on_device_disconnected(0x1038, 0x12e0)

    mock_teardown.assert_called_once()
