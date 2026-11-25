import importlib
import typing

import linux_arctis_manager


def test_package_restores_what_dbus_next_imports_from_typing(monkeypatch):
    # Removed in Python 3.15; dbus-next 0.2.3 still imports it.
    monkeypatch.delattr(typing, 'no_type_check_decorator', raising=False)

    importlib.reload(linux_arctis_manager)

    def marker() -> None: ...

    assert typing.no_type_check_decorator(marker) is marker
