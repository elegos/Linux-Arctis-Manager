from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal

MatcherType = Literal['stream', 'executable', 'steam']


@dataclass
class AppMatcher:
    type: MatcherType
    value: str = ''           # stream name or executable binary
    app_id: int | None = None  # Steam app ID
    name: str = ''            # Steam game display name

    def matches(self, props: dict[str, Any]) -> bool:
        if self.type == 'stream':
            return props.get('application.name', '') == self.value
        if self.type == 'executable':
            return props.get('application.process.binary', '') == self.value
        if self.type == 'steam':
            # Primary: SteamGameId env var surfaced in stream props
            env_game_id = props.get('env.SteamGameId') or props.get('application.process.env.SteamGameId')
            if env_game_id is not None:
                return str(env_game_id) == str(self.app_id)
            # Fallback: compare binary against known Steam game executables
            binary = props.get('application.process.binary', '')
            if binary and self.app_id is not None:
                from linux_arctis_manager.steam_library import get_game_executables
                return binary in get_game_executables(self.app_id)
        return False

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {'type': self.type}
        if self.type in ('stream', 'executable'):
            d['value'] = self.value
        elif self.type == 'steam':
            d['app_id'] = self.app_id
            d['name'] = self.name
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AppMatcher:
        return cls(
            type=data['type'],
            value=data.get('value', ''),
            app_id=data.get('app_id'),
            name=data.get('name', ''),
        )


@dataclass
class AppEQOverride:
    matcher: AppMatcher
    preset_name: str
    channel: str = 'media'   # 'media' or 'chat'

    def to_dict(self) -> dict[str, Any]:
        return {
            'matcher': self.matcher.to_dict(),
            'preset': self.preset_name,
            'channel': self.channel,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AppEQOverride:
        return cls(
            matcher=AppMatcher.from_dict(data['matcher']),
            preset_name=data['preset'],
            channel=data.get('channel', 'media'),
        )
