from typing import cast

import usb

from linux_arctis_manager.core import TypedDevice


def endpoint_type(bmAttributes):
    etype = bmAttributes & 0x3
    return {
        usb.util.ENDPOINT_TYPE_CTRL: "Control",
        usb.util.ENDPOINT_TYPE_ISO: "Isochronous",
        usb.util.ENDPOINT_TYPE_BULK: "Bulk",
        usb.util.ENDPOINT_TYPE_INTR: "Interrupt",
    }.get(etype, "Unknown")

def endpoint_direction(bEndpointAddress):
    return "IN" if usb.util.endpoint_direction(bEndpointAddress) == usb.util.ENDPOINT_IN else "OUT"


def arctis_usb_info(vendor_id: int = 0x1038, bInterfaceClass: int = 0x03):
    usb_elements = usb.core.find(idVendor=vendor_id)

    if not usb_elements:
        raise ValueError(f"No devices found with vendor ID {vendor_id:04x}")
    
    for element in usb_elements:
        device: TypedDevice
        if isinstance(element, usb.core.Configuration):
            device = cast(TypedDevice, element.device)
        else:
            device = cast(TypedDevice, element)
        
        print(f'{device.manufacturer} {device.product} ({device.idVendor:04x}:{device.idProduct:04x})')
        for config in device:
            print(f'\tConfiguration: {config.bConfigurationValue}')
            for interface in config:
                if interface.bInterfaceClass != bInterfaceClass:
                    continue
                print(f'\t\tHID interface (num : alt): {interface.bInterfaceNumber} : {interface.bAlternateSetting}')
                for endpoint in interface:
                    print(
                        f'\t\t\tEndpoint: {endpoint.bEndpointAddress:02x} '
                        f'Dir={endpoint_direction(endpoint.bEndpointAddress)} '
                        f'Type={endpoint_type(endpoint.bmAttributes)} '
                        f'MaxPacketSize={endpoint.wMaxPacketSize} '
                    )
        
