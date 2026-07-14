from __future__ import annotations

import logging
import subprocess

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
        cmd = ['pactl', 'load-module', 'module-loopback', 'latency_msec=5']
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
        sink: Arctis_Media (always explicit so PipeWire doesn't pick randomly)
        """
        source = self.nc_widget._source_combo.currentData() or ''
        sink = 'Arctis_Media'
        try:
            import pulsectl
            with pulsectl.Pulse('lam-sidetone-probe') as pulse:
                source_names = {s.name for s in pulse.source_list()}
            for candidate in ('Arctis_VC_Sink.monitor', 'Arctis_Manager_Mic', 'Arctis_NC_Mic'):
                if candidate in source_names:
                    source = candidate
                    break
        except Exception:
            pass
        return source, sink

    def _on_preview_toggled(self, checked: bool) -> None:
        if checked:
            source, sink = self._pick_preview_endpoints()
            if not self._preview.start(source, sink):
                self._preview_btn.blockSignals(True)
                self._preview_btn.setChecked(False)
                self._preview_btn.blockSignals(False)
        else:
            self._preview.stop()
