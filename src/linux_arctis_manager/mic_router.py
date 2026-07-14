from __future__ import annotations

import logging
import subprocess

import pulsectl

logger = logging.getLogger('MicRouter')

# The single user-facing microphone device.  Apps select this once and it
# automatically follows whichever processing chain is active (VC > NC > none).
ARCTIS_MIC_NAME = 'Arctis_Manager_Mic'
ARCTIS_MIC_DESC = 'Arctis Manager Mic'


class MicRouter:
    """
    Maintains a single stable PulseAudio source (Arctis_Manager_Mic) whose
    master changes as the active chain changes.

    Priority: VC output > NC output > teardown (no virtual mic)

    device.class=sound ensures the source appears in KDE/GNOME input device lists
    (module-remap-source sets device.class=filter by default, which hides it).
    """

    def __init__(self) -> None:
        self._module: int | None = None
        self._current_master: str | None = None
        self._pulse: pulsectl.Pulse | None = None

    # ── Public API ────────────────────────────────────────────────────────

    def update(self, master: str) -> bool:
        """Point Arctis_Manager_Mic at *master*.  Recreates the module if master changed."""
        if master == self._current_master and self._module is not None:
            return True
        self._unload()
        return self._load(master)

    def teardown(self) -> None:
        self._unload()
        if self._pulse:
            try:
                self._pulse.close()
            except Exception:
                pass
            self._pulse = None

    # ── Internals ─────────────────────────────────────────────────────────

    def _pulse_conn(self) -> pulsectl.Pulse:
        if self._pulse is None:
            self._pulse = pulsectl.Pulse('arctis-mic-router')
        return self._pulse

    def _load(self, master: str) -> bool:
        try:
            pulse = self._pulse_conn()
            # Inner quotes must be escaped as \" so the PA modargs parser treats
            # the whole source_properties value as one token.
            # node.virtual=false overrides PipeWire's automatic node.virtual=true
            # which would otherwise hide the source from KDE/GNOME device lists.
            props = (
                'node.virtual=false '
                f'node.description=\\"{ARCTIS_MIC_DESC}\\" '
                f'device.description=\\"{ARCTIS_MIC_DESC}\\" '
                'device.class=sound'
            )
            args = (
                f'source_name={ARCTIS_MIC_NAME} '
                f'master={master} '
                f'source_properties="{props}"'
            )
            for mod in ('module-remap-source', 'module-virtual-source'):
                try:
                    idx = pulse.module_load(mod, args)
                    self._module = idx
                    self._current_master = master
                    logger.info('MicRouter: %s → %s (module %d, via %s)',
                                ARCTIS_MIC_NAME, master, idx, mod)
                    # Set description via pactl after creation as a belt-and-suspenders
                    # fix in case PipeWire ignored the source_properties description.
                    try:
                        subprocess.run(
                            ['pactl', 'set-source-properties', ARCTIS_MIC_NAME,
                             f'node.description={ARCTIS_MIC_DESC}'],
                            capture_output=True, timeout=3,
                        )
                    except Exception:
                        pass
                    return True
                except Exception as e:
                    logger.warning('MicRouter: %s failed: %s', mod, e)
            logger.error('MicRouter: could not create %s → %s', ARCTIS_MIC_NAME, master)
            return False
        except Exception as e:
            logger.error('MicRouter: load failed: %s', e)
            return False

    def _unload(self) -> None:
        if self._module is None:
            return
        try:
            pulse = self._pulse_conn()
            pulse.module_unload(self._module)
            logger.info('MicRouter: unloaded module %d (%s)', self._module, self._current_master)
        except Exception as e:
            logger.warning('MicRouter: unload %d failed: %s', self._module, e)
        self._module = None
        self._current_master = None
