from __future__ import annotations

import contextlib
import math
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

from ruamel.yaml import YAML

from linux_arctis_manager.constants import EQ_PRESETS_FOLDER

EQMode = Literal['simple', 'advanced']

MBEQ_BAND_FREQUENCIES = [50, 100, 156, 220, 311, 440, 622, 880, 1250, 1750, 2500, 3500, 5000, 10000, 20000]
SIMPLE_BAND_INDICES = [0, 1, 2, 3, 5, 7, 9, 11, 13, 14]
SIMPLE_BAND_FREQUENCIES = [MBEQ_BAND_FREQUENCIES[i] for i in SIMPLE_BAND_INDICES]


@dataclass
class EQBand:
    frequency: int
    gain: float = 0.0


@dataclass
class EQPreset:
    name: str
    mode: EQMode = 'simple'
    description: str = ''
    bands: list[EQBand] = field(default_factory=list)
    builtin: bool = field(default=False, compare=False, repr=False)

    def __post_init__(self) -> None:
        if not self.bands:
            freqs = SIMPLE_BAND_FREQUENCIES if self.mode == 'simple' else MBEQ_BAND_FREQUENCIES
            self.bands = [EQBand(frequency=f) for f in freqs]

    def to_ladspa_controls(self) -> list[float]:
        """Returns 15 gain values for mbeq_1197, mapping from self.bands."""
        result = [0.0] * 15
        if self.mode == 'simple':
            for ui_idx, mbeq_idx in enumerate(SIMPLE_BAND_INDICES):
                if ui_idx < len(self.bands):
                    result[mbeq_idx] = float(self.bands[ui_idx].gain)
        else:
            for i, band in enumerate(self.bands[:15]):
                result[i] = float(band.gain)
        return result

    def save(self, path: Path | None = None) -> Path:
        EQ_PRESETS_FOLDER.mkdir(parents=True, exist_ok=True)
        if path is None:
            slug = self.name.lower().replace(' ', '_')
            path = EQ_PRESETS_FOLDER / f'{slug}.yaml'
        yaml = YAML()
        yaml.default_flow_style = False
        data = {
            'name': self.name,
            'mode': self.mode,
            'description': self.description,
            'bands': [{'frequency': b.frequency, 'gain': float(b.gain)} for b in self.bands],
        }
        with open(path, 'w') as f:
            yaml.dump(data, f)
        return path

    @classmethod
    def load(cls, path: Path) -> EQPreset:
        yaml = YAML(typ='safe')
        data = yaml.load(path)
        bands = [EQBand(frequency=b['frequency'], gain=float(b['gain'])) for b in data.get('bands', [])]
        return cls(
            name=data['name'],
            mode=data.get('mode', 'simple'),
            description=data.get('description', ''),
            bands=bands,
        )

    @classmethod
    def flat(cls, mode: EQMode = 'simple', name: str = 'Flat') -> EQPreset:
        return cls(name=name, mode=mode)


def _b(name: str, gains: list[float], description: str = '') -> EQPreset:
    """Shorthand for defining a builtin simple-mode preset."""
    bands = [EQBand(frequency=f, gain=g) for f, g in zip(SIMPLE_BAND_FREQUENCIES, gains, strict=True)]
    return EQPreset(name=name, mode='simple', description=description, bands=bands, builtin=True)


BUILTIN_PRESETS: list[EQPreset] = [
    _b('Rock',         [4.0,  3.0,  2.0,  1.0, -1.0, -1.0,  0.0,  2.0,  3.0,  4.0]),
    _b('Pop',          [2.0,  2.5,  1.5,  0.5,  0.0,  0.5,  2.0,  2.5,  2.0,  1.5]),
    _b('Classical',    [0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  2.0,  3.5]),
    _b('Jazz',         [2.0,  3.0,  2.0,  1.0,  0.0,  0.5,  1.0,  1.5,  2.0,  1.0]),
    _b('Bass Boost',   [7.0,  6.0,  5.0,  3.0,  1.0,  0.0,  0.0,  0.0,  0.0,  0.0]),
    _b('Treble Boost', [0.0,  0.0,  0.0,  0.0,  0.0,  1.0,  2.5,  4.0,  5.5,  6.0]),
    _b('Vocal Boost',  [-2.0, -1.5, -0.5,  1.0,  2.5,  3.5,  3.5,  2.5,  1.0,  0.0]),
    _b('Gaming',       [-2.0, -1.5,  0.0,  1.5,  2.0,  2.5,  2.5,  2.0,  1.0,  0.0]),
]


