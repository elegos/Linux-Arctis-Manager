from __future__ import annotations

import logging

logger = logging.getLogger('VoiceChangerManager')


class VoiceChangerManager:
    """
    Orchestrates LADSPA or RVC voice changer based on VCSettings.mode.
    Holds one active chain at a time and tears it down before rebuilding.
    """

    def __init__(self) -> None:
        self._ladspa: 'LADSPAVoiceChanger | None' = None
        self._rvc:    'RVCVoiceChanger | None'     = None

    def apply(self, settings: 'VCSettings') -> bool:
        from linux_arctis_manager.voice_changer.settings import VCSettings

        # Always tear down existing chains first
        self._teardown_all()

        if not settings.enabled:
            logger.info('VC disabled — all chains cleared')
            return True

        if settings.mode == 'rvc':
            return self._apply_rvc(settings)
        return self._apply_ladspa(settings)

    def teardown(self) -> None:
        self._teardown_all()

    # ── Internal ──────────────────────────────────────────────────────────

    def _apply_ladspa(self, settings: 'VCSettings') -> bool:
        from linux_arctis_manager.voice_changer.ladspa.chain import LADSPAVoiceChanger
        cfg = settings.to_ladspa_config()
        self._ladspa = LADSPAVoiceChanger()
        return self._ladspa.apply(cfg)

    def _apply_rvc(self, settings: 'VCSettings') -> bool:
        from linux_arctis_manager.voice_changer.rvc.rvc_chain import RVCVoiceChanger
        self._rvc = RVCVoiceChanger()
        return self._rvc.apply(
            source_id=settings.source_id,
            model_name=settings.rvc_model,
            pitch_offset=settings.rvc_pitch_offset,
        )

    def _teardown_all(self) -> None:
        if self._ladspa:
            self._ladspa.teardown()
            self._ladspa = None
        if self._rvc:
            self._rvc.teardown()
            self._rvc = None
