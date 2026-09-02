from enum import Enum
from typing import ClassVar

import pytest

from linux_arctis_manager.utils import (
    JsonSerializable,
    ObservableDict,
    compare_versions,
    project_version,
)

# --- project_version ---

def test_project_version_reads_installed_package_metadata():
    # The package is installed (editable) in this dev venv, so the
    # importlib.metadata lookup succeeds without needing the VERSION-file fallback.
    assert project_version() != ''


def test_project_version_falls_back_to_version_file(monkeypatch):
    import importlib.metadata as metadata_module

    def raise_not_found(_name):
        raise metadata_module.PackageNotFoundError

    monkeypatch.setattr('linux_arctis_manager.utils.version', raise_not_found)
    result = project_version()
    assert result != ''


def test_project_version_returns_dev_when_nothing_available(monkeypatch, tmp_path):
    import importlib.metadata as metadata_module

    def raise_not_found(_name):
        raise metadata_module.PackageNotFoundError

    def raise_os_error(_self):
        raise OSError

    monkeypatch.setattr('linux_arctis_manager.utils.version', raise_not_found)
    monkeypatch.setattr('pathlib.Path.read_text', raise_os_error)
    assert project_version() == 'dev'


# --- compare_versions ---

def test_compare_versions_equal():
    assert compare_versions('1.2.3', '1.2.3') == 0


def test_compare_versions_less_than():
    assert compare_versions('1.2.3', '1.2.4') == -1


def test_compare_versions_greater_than():
    assert compare_versions('2.0.0', '1.9.9') == 1


def test_compare_versions_pep440_dev_equivalence():
    assert compare_versions('2.5.1-dev', '2.5.1.dev0') == 0


def test_compare_versions_string_fallback_on_unparsable_input():
    assert compare_versions('not-a-version-a', 'not-a-version-b') == -1
    assert compare_versions('same', 'same') == 0
    assert compare_versions('zzz', 'aaa') == 1


# --- JsonSerializable ---

class Color(Enum):
    RED = 'red'


class Thing(JsonSerializable):
    name: str
    color: Color
    _js_exclude_fields: ClassVar[list[str]] = ['hidden']

    def __init__(self, name: str, color: Color, hidden: str = 'secret'):
        self.name = name
        self.color = color
        self.hidden = hidden

    def a_method(self) -> str:
        return 'not data'


def test_to_dict_serializes_plain_fields():
    t = Thing(name='widget', color=Color.RED)
    assert t.to_dict() == {'name': 'widget', 'color': 'red'}


def test_to_dict_excludes_configured_fields():
    t = Thing(name='widget', color=Color.RED, hidden='secret-value')
    result = t.to_dict()
    assert 'hidden' not in result


def test_to_dict_excludes_callables():
    t = Thing(name='widget', color=Color.RED)
    assert 'a_method' not in t.to_dict()


def test_to_dict_recurses_into_nested_json_serializable():
    class Outer(JsonSerializable):
        inner: Thing

        def __init__(self, inner):
            self.inner = inner

    outer = Outer(Thing(name='n', color=Color.RED))
    assert outer.to_dict() == {'inner': {'name': 'n', 'color': 'red'}}


def test_to_dict_serializes_lists_of_json_serializable():
    class Holder(JsonSerializable):
        items: list

        def __init__(self, items):
            self.items = items

    holder = Holder([Thing(name='a', color=Color.RED), Thing(name='b', color=Color.RED)])
    assert holder.to_dict() == {'items': [{'name': 'a', 'color': 'red'}, {'name': 'b', 'color': 'red'}]}


def test_to_dict_handles_dict_subclass():
    class DictThing(dict, JsonSerializable):
        pass

    d = DictThing(a=1, b=Color.RED)
    assert d.to_dict() == {'a': 1, 'b': 'red'}


# --- ObservableDict ---

def test_observable_dict_notifies_on_change():
    calls = []
    d: ObservableDict[str, int] = ObservableDict()
    d.add_observer(lambda k, v: calls.append((k, v)))
    d['a'] = 1
    assert calls == [('a', 1)]


def test_observable_dict_does_not_notify_when_value_unchanged():
    calls = []
    d: ObservableDict[str, int] = ObservableDict(a=1)
    d.add_observer(lambda k, v: calls.append((k, v)))
    d['a'] = 1
    assert calls == []


def test_observable_dict_update_with_mapping_notifies_each_key():
    calls = []
    d: ObservableDict[str, int] = ObservableDict()
    d.add_observer(lambda k, v: calls.append((k, v)))
    d.update({'a': 1, 'b': 2})
    assert set(calls) == {('a', 1), ('b', 2)}


def test_observable_dict_update_with_kwargs_notifies():
    calls = []
    d: ObservableDict[str, int] = ObservableDict()
    d.add_observer(lambda k, v: calls.append((k, v)))
    d.update(c=3)
    assert calls == [('c', 3)]


def test_observable_dict_update_rejects_multiple_positional_args():
    d: ObservableDict[str, int] = ObservableDict()
    with pytest.raises(TypeError, match='expected exactly 1 argument'):
        d.update({'a': 1}, {'b': 2})


def test_observable_dict_to_dict_excludes_observers():
    d: ObservableDict[str, int] = ObservableDict(a=1)
    d.add_observer(lambda k, v: None)
    assert d.to_dict() == {'a': 1}
