from argparse import ArgumentParser
import asyncio
import logging
import signal
import sys

from PySide6.QtCore import Qt, QMetaObject, QTimer
from PySide6.QtWidgets import QApplication

from linux_arctis_manager.gui.base_app import QBaseDesktopApp
from linux_arctis_manager.gui.main_app import QMainApp
from linux_arctis_manager.gui.systray_app import QSystrayApp
from linux_arctis_manager.systemd import ensure_systemd_unit


def main():
    parser = ArgumentParser()
    parser.add_argument('--systray', action='store_true', help='Run systray app, instead of opening the main window')
    parser.add_argument('--verbose', '-v', action='count', default=0, help='Increase verbosity (up to -vvvv)')
    parser.add_argument('--no-enforce-systemd', action='store_true', help='Do not enforce systemd unit')
    args = parser.parse_args()

    log_level = logging.CRITICAL
    for _ in range(args.verbose):
        log_level -= 10
    if log_level < logging.DEBUG:
        log_level = logging.DEBUG

    logging.basicConfig(level=log_level, format='%(name)20s %(levelname)8s | %(message)s')

    app = QApplication(sys.argv)
    if args.systray:
        q_object = QSystrayApp(app, log_level)
        app.setQuitOnLastWindowClosed(False)
    else:
        q_object = QMainApp(app, log_level)
        app.setQuitOnLastWindowClosed(True)
    
    if not args.no_enforce_systemd:
        ensure_systemd_unit(True)

    timer = QTimer()
    timer.timeout.connect(lambda: None)
    timer.start(500)

    def stop_app(*_) -> None:
        QTimer.singleShot(0, q_object.sig_stop)
        q_object.sig_stop()
        if timer.isActive():
            timer.stop()

    signal.signal(signal.SIGINT, stop_app)
    signal.signal(signal.SIGTERM, stop_app)

    if app.quitOnLastWindowClosed():
        app.lastWindowClosed.connect(stop_app)

    asyncio.run(q_object.start())

if __name__ == '__main__':
    main()
