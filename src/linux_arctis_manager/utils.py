from collections.abc import Callable
from enum import Enum
from importlib.metadata import PackageNotFoundError, version
from typing import Any, ClassVar, Generic, TypeVar


def project_version() -> str:
    try:
        return version("linux-arctis-manager")
    except PackageNotFoundError:
        pass
    # Running from source: read the shared VERSION file at the repo root.
    try:
        import pathlib
        v = (pathlib.Path(__file__).parents[2] / "VERSION").read_text().strip()
        if v:
            return v
    except OSError:
        pass
    return "dev"


def compare_versions(a: str, b: str) -> int:
    """Return -1 if a < b, 0 if a == b, 1 if a > b.

    Uses packaging.version.Version so that PEP 440 equivalent strings such as
    "2.5.1-dev" and "2.5.1.dev0" compare as equal.
    """
    try:
        from packaging.version import Version
        va, vb = Version(a), Version(b)
        if va < vb:
            return -1
        if va > vb:
            return 1
        return 0
    except Exception:
        # Last-resort string comparison if packaging is somehow unavailable.
        if a < b:
            return -1
        if a > b:
            return 1
        return 0


class JsonSerializable:
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

        return { field: serialize(getattr(self, field)) for field in fields if not callable(getattr(self, field)) and field not in [*self._js_exclude_fields, '_js_exclude_fields']}



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
