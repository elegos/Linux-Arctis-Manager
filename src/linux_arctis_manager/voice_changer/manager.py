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

    def apply(self, settings: 'VCSettings', nc_source: str | None = None) -> bool:
        """
        nc_source: if NC chain is active, pass its output source name here so RVC
                   records from NC output instead of the saved (possibly physical) source.
        """
        from linux_arctis_manager.voice_changer.settings import VCSettings

        # Always tear down existing chains first
        self._teardown_all()

        if not settings.enabled:
            logger.info('VC disabled — all chains cleared')
            return True

        if settings.mode == 'rvc':
            return self._apply_rvc(settings, nc_source)
        return self._apply_ladspa(settings)

    def teardown(self) -> None:
        self._teardown_all()

    def update_rvc_params(self, params: 'RVCParams') -> bool:
        """Live tuning update on the active RVC chain (auto-tuner path)."""
        return self._rvc.update_params(params) if self._rvc else False

    def rvc_metrics(self) -> dict | None:
        return self._rvc.get_metrics() if self._rvc else None

    # ── Internal ──────────────────────────────────────────────────────────

    def _apply_ladspa(self, settings: 'VCSettings') -> bool:
        from linux_arctis_manager.voice_changer.ladspa.chain import LADSPAVoiceChanger
        cfg = settings.to_ladspa_config()
        self._ladspa = LADSPAVoiceChanger()
        return self._ladspa.apply(cfg)

    def _apply_rvc(self, settings: 'VCSettings', nc_source: str | None = None) -> bool:
        from linux_arctis_manager.voice_changer.rvc.rvc_chain import RVCVoiceChanger
        from linux_arctis_manager.voice_changer.rvc.backend import RVCParams
        # Prefer NC chain output over the saved (possibly physical) source_id
        source_id = nc_source or settings.source_id
        if nc_source:
            logger.info('RVC: using NC chain output as source (%s)', nc_source)
        chain = RVCVoiceChanger()
        params = RVCParams(
            hubert_model=settings.rvc_hubert_model,
            vtln_alpha=settings.rvc_vtln_alpha,
            rms_mix_rate=settings.rvc_rms_mix_rate,
            filter_radius=settings.rvc_filter_radius,
            target_rms=settings.rvc_target_rms,
            limiter_thr=settings.rvc_limiter_thr,
        )
        ok = chain.apply(
            source_id=source_id,
            model_name=settings.rvc_model,
            pitch_offset=settings.rvc_pitch_offset,
            params=params,
        )
        if ok:
            self._rvc = chain
        else:
            chain.teardown()
        return ok

    def _teardown_all(self) -> None:
        if self._ladspa:
            self._ladspa.teardown()
            self._ladspa = None
        if self._rvc:
            self._rvc.teardown()
            self._rvc = None
