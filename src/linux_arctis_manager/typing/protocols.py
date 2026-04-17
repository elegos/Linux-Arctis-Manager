from collections.abc import Iterator
from typing import Protocol


class UsbEndpoint(Protocol):
    bEndpointAddress: int
    bmAttributes: int
    wMaxPacketSize: int


class UsbInterface(Protocol):
    bInterfaceClass: int
    bInterfaceNumber: int
    bAlternateSetting: int
    def __iter__(self) -> Iterator[UsbEndpoint]: ...


class UsbConfig(Protocol):
    bConfigurationValue: int
    def __iter__(self) -> Iterator[UsbInterface]: ...
