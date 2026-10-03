from unittest.mock import MagicMock

from linux_arctis_manager.eq_manager import ChannelEQConfig, EQConfig, EQManager
from linux_arctis_manager.eq_preset import EQPreset

PHYSICAL = {'media': 'hw.stereo-game', 'chat': 'hw.mono-chat'}


def _manager() -> EQManager:
    manager = EQManager()
    manager._create_ladspa_sink = MagicMock(return_value=True)
    manager.start_stream_monitor = MagicMock()
    manager.stop_stream_monitor = MagicMock()
    return manager


def _config(media: bool, chat: bool) -> EQConfig:
    return EQConfig(
        media=ChannelEQConfig(enabled=media, preset=EQPreset(name='m') if media else None),
        chat=ChannelEQConfig(enabled=chat, preset=EQPreset(name='c') if chat else None),
    )


def test_setup_without_eq_targets_each_channel_physical_sink():
    assert _manager().setup(PHYSICAL, _config(False, False)) == PHYSICAL


def test_setup_eq_sinks_use_their_channel_physical_sink_as_master():
    manager = _manager()
    targets = manager.setup(PHYSICAL, _config(True, True))

    masters = {c.args[0]: c.args[2] for c in manager._create_ladspa_sink.call_args_list}
    assert masters == PHYSICAL
    assert targets['media'].startswith('Arctis_Media') and targets['chat'].startswith('Arctis_Chat')


def test_setup_accepts_single_physical_sink():
    manager = _manager()
    manager.setup('hw.stereo', _config(True, True))

    assert {c.args[2] for c in manager._create_ladspa_sink.call_args_list} == {'hw.stereo'}


def test_reapply_enabling_chat_eq_uses_chat_physical_sink():
    manager = _manager()
    manager.setup(PHYSICAL, _config(False, False))

    targets, changed = manager.reapply(PHYSICAL, _config(False, True))

    assert manager._create_ladspa_sink.call_args.args[2] == 'hw.mono-chat'
    assert targets['media'] == 'hw.stereo-game'
    assert changed == {'chat'}
