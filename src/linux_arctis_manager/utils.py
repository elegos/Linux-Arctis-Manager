from abc import ABC
from enum import Enum
from typing import Any


class JsonSerializable(ABC):
    _js_exclude_fields: list[str] = []

    def to_dict(self) -> dict[str, Any]:
        def serialize(value: Any) -> Any:
            if isinstance(value, JsonSerializable):
                return value.to_dict()
            if isinstance(value, list):
                return [serialize(item) for item in value]

            if isinstance(value, Enum):
                return value.value

            return value
        
        cls = type(self)
        fields = getattr(cls, '__annotations__', {}).keys()

        return { field: serialize(getattr(self, field)) for field in fields if field not in [*self._js_exclude_fields, '_js_exclude_fields'] }
