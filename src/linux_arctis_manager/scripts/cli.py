import sys
from argparse import ArgumentParser

from linux_arctis_manager.cli_tools import arctis_usb_info
from linux_arctis_manager.utils import project_version


def main():
    parser = ArgumentParser(description=f'Arctis Manager CLI v {project_version()}')
    subparsers = parser.add_subparsers(dest='command', required=True)

    tools_parser = subparsers.add_parser('tools', help='Reverse engineering tools')
    usb_devices_subparser = tools_parser.add_subparsers(dest='action', required=True)
    arctis_devices_parser = usb_devices_subparser.add_parser(
        'arctis-devices',
        help='List important Arctis device(s) information, like HID interfaces, alternate configs, etc.'
    )
    arctis_devices_parser.add_argument('--vendor-id', default=0x1038, type=int)

    args = parser.parse_args()

    if args.command == 'tools':
        if args.action == 'arctis-devices':
            sys.exit(arctis_usb_info(args.vendor_id))


if __name__ == '__main__':
    main()
