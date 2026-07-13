"""
LADSPA effect definitions for the voice changer chain.

Plugin sources (all from swh-plugins / ladspa-swh-plugins):
  pitch_scale_1193  – Pitch Scaler         (1 ctrl: pitch_coeff [0.5,2])
  am_pitchshift_1433 – AM Pitch Shifter    (2 ctrl: pitch_shift [0.25,4], buffer_size [1,7])
  multivoice_chorus_1201 – Multivoice Chorus (6 ctrl)
  delay_1898        – Simple delay          (2 ctrl: max_delay, delay_time)
  valve_1209        – Valve saturation      (2 ctrl: level [0,1], character [0,1])
  gverb_1216        – GVerb reverb          (7 ctrl) — stereo out, must be last in chain
"""
from __future__ import annotations

import math
import os
from dataclasses import dataclass, field
from typing import ClassVar

_LADSPA_SEARCH_PATHS = [
    '/usr/lib/ladspa',
    '/usr/lib64/ladspa',
    '/usr/local/lib/ladspa',
    '/usr/local/lib64/ladspa',
    os.path.expanduser('~/.ladspa'),
]


def _plugin_available(plugin: str) -> bool:
    paths: list[str] = []
    env = os.environ.get('LADSPA_PATH', '')
    if env:
        paths.extend(p for p in env.split(':') if p)
    paths.extend(_LADSPA_SEARCH_PATHS)
    return any(os.path.isfile(os.path.join(d, f'{plugin}.so')) for d in paths)


def _find_plugin(candidates: list[tuple[str, str]]) -> tuple[str, str] | None:
    return next(((p, l) for p, l in candidates if _plugin_available(p)), None)


# ── Pitch ────────────────────────────────────────────────────────────────────
# am_pitchshift preferred (better quality via AM algorithm)
# Controls: pitch_shift [0.25, 4], buffer_size [1, 7]
# buffer_size is a quality hint (higher = more latency but better quality)
_PITCH_CANDIDATES: list[tuple[str, str]] = [
    ('am_pitchshift_1433',  'amPitchshift'),
    ('pitch_scale_1193',    'pitchScale'),
]

@dataclass
class PitchEffect:
    enabled:   bool  = False
    semitones: float = 0.0   # -24..+24

    CANDIDATES: ClassVar[list[tuple[str, str]]] = _PITCH_CANDIDATES

    @staticmethod
    def available() -> bool:
        return _find_plugin(_PITCH_CANDIDATES) is not None

    def factor(self) -> float:
        return math.pow(2.0, self.semitones / 12.0)

    def ladspa_controls(self, plugin: str) -> str:
        f = self.factor()
        if 'am_pitchshift' in plugin:
            return f'{f:.6f},4'   # pitch_shift, buffer_size=4
        return f'{f:.6f}'          # pitch_scale: 1 control

    def build_module_args(self, name: str, master: str) -> str | None:
        hit = _find_plugin(_PITCH_CANDIDATES)
        if hit is None:
            return None
        plugin, label = hit
        ctrl = self.ladspa_controls(plugin)
        return f'source_name={name} master={master} plugin={plugin} label={label} control={ctrl}'


# ── Chorus ───────────────────────────────────────────────────────────────────
# multivoice_chorus_1201 — 6 controls:
#   voices [1,8], delay_base_ms [10,40], voice_sep_ms [0,2],
#   detune_pct [0,5], lfo_hz [2,30], output_atten_db [-20,0]
_CHORUS_CANDIDATES: list[tuple[str, str]] = [
    ('multivoice_chorus_1201', 'multivoiceChorus'),
]

@dataclass
class ChorusEffect:
    enabled:    bool  = False
    voices:     int   = 3       # 1-8
    delay_ms:   float = 20.0    # 10-40 ms
    sep_ms:     float = 0.5     # 0-2 ms
    detune_pct: float = 1.0     # 0-5 %
    lfo_hz:     float = 4.0     # 2-30 Hz
    atten_db:   float = -3.0    # -20-0 dB

    CANDIDATES: ClassVar[list[tuple[str, str]]] = _CHORUS_CANDIDATES

    @staticmethod
    def available() -> bool:
        return _find_plugin(_CHORUS_CANDIDATES) is not None

    def ladspa_controls(self) -> str:
        return f'{self.voices},{self.delay_ms:.1f},{self.sep_ms:.2f},{self.detune_pct:.2f},{self.lfo_hz:.1f},{self.atten_db:.1f}'

    def build_module_args(self, name: str, master: str) -> str | None:
        hit = _find_plugin(_CHORUS_CANDIDATES)
        if hit is None:
            return None
        plugin, label = hit
        return f'source_name={name} master={master} plugin={plugin} label={label} control={self.ladspa_controls()}'


