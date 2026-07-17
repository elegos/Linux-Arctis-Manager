from __future__ import annotations

import logging
import subprocess

from PySide6.QtCore import QTimer
from PySide6.QtGui import QHideEvent
from PySide6.QtWidgets import QApplication, QHBoxLayout, QPushButton, QTabWidget, QVBoxLayout, QWidget

from linux_arctis_manager.gui.nc_widget import QNCWidget
from linux_arctis_manager.gui.vc_widget import QVCWidget
from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('mic_widget')

_T = lambda s, k: I18n.translate(s, k)  # noqa: E731


class _SidetonePreview:
    def __init__(self) -> None:
        self._module_id: int | None = None

    @property
    def active(self) -> bool:
        return self._module_id is not None

    def start(self, source_name: str = '', sink_name: str = '') -> bool:
        self.stop()
        self._sweep_stale()
        # dont_move: if our source (e.g. Arctis_VC_Sink.monitor) disappears —
        # daemon teardown, crash — the loopback must NOT be re-targeted by the
        # session manager.  Without this it lands on the headset's own monitor
        # and feeds playback back into the output (audible echo) even after
        # the daemon is long gone.
        cmd = ['pactl', 'load-module', 'module-loopback', 'latency_msec=5',
               'source_dont_move=true', 'sink_dont_move=true']
        if source_name:
            cmd.append(f'source={source_name}')
        if sink_name:
            cmd.append(f'sink={sink_name}')
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=3)
            if result.returncode == 0 and result.stdout.strip().isdigit():
                self._module_id = int(result.stdout.strip())
                return True
            logger.warning('sidetone start failed: %s', result.stderr.strip())
        except Exception as e:
            logger.warning('sidetone start error: %s', e)
        return False

    def stop(self) -> None:
        if self._module_id is not None:
            try:
                subprocess.run(
                    ['pactl', 'unload-module', str(self._module_id)],
                    capture_output=True, timeout=3,
                )
            except Exception as e:
                logger.warning('sidetone stop error: %s', e)
            self._module_id = None

    _STALE_MARKERS = ('Arctis_VC_Sink.monitor', 'Arctis_Manager_Mic', 'Arctis_NC_Mic')

    def _sweep_stale(self) -> None:
        """Unload sidetone loopbacks leaked by a previous crashed session.

        The module lives in the PipeWire server, so it survives both GUI and
        daemon; a leaked one whose source has since disappeared gets re-homed
        to the headset monitor and echoes all playback.
        """
        try:
            result = subprocess.run(['pactl', 'list', 'modules', 'short'],
                                    capture_output=True, text=True, timeout=3)
            for line in result.stdout.splitlines():
                parts = line.split('\t')
                if (len(parts) >= 3 and parts[1] == 'module-loopback'
                        and any(m in parts[2] for m in self._STALE_MARKERS)):
                    subprocess.run(['pactl', 'unload-module', parts[0]],
                                   capture_output=True, timeout=3)
                    logger.info('swept stale sidetone loopback (module %s)', parts[0])
        except Exception as e:
            logger.warning('stale sidetone sweep error: %s', e)


