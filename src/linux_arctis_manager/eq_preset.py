from __future__ import annotations

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


def list_presets() -> list[EQPreset]:
    if not EQ_PRESETS_FOLDER.exists():
        return []
    presets = []
    for f in sorted(EQ_PRESETS_FOLDER.glob('*.yaml')):
        try:
            presets.append(EQPreset.load(f))
        except Exception:
            pass
    return presets
