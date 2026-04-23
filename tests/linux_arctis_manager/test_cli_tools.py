from unittest.mock import MagicMock, patch

import pytest
import usb

from linux_arctis_manager.cli_tools import arctis_usb_info, endpoint_direction, endpoint_type


# --- endpoint_type ---

def test_endpoint_type_control():
    assert endpoint_type(usb.util.ENDPOINT_TYPE_CTRL) == "Control"


def test_endpoint_type_isochronous():
    assert endpoint_type(usb.util.ENDPOINT_TYPE_ISO) == "Isochronous"


def test_endpoint_type_bulk():
    assert endpoint_type(usb.util.ENDPOINT_TYPE_BULK) == "Bulk"


def test_endpoint_type_interrupt():
    assert endpoint_type(usb.util.ENDPOINT_TYPE_INTR) == "Interrupt"



def test_endpoint_type_masks_to_low_two_bits():
    # 0xFC has low 2 bits = 0x00 = ENDPOINT_TYPE_CTRL
    assert endpoint_type(0xFC) == "Control"
    # 0xFD has low 2 bits = 0x01 = ENDPOINT_TYPE_ISO
    assert endpoint_type(0xFD) == "Isochronous"


# --- endpoint_direction ---

def test_endpoint_direction_in():
    assert endpoint_direction(usb.util.ENDPOINT_IN) == "IN"


def test_endpoint_direction_out():
    assert endpoint_direction(usb.util.ENDPOINT_OUT) == "OUT"


def test_endpoint_direction_in_with_address_bits():
    # 0x81: IN direction (bit 7 set) + endpoint number 1
    assert endpoint_direction(0x81) == "IN"


def test_endpoint_direction_out_with_address_bits():
    # 0x02: OUT direction (bit 7 clear) + endpoint number 2
    assert endpoint_direction(0x02) == "OUT"


# --- arctis_usb_info ---

def _make_endpoint(address, attributes, max_packet_size=64):
    ep = MagicMock()
    ep.bEndpointAddress = address
    ep.bmAttributes = attributes
    ep.wMaxPacketSize = max_packet_size
    return ep


def _make_interface(number, alt_setting, interface_class, endpoints):
    iface = MagicMock()
    iface.bInterfaceNumber = number
    iface.bAlternateSetting = alt_setting
    iface.bInterfaceClass = interface_class
    iface.__iter__ = MagicMock(return_value=iter(endpoints))
    return iface


def _make_config(value, interfaces):
    config = MagicMock(spec=usb.core.Configuration)
    config.bConfigurationValue = value
    config.__iter__ = MagicMock(return_value=iter(interfaces))
    return config


def _make_device(vendor_id, product_id, manufacturer, product, configs, langids=(1033,)):
    device = MagicMock(spec=usb.core.Device)
    device.idVendor = vendor_id
    device.idProduct = product_id
    device.manufacturer = manufacturer
    device.product = product
    device.langids = langids
    device.__iter__ = MagicMock(return_value=iter(configs))
    return device


def test_arctis_usb_info_raises_when_no_devices_found():
    with patch('usb.core.find', return_value=None):
        with pytest.raises(ValueError, match="No devices found"):
            arctis_usb_info()


def test_arctis_usb_info_raises_when_empty_list():
    with patch('usb.core.find', return_value=[]):
        with pytest.raises(ValueError, match="No devices found"):
            arctis_usb_info()


def test_arctis_usb_info_patches_missing_langids(capsys):
    endpoint = _make_endpoint(usb.util.ENDPOINT_IN, usb.util.ENDPOINT_TYPE_INTR)
    interface = _make_interface(0, 0, 0x03, [endpoint])
    config = _make_config(1, [interface])
    device = _make_device(0x1038, 0x12e0, 'SteelSeries', 'Arctis Nova', [config], langids=())

    with patch('usb.core.find', return_value=[device]):
        arctis_usb_info()

    assert device._langids == (1033,)


def test_arctis_usb_info_skips_non_hid_interfaces(capsys):
    endpoint = _make_endpoint(usb.util.ENDPOINT_IN, usb.util.ENDPOINT_TYPE_INTR)
    hid_iface = _make_interface(0, 0, 0x03, [endpoint])
    non_hid_iface = _make_interface(1, 0, 0x08, [endpoint])  # Mass storage
    config = _make_config(1, [hid_iface, non_hid_iface])
    device = _make_device(0x1038, 0x12e0, 'SteelSeries', 'Arctis Nova', [config])

    with patch('usb.core.find', return_value=[device]):
        arctis_usb_info()

    captured = capsys.readouterr()
    assert captured.out.count('HID interface') == 1


def test_arctis_usb_info_prints_device_header(capsys):
    config = _make_config(1, [])
    device = _make_device(0x1038, 0x12e0, 'SteelSeries', 'Arctis Nova', [config])

    with patch('usb.core.find', return_value=[device]):
        arctis_usb_info()

    captured = capsys.readouterr()
    assert 'SteelSeries Arctis Nova (1038:12e0)' in captured.out


def test_arctis_usb_info_prints_endpoint_info(capsys):
    endpoint = _make_endpoint(usb.util.ENDPOINT_IN, usb.util.ENDPOINT_TYPE_INTR, max_packet_size=32)
    interface = _make_interface(2, 0, 0x03, [endpoint])
    config = _make_config(1, [interface])
    device = _make_device(0x1038, 0x12e0, 'SteelSeries', 'Arctis Nova', [config])

    with patch('usb.core.find', return_value=[device]):
        arctis_usb_info()

    captured = capsys.readouterr()
    assert 'HID interface (num : alt): 2 : 0' in captured.out
    assert 'Dir=IN' in captured.out
    assert 'Type=Interrupt' in captured.out
    assert 'MaxPacketSize=32' in captured.out


def test_arctis_usb_info_handles_configuration_element(capsys):
    endpoint = _make_endpoint(usb.util.ENDPOINT_IN, usb.util.ENDPOINT_TYPE_INTR)
    interface = _make_interface(0, 0, 0x03, [endpoint])
    config = _make_config(1, [interface])
    device = _make_device(0x1038, 0x12e0, 'SteelSeries', 'Arctis Nova', [config])

    config_element = MagicMock(spec=usb.core.Configuration)
    config_element.device = device

    with patch('usb.core.find', return_value=[config_element]):
        arctis_usb_info()

    captured = capsys.readouterr()
    assert 'SteelSeries' in captured.out


def test_arctis_usb_info_custom_interface_class_filter(capsys):
    endpoint = _make_endpoint(usb.util.ENDPOINT_IN, usb.util.ENDPOINT_TYPE_BULK)
    hid_iface = _make_interface(0, 0, 0x03, [endpoint])
    mass_storage_iface = _make_interface(1, 0, 0x08, [endpoint])
    config = _make_config(1, [hid_iface, mass_storage_iface])
    device = _make_device(0x1038, 0x12e0, 'SteelSeries', 'Arctis Nova', [config])

    with patch('usb.core.find', return_value=[device]):
        arctis_usb_info(bInterfaceClass=0x08)

    captured = capsys.readouterr()
    assert captured.out.count('HID interface') == 1
    assert 'num : alt): 1 : 0' in captured.out
