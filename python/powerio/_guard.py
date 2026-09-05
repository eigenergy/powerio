"""Convert a Rust panic into a coded :class:`PowerIOError`.

pyo3 reports a panic as ``pyo3_runtime.PanicException``, a ``BaseException``
that escapes ``except Exception``. Every public entry point of this package is
wrapped so a panic surfaces as ``PowerIOError`` with code ``BIND.PY.PANIC``,
the same shape the C ABI gives its callers.
"""

from __future__ import annotations

import functools
import types
from typing import Any, Callable, TypeVar

from ._powerio import PANIC_CODE, PanicException, PowerIOError

_F = TypeVar("_F", bound=Callable[..., Any])
_C = TypeVar("_C", bound=type)


def panic_error(exc: BaseException) -> PowerIOError:
    error = PowerIOError(f"PowerIO panicked inside the extension: {exc}")
    error.code = PANIC_CODE
    return error


def guard(fn: _F) -> _F:
    """Wrap one callable so a panic raises :class:`PowerIOError`."""

    @functools.wraps(fn)
    def guarded(*args: Any, **kwargs: Any) -> Any:
        try:
            return fn(*args, **kwargs)
        except PanicException as exc:
            raise panic_error(exc) from None

    return guarded  # type: ignore[return-value]


def guard_class(cls: _C) -> _C:
    """Wrap every function, property, classmethod, and staticmethod of ``cls``."""
    for name, member in list(vars(cls).items()):
        if isinstance(member, types.FunctionType):
            setattr(cls, name, guard(member))
        elif isinstance(member, property):
            fget = guard(member.fget) if member.fget is not None else None
            setattr(cls, name, property(fget, member.fset, member.fdel, member.__doc__))
        elif isinstance(member, classmethod):
            setattr(cls, name, classmethod(guard(member.__func__)))
        elif isinstance(member, staticmethod):
            setattr(cls, name, staticmethod(guard(member.__func__)))
    return cls
