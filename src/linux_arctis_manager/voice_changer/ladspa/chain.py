from __future__ import annotations

import logging

import pulsectl

from linux_arctis_manager.voice_changer.ladspa.effects import (
    ChorusEffect, DelayEffect, DistortionEffect, PitchEffect, ReverbEffect,
)

logger = logging.getLogger('LADSPAVoiceChanger')

from linux_arctis_manager.constants import ARCTIS_VC_SINK  # noqa: E402 — single source
ARCTIS_VC_MIC      = 'Arctis_VC_Mic'
ARCTIS_VC_MIC_DESC = 'Arctis Manager VC Mic'


class LADSPAChainConfig:
    """Aggregated LADSPA effect chain configuration."""
    def __init__(self) -> None:
        self.source_id   = ''
        self.pitch       = PitchEffect()
        self.chorus      = ChorusEffect()
        self.delay       = DelayEffect()
        self.distortion  = DistortionEffect()
        self.reverb      = ReverbEffect()

    @property
    def active(self) -> bool:
        return bool(self.source_id) and any([
            self.pitch.enabled,
            self.chorus.enabled,
            self.delay.enabled,
            self.distortion.enabled,
            self.reverb.enabled,
        ])


class LADSPAVoiceChanger:
    def __init__(self) -> None:
        self._pulse: pulsectl.Pulse | None = None
        self._chain_modules: list[int] = []
        self._loopback_module: int | None = None
        self._null_sink_module: int | None = None
        self._counter = 0

    def _pulse_conn(self) -> pulsectl.Pulse:
        if self._pulse is None:
            self._pulse = pulsectl.Pulse('arctis-vc-ladspa')
        return self._pulse

    def _next_name(self, stage: str) -> str:
        name = f'{ARCTIS_VC_SINK}_{stage}_{self._counter}'
        self._counter += 1
        return name

    def apply(self, config: LADSPAChainConfig) -> bool:
        logger.info('Applying LADSPA VC chain (source=%r active=%s)',
                    config.source_id, config.active)
        self._teardown_chain()

        if not config.active:
            logger.info('LADSPA VC inactive — chain cleared')
            return True

        pulse = self._pulse_conn()
        current = config.source_id
        stages: list[str] = ['<physical>']

        # Order: pitch → chorus → delay → distortion → reverb (reverb last: stereo out)
        for effect, tag in [
            (config.pitch,       'Pitch'),
            (config.chorus,      'Chorus'),
            (config.delay,       'Delay'),
            (config.distortion,  'Distortion'),
            (config.reverb,      'Reverb'),
        ]:
            if not effect.enabled:
                continue
            name = self._next_name(tag)
            args = effect.build_module_args(name, current)
            if args is None:
                logger.warning('VC %s: plugin not found — skipping', tag)
                continue
            try:
                idx = pulse.module_load('module-ladspa-source', args)
                self._chain_modules.append(idx)
                logger.info('VC LADSPA source %r loaded (module %d)', name, idx)
                current = name
                stages.append(tag)
            except Exception as e:
                logger.error('VC %s stage failed: %s — skipping', tag, e)

        if not self._ensure_null_sink(pulse):
            self._teardown_chain()
            return False

        if not self._load_loopback(pulse, current):
            self._teardown_chain()
            return False

        logger.info('VC chain: %s', ' → '.join(stages + [ARCTIS_VC_MIC]))
        return True

    def teardown(self) -> None:
        logger.info('LADSPA VC teardown')
        self._teardown_chain()
        if self._pulse:
            try:
                self._pulse.close()
            except Exception:
                pass
            self._pulse = None

    def _ensure_null_sink(self, pulse: pulsectl.Pulse) -> bool:
        try:
            existing = next(
                (s for s in pulse.sink_list()
                 if s.name == ARCTIS_VC_SINK
                 or s.proplist.get('node.name', '') == ARCTIS_VC_SINK),
                None,
            )
            if existing:
                return True
            idx = pulse.module_load(
                'module-null-sink',
                f'sink_name={ARCTIS_VC_SINK} '
                f'sink_properties=node.description="Arctis VC Output" '
                f'source_name={ARCTIS_VC_MIC} '
                f'source_properties=node.description="{ARCTIS_VC_MIC_DESC}"',
            )
            self._null_sink_module = idx
            logger.info('VC null sink created (module %d)', idx)
            return True
        except Exception as e:
            logger.error('VC: failed to create null sink: %s', e)
            return False

    def _load_loopback(self, pulse: pulsectl.Pulse, source_name: str) -> bool:
        try:
            idx = pulse.module_load(
                'module-loopback',
                f'source={source_name} sink={ARCTIS_VC_SINK} latency_msec=5',
            )
            self._loopback_module = idx
            logger.info('VC loopback %r → %r (module %d)', source_name, ARCTIS_VC_SINK, idx)
            return True
        except Exception as e:
            logger.error('VC: failed to load loopback: %s', e)
            return False

    def _teardown_chain(self) -> None:
        if not self._chain_modules and self._loopback_module is None and self._null_sink_module is None:
            return
        pulse = self._pulse_conn()
        for mod_id in reversed(list(self._chain_modules)):
            try:
                pulse.module_unload(mod_id)
            except Exception as e:
                logger.warning('VC: unload module %d: %s', mod_id, e)
        self._chain_modules.clear()
        for mod_id, lbl in [(self._loopback_module, 'loopback'),
                             (self._null_sink_module, 'null sink')]:
            if mod_id is not None:
                try:
                    pulse.module_unload(mod_id)
                except Exception as e:
                    logger.warning('VC: unload %s %d: %s', lbl, mod_id, e)
        self._loopback_module = None
        self._null_sink_module = None
