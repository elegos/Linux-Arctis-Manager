from unittest.mock import MagicMock, call, patch

import pytest

from linux_arctis_manager.mic_router import ARCTIS_MIC_NAME, MicRouter


def _make_source(name: str, module: int) -> MagicMock:
    src = MagicMock()
    src.name = name
    src.owner_module = module
    return src


class TestMicRouterNoDuplicates:
    """MicRouter must not create a second Arctis_Manager_Mic if one already exists."""

    def _make_router_with_existing(self, existing_module_idx: int = 42):
        """Return a MicRouter whose pulse connection reports an existing source."""
        router = MicRouter()

        pulse = MagicMock()
        existing = _make_source(ARCTIS_MIC_NAME, existing_module_idx)
        pulse.source_list.return_value = [existing]

        router._pulse = pulse
        return router, pulse

    def test_load_skips_create_when_source_already_exists(self):
        """If Arctis_Manager_Mic already exists, _load() must reuse it, not load a new module."""
        router, pulse = self._make_router_with_existing(existing_module_idx=42)

        result = router.update('some_master')

        assert result is True
        pulse.module_load.assert_not_called()
        assert router._module == 42
        assert router._current_master == 'some_master'

    def test_load_creates_when_source_absent(self):
        """If Arctis_Manager_Mic is not present, _load() must create a new module."""
        router = MicRouter()

        pulse = MagicMock()
        pulse.source_list.return_value = []  # no existing source
        pulse.module_load.return_value = 99
        router._pulse = pulse

        with patch('subprocess.run'):
            result = router.update('some_master')

        assert result is True
        pulse.module_load.assert_called_once()
        assert router._module == 99

    def test_update_same_master_does_not_reload(self):
        """Calling update() with the same master twice must not reload the module."""
        router = MicRouter()
        router._module = 7
        router._current_master = 'same_master'

        pulse = MagicMock()
        router._pulse = pulse

        result = router.update('same_master')

        assert result is True
        pulse.module_load.assert_not_called()
        pulse.module_unload.assert_not_called()

    def test_unload_uses_discovered_module_index(self):
        """teardown() must unload the module index recovered from the existing source."""
        router, pulse = self._make_router_with_existing(existing_module_idx=55)
        router.update('some_master')

        router.teardown()

        pulse.module_unload.assert_called_once_with(55)
