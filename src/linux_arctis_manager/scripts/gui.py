import logging
import signal
import subprocess
import sys
import time
from argparse import ArgumentParser

from PySide6.QtCore import QTimer
from PySide6.QtWidgets import QApplication, QMessageBox

from linux_arctis_manager.constants import DBUS_BUS_NAME, SYSTEMD_SERVICE_NAME
from linux_arctis_manager.gui.main_app import QMainApp
from linux_arctis_manager.gui.systray_app import QSystrayApp
from linux_arctis_manager.i18n import I18n
from linux_arctis_manager.systemd import ensure_systemd_unit


def _is_dbus_service_available() -> bool:
    """Check if the daemon's D-Bus name is currently registered."""
    try:
        result = subprocess.run(
            ['dbus-send', '--session', '--print-reply', '--reply-timeout=2000',
             '--dest=org.freedesktop.DBus', '/org/freedesktop/DBus',
             'org.freedesktop.DBus.GetNameOwner', f'string:{DBUS_BUS_NAME}'],
            capture_output=True, timeout=3,
        )
        return result.returncode == 0
    except (subprocess.TimeoutExpired, FileNotFoundError):
        return False


def _wait_for_dbus_service(timeout: float = 10.0) -> bool:
    """Poll until the daemon's D-Bus name appears or timeout expires."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if _is_dbus_service_available():
            return True
        time.sleep(0.5)
    return False


def _show_daemon_offline_error() -> None:
    msg = QMessageBox()
    msg.setIcon(QMessageBox.Icon.Critical)
    msg.setWindowTitle(I18n.translate('ui', 'daemon_offline_title'))
    msg.setText(I18n.translate('ui', 'daemon_offline_message'))
    msg.setInformativeText(
        I18n.translate('ui', 'daemon_offline_hint') +
        f'\n    systemctl --user start {SYSTEMD_SERVICE_NAME}'
    )
    msg.exec()


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

    # On Wayland, xdg-toplevel windows cannot be positioned programmatically —
    # the compositor controls placement (KWin ignores move() hints).
    # Force XWayland for the systray so the popup panel appears near the icon.
    import os
    if args.systray and os.environ.get('WAYLAND_DISPLAY') and not os.environ.get('QT_QPA_PLATFORM'):
        os.environ['QT_QPA_PLATFORM'] = 'xcb'

    app = QApplication(sys.argv)
    app.setApplicationName('Arctis Manager')
    app.setApplicationDisplayName('Arctis Manager')

    # Ensure the daemon is reachable before creating the app objects that
    # open D-Bus connections.  QMainApp.__init__ calls dbus_wrapper.start()
    # which fires off threads immediately, so this check must come first.
    if not _is_dbus_service_available():
        if args.no_enforce_systemd:
            _show_daemon_offline_error()
            sys.exit(1)
        else:
            ensure_systemd_unit(True)
            if not _wait_for_dbus_service():
                _show_daemon_offline_error()
                sys.exit(1)

    q_object = None
    if args.systray:
        q_object = QSystrayApp(app, log_level)
        app.setQuitOnLastWindowClosed(False)
    else:
        q_object = QMainApp(app, log_level)
        app.setQuitOnLastWindowClosed(True)

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

    if q_object:
        import asyncio
        asyncio.run(q_object.start())

if __name__ == '__main__':
    main()
