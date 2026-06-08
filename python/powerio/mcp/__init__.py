"""MCP (Model Context Protocol) server for powerio.

Optional: install with ``pip install 'powerio[mcp]'`` (needs Python 3.10+).
This submodule is never imported by ``powerio/__init__.py``, so ``import
powerio`` stays zero-dependency; the MCP SDK is pulled in only here.
"""

from .server import main

__all__ = ["main"]
