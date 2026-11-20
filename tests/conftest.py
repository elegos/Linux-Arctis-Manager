import pytest


@pytest.fixture(autouse=True)
def _never_escalate_privileges(monkeypatch):
    """No test may trigger a pkexec/sudo prompt (it would block CI and annoy a dev machine).

    `collect_debug_info.sh` honours this switch, and QProcess/subprocess children
    inherit it."""
    monkeypatch.setenv('LAM_NO_PRIVILEGE', '1')
