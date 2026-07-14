from __future__ import annotations

import logging
from pathlib import Path

from ruamel.yaml import YAML

_log = logging.getLogger('VCSettings')

VC_SETTINGS_FILE = Path.home() / '.config' / 'arctis_manager' / 'vc_settings.yaml'


class VCSettings:
    # Global
    enabled: bool
    mode:    str        # 'ladspa' | 'rvc'
    source_id: str

    # Pitch (LADSPA)
    pitch_enabled:   bool
    pitch_semitones: float   # -24..+24

    # Chorus (multivoice_chorus_1201)
    chorus_enabled:    bool
    chorus_voices:     int    # 1-8
    chorus_delay_ms:   float  # 10-40 ms
    chorus_sep_ms:     float  # 0-2 ms
    chorus_detune_pct: float  # 0-5 %
    chorus_lfo_hz:     float  # 2-30 Hz
    chorus_atten_db:   float  # -20-0 dB

    # Delay (delay_1898)
    delay_enabled: bool
    delay_s:       float  # 0-5 s

    # Distortion (valve_1209)
    distortion_enabled:   bool
    distortion_level:     float  # 0-1
    distortion_character: float  # 0-1

    # Reverb (gverb_1216)
    reverb_enabled:    bool
    reverb_roomsize_m: float  # 1-300 m
    reverb_time_s:     float  # 0.1-30 s
    reverb_damping:    float  # 0-1
    reverb_bandwidth:  float  # 0-1
    reverb_dry_db:     float  # -70-0 dB
    reverb_early_db:   float  # -70-0 dB
    reverb_tail_db:    float  # -70-0 dB

    # RVC
    rvc_model:        str
    rvc_pitch_offset: float  # semitones
    rvc_hubert_model: str    # 'torchaudio' | 'contentvec'
    rvc_vtln_alpha: float  # warp factor: <1 = formants up (male→female); 1.0 = off

    def __init__(self) -> None:
        self.enabled   = False
        self.mode      = 'ladspa'
        self.source_id = ''

        self.pitch_enabled   = False
        self.pitch_semitones = 0.0

        self.chorus_enabled    = False
        self.chorus_voices     = 3
        self.chorus_delay_ms   = 20.0
        self.chorus_sep_ms     = 0.5
        self.chorus_detune_pct = 1.0
        self.chorus_lfo_hz     = 4.0
        self.chorus_atten_db   = -3.0

        self.delay_enabled = False
        self.delay_s       = 0.3

        self.distortion_enabled   = False
        self.distortion_level     = 0.3
        self.distortion_character = 0.5

        self.reverb_enabled    = False
        self.reverb_roomsize_m = 30.0
        self.reverb_time_s     = 2.0
        self.reverb_damping    = 0.5
        self.reverb_bandwidth  = 0.75
        self.reverb_dry_db     = -3.0
        self.reverb_early_db   = -9.0
        self.reverb_tail_db    = -12.0

        self.rvc_model        = ''
        self.rvc_pitch_offset = 0.0
        self.rvc_hubert_model = 'torchaudio'
        self.rvc_vtln_alpha = 1.0

    def _to_dict(self) -> dict:
        return {
            'enabled':   self.enabled,
            'mode':      self.mode,
            'source_id': self.source_id,
            'pitch': {
                'enabled':   self.pitch_enabled,
                'semitones': self.pitch_semitones,
            },
            'chorus': {
                'enabled':    self.chorus_enabled,
                'voices':     self.chorus_voices,
                'delay_ms':   self.chorus_delay_ms,
                'sep_ms':     self.chorus_sep_ms,
                'detune_pct': self.chorus_detune_pct,
                'lfo_hz':     self.chorus_lfo_hz,
                'atten_db':   self.chorus_atten_db,
            },
            'delay': {
                'enabled': self.delay_enabled,
                'delay_s': self.delay_s,
            },
            'distortion': {
                'enabled':   self.distortion_enabled,
                'level':     self.distortion_level,
                'character': self.distortion_character,
            },
            'reverb': {
                'enabled':    self.reverb_enabled,
                'roomsize_m': self.reverb_roomsize_m,
                'time_s':     self.reverb_time_s,
                'damping':    self.reverb_damping,
                'bandwidth':  self.reverb_bandwidth,
                'dry_db':     self.reverb_dry_db,
                'early_db':   self.reverb_early_db,
                'tail_db':    self.reverb_tail_db,
            },
            'rvc': {
                'model':        self.rvc_model,
                'pitch_offset': self.rvc_pitch_offset,
                'hubert_model': self.rvc_hubert_model,
                'vtln_alpha': self.rvc_vtln_alpha,
            },
        }

    def save(self) -> None:
        VC_SETTINGS_FILE.parent.mkdir(parents=True, exist_ok=True)
        yaml = YAML()
        yaml.dump(self._to_dict(), VC_SETTINGS_FILE.open('w'))

    @classmethod
    def load(cls) -> 'VCSettings':
        s = cls()
        if not VC_SETTINGS_FILE.exists():
            return s
        try:
            data = YAML(typ='safe').load(VC_SETTINGS_FILE)
            if not data:
                return s
            s.enabled   = bool(data.get('enabled', False))
            s.mode      = str(data.get('mode', 'ladspa'))
            s.source_id = str(data.get('source_id', ''))

            p = data.get('pitch', {})
            s.pitch_enabled   = bool(p.get('enabled', False))
            s.pitch_semitones = float(p.get('semitones', 0.0))

            c = data.get('chorus', {})
            s.chorus_enabled    = bool(c.get('enabled', False))
            s.chorus_voices     = int(c.get('voices', 3))
            s.chorus_delay_ms   = float(c.get('delay_ms', 20.0))
            s.chorus_sep_ms     = float(c.get('sep_ms', 0.5))
            s.chorus_detune_pct = float(c.get('detune_pct', 1.0))
            s.chorus_lfo_hz     = float(c.get('lfo_hz', 4.0))
            s.chorus_atten_db   = float(c.get('atten_db', -3.0))

            d = data.get('delay', {})
            s.delay_enabled = bool(d.get('enabled', False))
            s.delay_s       = float(d.get('delay_s', 0.3))

            dist = data.get('distortion', {})
            s.distortion_enabled   = bool(dist.get('enabled', False))
            s.distortion_level     = float(dist.get('level', 0.3))
            s.distortion_character = float(dist.get('character', 0.5))

            r = data.get('reverb', {})
            s.reverb_enabled    = bool(r.get('enabled', False))
            s.reverb_roomsize_m = float(r.get('roomsize_m', 30.0))
            s.reverb_time_s     = float(r.get('time_s', 2.0))
            s.reverb_damping    = float(r.get('damping', 0.5))
            s.reverb_bandwidth  = float(r.get('bandwidth', 0.75))
            s.reverb_dry_db     = float(r.get('dry_db', -3.0))
            s.reverb_early_db   = float(r.get('early_db', -9.0))
            s.reverb_tail_db    = float(r.get('tail_db', -12.0))

            rv = data.get('rvc', {})
            s.rvc_model        = str(rv.get('model', ''))
            s.rvc_pitch_offset = float(rv.get('pitch_offset', 0.0))
            s.rvc_hubert_model = str(rv.get('hubert_model', 'torchaudio'))
            s.rvc_vtln_alpha = float(rv.get('vtln_alpha', 1.0))

            _log.debug('Loaded VC settings: mode=%s enabled=%s', s.mode, s.enabled)
        except Exception as e:
            _log.error('Failed to parse VC settings: %s', e)
        return s

    def to_ladspa_config(self) -> 'LADSPAChainConfig':
        from linux_arctis_manager.voice_changer.ladspa.chain import LADSPAChainConfig
        from linux_arctis_manager.voice_changer.ladspa.effects import (
            ChorusEffect, DelayEffect, DistortionEffect, PitchEffect, ReverbEffect,
        )
        cfg = LADSPAChainConfig()
        cfg.source_id = self.source_id
        cfg.pitch = PitchEffect(
            enabled=self.pitch_enabled,
            semitones=self.pitch_semitones,
        )
        cfg.chorus = ChorusEffect(
            enabled=self.chorus_enabled,
            voices=self.chorus_voices,
            delay_ms=self.chorus_delay_ms,
            sep_ms=self.chorus_sep_ms,
            detune_pct=self.chorus_detune_pct,
            lfo_hz=self.chorus_lfo_hz,
            atten_db=self.chorus_atten_db,
        )
        cfg.delay = DelayEffect(
            enabled=self.delay_enabled,
            delay_s=self.delay_s,
        )
        cfg.distortion = DistortionEffect(
            enabled=self.distortion_enabled,
            level=self.distortion_level,
            character=self.distortion_character,
        )
        cfg.reverb = ReverbEffect(
            enabled=self.reverb_enabled,
            roomsize_m=self.reverb_roomsize_m,
            time_s=self.reverb_time_s,
            damping=self.reverb_damping,
            bandwidth=self.reverb_bandwidth,
            dry_db=self.reverb_dry_db,
            early_db=self.reverb_early_db,
            tail_db=self.reverb_tail_db,
        )
        return cfg
