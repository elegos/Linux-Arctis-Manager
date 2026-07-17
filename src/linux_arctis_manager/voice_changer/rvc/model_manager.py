from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path

logger = logging.getLogger('RVCModelManager')

RVC_MODELS_FOLDER = Path.home() / '.config' / 'arctis_manager' / 'rvc_models'


@dataclass
class RVCModel:
    name: str          # display name (stem of the .pth file)
    path: Path         # absolute path to the .pth file
    has_index: bool = False   # a FAISS .index file sits next to the model

    @staticmethod
    def index_exists(pth: Path) -> bool:
        """Same matching rule as the pipeline: '<stem>.index' or any
        '*.index' whose filename contains the model stem."""
        if (pth.parent / f'{pth.stem}.index').is_file():
            return True
        return any(pth.stem in p.stem for p in pth.parent.glob('*.index'))


class RVCModelManager:
    """Scans RVC_MODELS_FOLDER for .pth model files."""

    @staticmethod
    def models_folder() -> Path:
        return RVC_MODELS_FOLDER

    @staticmethod
    def list_models() -> list[RVCModel]:
        folder = RVC_MODELS_FOLDER
        if not folder.exists():
            return []
        models = [
            RVCModel(name=p.stem, path=p, has_index=RVCModel.index_exists(p))
            for p in sorted(folder.iterdir())
            if p.suffix == '.pth' and p.is_file()
        ]
        logger.debug('RVC models found: %d', len(models))
        return models

    @staticmethod
    def find_model(name: str) -> RVCModel | None:
        return next((m for m in RVCModelManager.list_models() if m.name == name), None)
