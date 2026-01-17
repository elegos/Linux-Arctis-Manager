from typing import Callable, Literal, ParamSpec, TypeVar


P = ParamSpec("P")
R = TypeVar("R")

def status_type(name: str) -> Callable[[Callable[P, R]], Callable[P, R]]:
    def decorator(func: Callable[P, R]) -> Callable[P, R]:
        setattr(func, "_status_type", name)
        return func
    return decorator

@status_type("percentage")
def percentage(perc_min: int, perc_max: int, value: int) -> int:
    if perc_max < perc_min:
        value = perc_min - value
        perc_min, perc_max = perc_min, perc_max

        return 100 - (value - perc_min) * 100 // (perc_max - perc_min)

    return (value - perc_min) * 100 // (perc_max - perc_min)

@status_type("on_off")
def on_off(value: int, on: int, off: int) -> Literal['on', 'off']:
    return 'on' if value == on else 'off'

@status_type("int_str_mapping")
def int_str_mapping(values: dict[int, str], value: int) -> str|None:
    return values.get(value, None)

@status_type("int_int_mapping")
def int_int_mapping(values: dict[int, int], value: int) -> int|None:
    return values.get(value, None)
