"""Filesystem containment policy for MCP servers that expose powerio.

The powerio MCP server accepts local paths and ``file://`` URIs for its
``path`` and ``out_path`` arguments, and confines every read and write to the
directories named by ``POWERIO_MCP_ALLOWED_ROOTS`` (an ``os.pathsep``
separated list). This module is the policy on its own: it imports nothing but
the standard library, so a server built on a different MCP SDK, or on no SDK
at all, can apply the same rules by calling :func:`checked_path`.

Two legacy single root spellings are still read, in this order after the
primary variable: ``POWERIO_MCP_ROOT``, then ``POWERIO_MCP_ALLOWED_ROOT``, an
alternate legacy spelling. The first variable that is set and non-empty wins;
the others are ignored rather than merged. With none of the three set the
policy is off and every path is allowed.

    >>> from powerio.mcp.sandbox import checked_path
    >>> checked_path("case9.m", purpose="path")            # doctest: +SKIP
    '/data/case9.m'
    >>> checked_path(out, purpose="out_path", for_write=True)  # doctest: +SKIP
    '/data/out.raw'
"""

from __future__ import annotations

import os
from pathlib import Path
from urllib.parse import unquote, urlparse

ALLOWED_ROOTS_ENV = "POWERIO_MCP_ALLOWED_ROOTS"
"""Primary variable: an ``os.pathsep`` separated list of allowed roots."""

LEGACY_ROOT_ENVS = ("POWERIO_MCP_ROOT", "POWERIO_MCP_ALLOWED_ROOT")
"""Legacy single root spellings, read in order when the primary is unset."""

__all__ = [
    "ALLOWED_ROOTS_ENV",
    "LEGACY_ROOT_ENVS",
    "PathNotAllowed",
    "admitting_root",
    "allowed_roots",
    "check_allowed_path",
    "checked_path",
    "decode_local_path",
]


class PathNotAllowed(ValueError):
    """The path gate refused a value: outside the allowed roots, a remote
    URI, or a non-local file URI.

    Subclasses :class:`ValueError`, which is what every refusal site
    raised before this type existed.
    """


def allowed_roots() -> tuple[Path, ...]:
    """Roots the policy confines paths to, empty when the policy is off."""
    raw = ""
    for name in (ALLOWED_ROOTS_ENV,) + LEGACY_ROOT_ENVS:
        raw = os.environ.get(name) or ""
        if raw:
            break
    if not raw:
        return ()
    roots = []
    for entry in raw.split(os.pathsep):
        item = entry.strip()
        if item:
            roots.append(Path(item).expanduser().resolve(strict=False))
    return tuple(roots)


def decode_local_path(value: str, *, purpose: str = "path") -> Path:
    """Read a local path or a local ``file://`` URI as a :class:`Path`.

    Remote URI schemes are refused. ``purpose`` names the argument in the
    error message.
    """
    parsed = urlparse(str(value))
    windows_drive = os.name == "nt" and len(parsed.scheme) == 1
    if parsed.scheme and not windows_drive:
        if parsed.scheme != "file":
            raise PathNotAllowed(f"`{purpose}` must be a local path or file:// URI")
        netloc = unquote(parsed.netloc)
        path = unquote(parsed.path)
        if len(netloc) == 2 and netloc[0].isalpha() and netloc[1] == ":":
            return Path(f"{netloc}{path}").expanduser()
        if netloc.lower() not in ("", "localhost"):
            raise PathNotAllowed(f"`{purpose}` file URI must be local")
        if (
            len(path) >= 3
            and path[0] == "/"
            and path[1].isalpha()
            and path[2] == ":"
            and (len(path) == 3 or path[3] in "/\\")
        ):
            path = path[1:]
        return Path(path).expanduser()
    return Path(str(value)).expanduser()


def _path_for_policy(path: Path, *, for_write: bool) -> Path:
    try:
        if for_write and not path.exists():
            parent = path.parent if path.parent != Path("") else Path(".")
            candidate = parent.resolve(strict=True) / path.name
            # `path.exists()` follows symlinks, so a final component that is a
            # dangling symlink (its target absent) lands here. Joining the name
            # onto the resolved parent would leave that symlink unresolved, so
            # the containment check would pass on the link's own location while
            # the real write followed it outside the roots. `realpath` resolves
            # the final symlink (and is a no-op for a plain new name).
            return Path(os.path.realpath(candidate))
        return path.resolve(strict=True)
    except FileNotFoundError:
        if for_write:
            raise
        return path.resolve(strict=False)


def admitting_root(
    path: Path, *, for_write: bool = False, purpose: str = "path"
) -> Path | None:
    """The allowed root that contains ``path``, ``None`` when the policy is off.

    Raises :class:`PathNotAllowed` when roots are configured and ``path``
    resolves outside all of them. Symlinks are resolved first, including a
    dangling final component under ``for_write``, so a link inside a root
    cannot redirect a write out of it.
    """
    roots = allowed_roots()
    if not roots:
        return None
    try:
        resolved = _path_for_policy(path, for_write=for_write)
    except OSError as exc:
        raise PathNotAllowed(
            f"cannot resolve `{purpose}` against allowed MCP roots: {exc}"
        ) from exc
    for root in roots:
        if resolved == root or root in resolved.parents:
            return root
    root_list = ", ".join(str(root) for root in roots)
    raise PathNotAllowed(f"`{purpose}` is outside allowed MCP roots: {root_list}")


def check_allowed_path(
    path: Path, *, for_write: bool = False, purpose: str = "path"
) -> None:
    """Raise :class:`PathNotAllowed` when ``path`` resolves outside the roots.

    :func:`admitting_root` with the result discarded.
    """
    admitting_root(path, for_write=for_write, purpose=purpose)


def checked_path(
    value: str, *, purpose: str = "path", for_write: bool = False
) -> str:
    """Decode a tool argument and confine it to the allowed roots.

    Returns the decoded path as a string, ready to hand to a powerio reader or
    writer. Set ``for_write`` for an output argument: the parent directory must
    exist, and a dangling symlink in the final position is resolved before the
    containment check.
    """
    path = decode_local_path(value, purpose=purpose)
    check_allowed_path(path, for_write=for_write, purpose=purpose)
    return str(path)
