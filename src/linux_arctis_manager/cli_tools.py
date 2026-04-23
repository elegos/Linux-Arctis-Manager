from typing import cast

import usb

from linux_arctis_manager.core import TypedDevice
from linux_arctis_manager.typing.protocols import UsbConfig


def endpoint_type(bmAttributes: int) -> str:
    etype = bmAttributes & 0x3
    return {
        usb.util.ENDPOINT_TYPE_CTRL: "Control",
        usb.util.ENDPOINT_TYPE_ISO: "Isochronous",
        usb.util.ENDPOINT_TYPE_BULK: "Bulk",
        usb.util.ENDPOINT_TYPE_INTR: "Interrupt",
    }.get(etype, "Unknown")


def endpoint_direction(bEndpointAddress: int) -> str:
    return "IN" if usb.util.endpoint_direction(bEndpointAddress) == usb.util.ENDPOINT_IN else "OUT"


def arctis_usb_info(vendor_id: int = 0x1038, bInterfaceClass: int = 0x03) -> None:
    usb_elements = usb.core.find(idVendor=vendor_id)

    if not usb_elements:
        raise ValueError(f"No devices found with vendor ID {vendor_id:04x}")

    for element in usb_elements:
        device: TypedDevice
        if isinstance(element, usb.core.Configuration):
            device = cast(TypedDevice, element.device)
        else:
            device = cast(TypedDevice, element)

        if not hasattr(device, 'langids') or not device.langids:
            setattr(device, '_langids', (1033,))

        print(f'{device.manufacturer} {device.product} ({device.idVendor:04x}:{device.idProduct:04x})')
        for config in device:
            cfg = cast(UsbConfig, cast(object, config))
            print(f'\tConfiguration: {cfg.bConfigurationValue}')
            for interface in cfg:
                if interface.bInterfaceClass != bInterfaceClass:
                    continue
                print(f'\t\tHID interface (num : alt): {interface.bInterfaceNumber} : {interface.bAlternateSetting}')
                for endpoint in interface:
                    ep = endpoint
                    addr = ep.bEndpointAddress
                    print(f'\t\t\tEndpoint: {addr:02x} Dir={endpoint_direction(addr)} Type={endpoint_type(ep.bmAttributes)} MaxPacketSize={ep.wMaxPacketSize}')
