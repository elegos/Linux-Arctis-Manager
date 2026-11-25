import typing

# Python 3.15 removed this, and dbus-next 0.2.3 (unmaintained) imports it at
# module level. It has to be back before the first `import dbus_next`.
if not hasattr(typing, 'no_type_check_decorator'):
    typing.no_type_check_decorator = lambda decorator: decorator  # type: ignore[attr-defined]