_SIMPLE_IDX_SET = set(SIMPLE_BAND_INDICES)
_MISSING_INDICES = [i for i in range(15) if i not in _SIMPLE_IDX_SET]


def elevate_bands(bands_10: list[EQBand]) -> list[EQBand]:
    """Expand 10 simple bands → 15 advanced bands, interpolating 5 missing gains on a log-freq scale."""
    known: dict[int, float] = {
        mbeq_idx: bands_10[ui_idx].gain
        for ui_idx, mbeq_idx in enumerate(SIMPLE_BAND_INDICES)
        if ui_idx < len(bands_10)
    }
    result: list[EQBand] = []
    for mbeq_idx, freq in enumerate(MBEQ_BAND_FREQUENCIES):
        if mbeq_idx in known:
            gain = known[mbeq_idx]
        else:
            lo = max(i for i in known if i < mbeq_idx)
            hi = min(i for i in known if i > mbeq_idx)
            log_lo  = math.log10(MBEQ_BAND_FREQUENCIES[lo])
            log_hi  = math.log10(MBEQ_BAND_FREQUENCIES[hi])
            log_tgt = math.log10(freq)
            t = (log_tgt - log_lo) / (log_hi - log_lo)
            gain = round(known[lo] + t * (known[hi] - known[lo]), 2)
        result.append(EQBand(frequency=freq, gain=gain))
    return result


def downsample_bands(bands_15: list[EQBand]) -> list[EQBand]:
    """
    Downsample 15 advanced bands to 10 simple bands.
    Each simple band starts from its exact advanced value; each of the 5 extra
    bands redistributes its gain to its two neighboring simple bands using
    log-frequency weighting, so no advanced band value is silently discarded.
    """
    freq_to_gain = {b.frequency: b.gain for b in bands_15}
    simple_freqs = [MBEQ_BAND_FREQUENCIES[i] for i in SIMPLE_BAND_INDICES]
    result_gains: dict[int, float] = {f: freq_to_gain.get(f, 0.0) for f in simple_freqs}

    for mbeq_idx, freq in enumerate(MBEQ_BAND_FREQUENCIES):
        if mbeq_idx in _SIMPLE_IDX_SET:
            continue
        extra_gain = freq_to_gain.get(freq, 0.0)
        if extra_gain == 0.0:
            continue
        lo_idx = max(i for i in SIMPLE_BAND_INDICES if i < mbeq_idx)
        hi_idx = min(i for i in SIMPLE_BAND_INDICES if i > mbeq_idx)
        lo_freq = MBEQ_BAND_FREQUENCIES[lo_idx]
        hi_freq = MBEQ_BAND_FREQUENCIES[hi_idx]
        t = (math.log10(freq) - math.log10(lo_freq)) / (math.log10(hi_freq) - math.log10(lo_freq))
        result_gains[lo_freq] += extra_gain * (1 - t)
        result_gains[hi_freq] += extra_gain * t

    return [EQBand(frequency=f, gain=round(result_gains[f], 2)) for f in simple_freqs]


def list_presets() -> list[EQPreset]:
    result: list[EQPreset] = list(BUILTIN_PRESETS)
    if EQ_PRESETS_FOLDER.exists():
        for f in sorted(EQ_PRESETS_FOLDER.glob('*.yaml')):
            with contextlib.suppress(Exception):
                result.append(EQPreset.load(f))
    return result
