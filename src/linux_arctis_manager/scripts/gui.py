from argparse import ArgumentParser
import asyncio
import logging
import signal
import sys

from PySide6.QtCore import Qt, QMetaObject, QTimer
from PySide6.QtWidgets import QApplication

from linux_arctis_manager.gui.base_app import QBaseDesktopApp
from linux_arctis_manager.gui.systray_app import QSystrayApp
from linux_arctis_manager.systemd import ensure_systemd_unit


def main():
    parser = ArgumentParser()
    parser.add_argument('--systray', action='store_true', help='Run systray app, instead of opening the main window')
    parser.add_argument('--verbose', '-v', action='count', default=0, help='Increase verbosity (up to -vvvv)')
    args = parser.parse_args()

    log_level = logging.CRITICAL
    for _ in range(args.verbose):
        log_level -= 10
    if log_level < logging.DEBUG:
        log_level = logging.DEBUG

    logging.basicConfig(level=log_level, format='%(name)20s %(levelname)8s | %(message)s')

    main_app: QApplication|None = None
    app: QBaseDesktopApp|None = None
    if args.systray:
        main_app = QApplication(sys.argv)
        app = QSystrayApp(main_app, log_level)
    else:
        raise Exception('Not implemented yet')
    
    if app and main_app:
        ensure_systemd_unit(True)

        timer = QTimer()
        timer.timeout.connect(lambda: None)
        timer.start(500)

        def stop_app(*_) -> None:
            QTimer.singleShot(0, app.sig_stop)

        signal.signal(signal.SIGINT, stop_app)
        signal.signal(signal.SIGTERM, stop_app)

        asyncio.run(app.start())

if __name__ == '__main__':
    main()
