from linux_arctis_manager.i18n import I18n


def test_get_instance_returns_singleton(monkeypatch):
    monkeypatch.setattr(I18n, '_instance', None, raising=False)
    a = I18n.get_instance()
    b = I18n.get_instance()
    assert a is b


def test_translate_returns_known_key():
    I18n.get_instance().set_language('en')
    assert I18n.translate('status', 'sidetone') == 'Sidetone'


def test_translate_falls_back_to_key_when_missing():
    I18n.get_instance().set_language('en')
    assert I18n.translate('status', 'totally_unknown_key') == 'totally_unknown_key'


def test_translate_strips_trailing_comment():
    I18n.get_instance().set_language('en')
    assert I18n.translate('status', 'headset') == 'Headset'


def test_translate_unescapes_newlines():
    instance = I18n.get_instance()
    instance.translations.read_dict({'test_section': {'multiline': 'line1\\nline2'}})
    assert I18n.translate('test_section', 'multiline') == 'line1\nline2'


def test_set_language_falls_back_to_default_when_lang_missing(caplog):
    I18n.get_instance().set_language('nonexistent_lang_xx')
    assert 'not found' in caplog.text


def test_set_language_prefers_home_override(tmp_path, monkeypatch):
    monkeypatch.setattr('linux_arctis_manager.i18n.HOME_LANG_FOLDER', tmp_path)
    lang_file = tmp_path / 'fr.ini'
    lang_file.write_text('[status]\nsidetone = Tonalite laterale\n')

    instance = I18n.get_instance()
    instance.set_language('fr')
    assert I18n.translate('status', 'sidetone') == 'Tonalite laterale'

    # Restore English for any tests that run after this one.
    instance.set_language('en')
