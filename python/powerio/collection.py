"""Bounded inventories with independent parser selections and typed results."""
from __future__ import annotations

import json
import os
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Mapping

from . import _powerio
from ._guard import guard

if TYPE_CHECKING:
    from . import PioModule, SincalBalancedReadOptions
    from .dist import SincalReadOptions


@dataclass(frozen=True)
class CollectionResult:
    module: PioModule
    dependencies: tuple[str, ...]
    incomplete: bool


class Collection:
    """A shared inventory. Closing it does not invalidate parsed modules.

    Inspect returns metadata and suggested selection dictionaries without
    electrical parsing. Pass one selection to parse. Candidates are not a
    completeness or solver-readiness certificate.
    """

    def __init__(self, native: Any) -> None:
        self._native = native

    @classmethod
    @guard
    def open(cls, path: str | os.PathLike[str], *, max_bytes: int = 64 << 20,
             max_files: int = 4096) -> Collection:
        return cls(_powerio._Collection._open(os.fspath(path), max_bytes, max_files))

    @classmethod
    @guard
    def from_zip(cls, data: bytes, *, name: str = "collection.zip",
                 max_bytes: int = 64 << 20, max_files: int = 4096) -> Collection:
        return cls(_powerio._Collection._from_zip(name, data, max_bytes, max_files))

    @classmethod
    @guard
    def from_files(cls, files: Mapping[str, bytes], *, name: str = "collection",
                   max_bytes: int = 64 << 20, max_files: int = 4096) -> Collection:
        return cls(_powerio._Collection._from_files(name, dict(files), max_bytes, max_files))

    @guard
    def inspect(self) -> dict[str, Any]:
        return self._native._inspect()

    @guard
    def parse(self, selection: Mapping[str, Any], *,
              sincal_multiconductor: SincalReadOptions | None = None,
              sincal_balanced: SincalBalancedReadOptions | None = None) -> CollectionResult:
        from . import PioModule
        from ._sincal import _native_selections
        tx, mc = _native_selections(sincal_balanced, sincal_multiconductor)
        module, dependencies, incomplete = self._native._parse(json.dumps(dict(selection)), mc, tx)
        return CollectionResult(PioModule(module), tuple(dependencies), incomplete)

    def close(self) -> None:
        self._native.close()

    def __enter__(self) -> Collection:
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()
