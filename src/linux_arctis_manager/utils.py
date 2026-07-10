from abc import ABC
from enum import Enum
from importlib.metadata import PackageNotFoundError, version
from typing import Any, Callable, ClassVar, Generic, TypeVar


def project_version() -> str:
    try:
        return version("linux-arctis-manager")  # metti il nome del package
    except PackageNotFoundError:
        return "dev"


def compare_versions(a: str, b: str) -> int:
    """Return -1 if a < b, 0 if a == b, 1 if a > b.

    Follows SemVer precedence: pre-release (dev/beta/rc/…) is older than the
    bare release with the same numeric parts.
    """
    import re

    def _parse(v: str) -> tuple:
        m = re.match(r'^(\d+)\.(\d+)\.(\d+)(?:[.\-](.+))?$', v.strip())
        if not m:
            return (0, 0, 0, v)
        return (int(m.group(1)), int(m.group(2)), int(m.group(3)), m.group(4) or '')

    pa, pb = _parse(a), _parse(b)
    if pa[:3] < pb[:3]:
        return -1
    if pa[:3] > pb[:3]:
        return 1
    # Same numeric version — pre-release < release
    a_pre, b_pre = pa[3], pb[3]
    if a_pre == b_pre:
        return 0
    if not a_pre:   # a is release, b is pre-release → a is newer
        return 1
    if not b_pre:   # b is release, a is pre-release → a is older
        return -1
    return (-1 if a_pre < b_pre else 1)


class JsonSerializable(ABC):
    _js_exclude_fields: ClassVar[list[str]] = []

    def to_dict(self) -> dict[str, Any]:
        def serialize(value: Any) -> Any:
            if isinstance(value, JsonSerializable):
                return value.to_dict()
            if isinstance(value, list):
                return [serialize(item) for item in value]

            if isinstance(value, Enum):
                return value.value

            return value
        
        if isinstance(self, dict):
            return { k: serialize(v) for k, v in self.items() }
        
        cls = type(self)
        fields = getattr(cls, '__annotations__', {}).keys()

        return { field: serialize(getattr(self, field)) for field in fields if type(getattr(self, field)) != callable and field not in [*self._js_exclude_fields, '_js_exclude_fields']}



K = TypeVar('K')
V = TypeVar('V')

class ObservableDict(dict[K, V], Generic[K, V], JsonSerializable):
    _js_exclude_fields: ClassVar[list[str]] = ['_observers']
    _observers: list[Callable[[K, V], None]]

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._observers = []

    def add_observer(self, observer: Callable[[K, V], None]):
        self._observers.append(observer)

    def __setitem__(self, key, value):
        old_value = self.get(key, None)

        super().__setitem__(key, value)
        if old_value != value:
            for observer in self._observers:
                observer(key, value)
    
    def update(self, *args, **kwargs):
        if args:
            if len(args) != 1:
                raise TypeError("update expected exactly 1 argument")
            other = dict(args[0])
            for k, v in other.items():
                self[k] = v

        for k, v in kwargs.items():
            self[k] = v
