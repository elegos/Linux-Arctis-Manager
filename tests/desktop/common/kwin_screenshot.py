"""Saves KWin's whole screen to a file. Runs on the system Python (PyGObject).

Needs KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1 in KWin's environment.
"""

import os
import subprocess
import sys
import threading

from gi.repository import Gio, GLib  # pyright: ignore[reportMissingImports]


def main(target: str) -> None:
    read_end, write_end = os.pipe()
    chunks: list[bytes] = []

    def drain() -> None:
        while chunk := os.read(read_end, 1 << 20):
            chunks.append(chunk)

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    fds = Gio.UnixFDList()
    handle = fds.append(write_end)
    os.close(write_end)
    result, _ = Gio.bus_get_sync(Gio.BusType.SESSION).call_with_unix_fd_list_sync(
        'org.kde.KWin', '/org/kde/KWin/ScreenShot2', 'org.kde.KWin.ScreenShot2', 'CaptureWorkspace',
        GLib.Variant('(a{sv}h)', ({}, handle)), None, Gio.DBusCallFlags.NONE, 30000, fds, None)
    del fds  # closes our copy of the write end, so the reader sees the end
    reader.join(30)
    meta = result.unpack()[0]
    subprocess.run(
        ['magick', '-size', f"{meta['width']}x{meta['height']}", '-depth', '8', 'bgra:-', '-alpha', 'off', target],
        input=b''.join(chunks), check=True)


if __name__ == '__main__':
    main(sys.argv[1])
