import logging
from configparser import ConfigParser
from pathlib import Path

from linux_arctis_manager.constants import HOME_LANG_FOLDER


class I18n:
    _instance: 'I18n'
    translations: ConfigParser

    @staticmethod
    def get_instance() -> 'I18n':
        if getattr(I18n, '_instance', None) is None:
            I18n._instance = I18n()

        return I18n._instance

    def __init__(self):
        self.translations = ConfigParser()
        self.set_language('en')

    def set_language(self, lang_code: str, default: str = 'en') -> None:
        home_path = HOME_LANG_FOLDER / f'{lang_code or default}.ini'
        sys_path = Path(__file__).parent / 'lang' / f'{lang_code or default}.ini'

        lang_file = home_path if home_path.is_file() else sys_path
        logger = logging.getLogger('I18n')
        if not lang_file.exists():
            lang_file = Path(__file__).parent / 'lang' / f'{default}.ini'
            logger.warning(f'Language file {lang_file} not found, falling back to {lang_file}')
        
        if not lang_file.exists():
            logger.critical(f'Language file {lang_file} not found')
            return
        
        self.translations.read(lang_file)

    @staticmethod
    def translate(section: str, key: str|int) -> str:
        return f'{I18n.get_instance().translations.get(section, f"{key}", fallback=key)}'.split('#')[0].strip()
