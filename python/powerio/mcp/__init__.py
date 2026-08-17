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
from typing import Any

__all__ = ["main", "sandbox"]


def __getattr__(name: str) -> Any:
    # `import_module` rather than `from . import sandbox`: the latter looks the
    # attribute up on this module again and recurses back into here.
    if name == "sandbox":
        return importlib.import_module(f"{__name__}.sandbox")
    if name == "main":
        return importlib.import_module(f"{__name__}.server").main
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
