"""MCP (Model Context Protocol) server for powerio.

Optional: install with ``pip install 'powerio[mcp]'`` (needs Python 3.10+).
This submodule is never imported by ``powerio/__init__.py``, so ``import
powerio`` stays zero-dep; the MCP SDK is pulled in only here.

``powerio.mcp.sandbox`` is the exception: it carries the filesystem
containment policy the server applies to ``path`` and ``out_path``, imports
only the standard library, and is importable on every version the wheel
supports. ``main`` is resolved on attribute access so reaching the sandbox
does not drag in the SDK.
"""

import importlib
import sys
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from . import sandbox as sandbox
    from .server import main as main

__all__ = ["main", "sandbox"]

_MIN_PYTHON = (3, 10)


def __getattr__(name: str) -> Any:
    # `import_module` rather than `from . import sandbox`: the latter looks the
    # attribute up on this module again and recurses back into here.
    if name == "sandbox":
        return importlib.import_module(f"{__name__}.sandbox")
    if name == "main":
        # server.py uses typing.TypeAlias (3.10+), and the mcp extra installs
        # nothing on 3.9, so importing it there would fail on that syntax
        # instead of saying why. Gate before the import runs.
        if sys.version_info < _MIN_PYTHON:
            raise ImportError(
                "powerio.mcp requires Python 3.10+; the mcp extra installs "
                "nothing on 3.9"
            )
        return importlib.import_module(f"{__name__}.server").main
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