# ── Delay ────────────────────────────────────────────────────────────────────
# delay_1898 — 2 controls:
#   max_delay_s [0,∞], delay_time_s [0,∞]
# max_delay sets buffer at load time; must be ≥ delay_time
_DELAY_CANDIDATES: list[tuple[str, str]] = [
    ('delay_1898', 'delay_n'),
]

@dataclass
class DelayEffect:
    enabled:    bool  = False
    delay_s:    float = 0.3     # 0-5 s

    CANDIDATES: ClassVar[list[tuple[str, str]]] = _DELAY_CANDIDATES

    @staticmethod
    def available() -> bool:
        return _find_plugin(_DELAY_CANDIDATES) is not None

    def ladspa_controls(self) -> str:
        max_s = self.delay_s + 0.5   # headroom
        return f'{max_s:.2f},{self.delay_s:.2f}'

    def build_module_args(self, name: str, master: str) -> str | None:
        hit = _find_plugin(_DELAY_CANDIDATES)
        if hit is None:
            return None
        plugin, label = hit
        return f'source_name={name} master={master} plugin={plugin} label={label} control={self.ladspa_controls()}'


# ── Distortion ───────────────────────────────────────────────────────────────
# valve_1209 — 2 controls:
#   distortion_level [0,1], distortion_character [0,1]
# character: 0 = even harmonics (warm), 1 = odd harmonics (harsh/robotic)
_DISTORTION_CANDIDATES: list[tuple[str, str]] = [
    ('valve_1209', 'valve'),
]

@dataclass
class DistortionEffect:
    enabled:   bool  = False
    level:     float = 0.3    # 0-1
    character: float = 0.5    # 0-1

    CANDIDATES: ClassVar[list[tuple[str, str]]] = _DISTORTION_CANDIDATES

    @staticmethod
    def available() -> bool:
        return _find_plugin(_DISTORTION_CANDIDATES) is not None

    def ladspa_controls(self) -> str:
        return f'{self.level:.2f},{self.character:.2f}'

    def build_module_args(self, name: str, master: str) -> str | None:
        hit = _find_plugin(_DISTORTION_CANDIDATES)
        if hit is None:
            return None
        plugin, label = hit
        return f'source_name={name} master={master} plugin={plugin} label={label} control={self.ladspa_controls()}'


# ── Reverb ───────────────────────────────────────────────────────────────────
# gverb_1216 — 7 controls:
#   roomsize_m [1,300], reverb_time_s [0.1,30], damping [0,1],
#   input_bandwidth [0,1], dry_db [-70,0], early_db [-70,0], tail_db [-70,0]
# NOTE: gverb has stereo output → creates a stereo virtual source.
# Place it last in the chain so the loopback can mix down to mono.
_REVERB_CANDIDATES: list[tuple[str, str]] = [
    ('gverb_1216', 'gverb'),
]

@dataclass
class ReverbEffect:
    enabled:    bool  = False
    roomsize_m: float = 30.0    # 1-300 m
    time_s:     float = 2.0     # 0.1-30 s
    damping:    float = 0.5     # 0-1
    bandwidth:  float = 0.75    # 0-1
    dry_db:     float = -3.0    # -70-0 dB
    early_db:   float = -9.0    # -70-0 dB
    tail_db:    float = -12.0   # -70-0 dB

    CANDIDATES: ClassVar[list[tuple[str, str]]] = _REVERB_CANDIDATES

    @staticmethod
    def available() -> bool:
        return _find_plugin(_REVERB_CANDIDATES) is not None

    def ladspa_controls(self) -> str:
        return (f'{self.roomsize_m:.1f},{self.time_s:.2f},{self.damping:.2f},'
                f'{self.bandwidth:.2f},{self.dry_db:.1f},{self.early_db:.1f},{self.tail_db:.1f}')

    def build_module_args(self, name: str, master: str) -> str | None:
        hit = _find_plugin(_REVERB_CANDIDATES)
        if hit is None:
            return None
        plugin, label = hit
        return f'source_name={name} master={master} plugin={plugin} label={label} control={self.ladspa_controls()}'


def capabilities() -> dict[str, bool]:
    return {
        'pitch':      PitchEffect.available(),
        'chorus':     ChorusEffect.available(),
        'delay':      DelayEffect.available(),
        'distortion': DistortionEffect.available(),
        'reverb':     ReverbEffect.available(),
    }