class QMicWidget(QWidget):
    def __init__(self, parent: QWidget) -> None:
        super().__init__(parent)

        self._preview = _SidetonePreview()

        layout = QVBoxLayout()
        layout.setContentsMargins(0, 0, 0, 0)
        self.setLayout(layout)

        # ── Sidetone preview row ───────────────────────────────────────
        preview_row = QHBoxLayout()
        preview_row.setContentsMargins(4, 4, 4, 0)
        self._preview_btn = QPushButton(_T('ui', 'mic_sidetone_preview'))
        self._preview_btn.setCheckable(True)
        self._preview_btn.setChecked(False)
        self._preview_btn.setFixedWidth(200)
        self._preview_btn.toggled.connect(self._on_preview_toggled)
        preview_row.addWidget(self._preview_btn)
        preview_row.addStretch()
        layout.addLayout(preview_row)

        # ── Sub-tabs ───────────────────────────────────────────────────
        self._tabs = QTabWidget(self)
        self.nc_widget = QNCWidget(self._tabs, show_title=False)
        self._tabs.addTab(self.nc_widget, _T('ui', 'nc'))
        self.vc_widget = QVCWidget(self._tabs, show_title=False)
        self._tabs.addTab(self.vc_widget, _T('ui', 'vc'))
        layout.addWidget(self._tabs)

        app = QApplication.instance()
        if app:
            app.aboutToQuit.connect(self._stop_preview)

        # The sidetone loopback is pinned to its source (dont_move); when VC
        # settings apply, the daemon may rebuild the chain and recreate
        # Arctis_VC_Sink, orphaning the pinned loopback.  Restart the preview
        # once the rebuild has settled so the user doesn't have to.
        self._preview_restart_tries = 0
        self.vc_widget.sig_applied.connect(self._on_vc_applied)

    def _on_vc_applied(self) -> None:
        if self._preview_btn.isChecked():
            self._preview_restart_tries = 0
            QTimer.singleShot(1500, self._restart_preview)

    def _restart_preview(self) -> None:
        """Reload the preview loopback, waiting out a VC chain rebuild.

        A VC apply rebuilds the chain over several seconds (model load), and
        MicRouter recreates its nodes at the end — restarting too early pins
        the loopback to a source that is missing or about to be destroyed.
        Retry until the expected VC sink monitor is back (or, as a last
        resort, start with whatever endpoint is available).
        """
        if not self._preview_btn.isChecked():
            return
        self._preview_restart_tries += 1
        source, sink = self._pick_preview_endpoints()
        vc_on = self.vc_widget._enable_check.isChecked()
        waiting = vc_on and source != 'Arctis_VC_Sink.monitor' \
            and self._preview_restart_tries < 10
        ok = False if waiting else self._preview.start(source, sink)
        if not ok and self._preview_restart_tries < 10:
            QTimer.singleShot(1500, self._restart_preview)

    def hideEvent(self, event: QHideEvent) -> None:
        super().hideEvent(event)
        self._stop_preview()

    def _stop_preview(self) -> None:
        self._preview.stop()
        self._preview_btn.blockSignals(True)
        self._preview_btn.setChecked(False)
        self._preview_btn.blockSignals(False)

    def _pick_preview_endpoints(self) -> tuple[str, str]:
        """Return (source, sink) for the sidetone loopback.

        source priority: Arctis_VC_Sink.monitor (RVC active) >
                         Arctis_Manager_Mic > Arctis_NC_Mic > physical mic
        Using the VC sink monitor directly avoids a PipeWire loopback
        reconnect race that occurs when MicRouter recreates Arctis_Manager_Mic.
        sink: physical headset output directly — the Arctis_Media null sink
        depends on its own monitor loopback being alive, which is not
        guaranteed (it can be lost across EQ/chain rebuilds).
        """
        source = self.nc_widget._source_combo.currentData() or ''
        sink = ''
        try:
            import pulsectl
            with pulsectl.Pulse('lam-sidetone-probe') as pulse:
                source_names = {s.name for s in pulse.source_list()}
                sink_names = [s.name for s in pulse.sink_list()]
            for candidate in ('Arctis_VC_Sink.monitor', 'Arctis_Manager_Mic', 'Arctis_NC_Mic'):
                if candidate in source_names:
                    source = candidate
                    break
            sink = next(
                (n for n in sink_names
                 if n.startswith('alsa_output') and ('SteelSeries' in n or 'Arctis' in n)),
                'Arctis_Media',
            )
        except Exception:
            pass
        return source, sink

    def _on_preview_toggled(self, checked: bool) -> None:
        if checked:
            # Same retrying path as the post-VC-apply restart: the VC chain
            # may be mid-rebuild when the user hits the button.
            self._preview_restart_tries = 0
            self._restart_preview()
        else:
            self._preview.stop()
