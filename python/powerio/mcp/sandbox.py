"""Filesystem containment policy for MCP servers that expose powerio.

The powerio MCP server accepts local paths and ``file://`` URIs for its
``path`` and ``destination`` arguments, and confines every read and write to the
directories named by ``POWERIO_MCP_ALLOWED_ROOTS`` (an ``os.pathsep``
separated list). This module is the policy on its own: it imports nothing but
the standard library, so a server built on a different MCP SDK, or on no SDK
at all, can apply the same rules. Use :func:`checked_path` for one file,
:func:`checked_read_tree` before a directory reader, and
:func:`staged_file_write` for a file writer, and
:func:`staged_directory_write` for a directory writer.

When the variable is unset or empty, the policy is off and every path is
allowed.

    >>> from powerio.mcp.sandbox import checked_path
    >>> checked_path("case9.m", purpose="path")            # doctest: +SKIP
    '/data/case9.m'
    >>> checked_path(out, purpose="destination", for_write=True)  # doctest: +SKIP
    '/data/out.raw'
"""

from __future__ import annotations

import os
import shutil
import stat
import tempfile
from pathlib import Path
from typing import Any, Callable, TypeVar, cast
from urllib.parse import unquote, urlparse

ALLOWED_ROOTS_ENV = "POWERIO_MCP_ALLOWED_ROOTS"
"""Primary variable: an ``os.pathsep`` separated list of allowed roots."""

__all__ = [
    "ALLOWED_ROOTS_ENV",
    "PathNotAllowed",
    "admitting_root",
    "allowed_roots",
    "check_allowed_path",
    "check_allowed_read_tree",
    "checked_path",
    "checked_read_tree",
    "decode_local_path",
    "staged_file_write",
    "staged_directory_write",
]

_T = TypeVar("_T")


class PathNotAllowed(ValueError):
    """The path gate refused a value: outside the allowed roots, a remote
    URI, or a non-local file URI.
    """


def allowed_roots() -> tuple[Path, ...]:
    """Roots the policy confines paths to, empty when the policy is off."""
    raw = os.environ.get(ALLOWED_ROOTS_ENV) or ""
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


def checked_path(value: str, *, purpose: str = "path", for_write: bool = False) -> str:
    """Decode a tool argument and confine it to the allowed roots.

    Returns the decoded path as a string, ready to hand to a powerio reader or
    writer. Set ``for_write`` for an output argument: the parent directory must
    exist, and a dangling symlink in the final position is resolved before the
    containment check.
    """
    path = decode_local_path(value, purpose=purpose)
    check_allowed_path(path, for_write=for_write, purpose=purpose)
    return str(path)


def _inside(path: Path, root: Path) -> bool:
    return path == root or root in path.parents


def _check_tree_target(
    path: Path, root: Path | None, *, purpose: str
) -> os.stat_result:
    """Resolve one read-tree entry and return its followed stat record."""
    try:
        resolved = path.resolve(strict=True)
        info = path.stat()
    except FileNotFoundError as exc:
        raise PathNotAllowed(f"`{purpose}` contains a broken link: {path}") from exc
    except OSError as exc:
        raise PathNotAllowed(f"cannot inspect `{purpose}` entry {path}: {exc}") from exc
    if root is not None and not _inside(resolved, root):
        raise PathNotAllowed(
            f"`{purpose}` contains a link outside its allowed MCP root: {path}"
        )
    if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):
        raise PathNotAllowed(
            f"`{purpose}` contains a non-file, non-directory entry: {path}"
        )
    return info


def check_allowed_read_tree(path: Path, *, purpose: str = "path") -> None:
    """Preflight every file reachable from a directory input.

    The input and each followed symlink must remain under the same configured
    allowed root. Contained directory links are followed, with inode based
    cycle detection; broken links and special files are refused. A regular
    file is accepted as a one-entry tree.

    This closes the gap between checking a directory name and handing that
    directory to a reader which opens its children. It is still a path
    preflight, not a kernel sandbox: another process can replace an entry
    between this check and the reader's later ``open`` call.
    """
    root = admitting_root(path, purpose=purpose)
    stack = [path]
    visited: set[tuple[int, int]] = set()
    while stack:
        current = stack.pop()
        info = _check_tree_target(current, root, purpose=purpose)
        if stat.S_ISREG(info.st_mode):
            continue
        identity = (info.st_dev, info.st_ino)
        if identity in visited:
            continue
        visited.add(identity)
        try:
            with os.scandir(current) as entries:
                children = [Path(entry.path) for entry in entries]
        except OSError as exc:
            raise PathNotAllowed(
                f"cannot inspect `{purpose}` directory {current}: {exc}"
            ) from exc
        stack.extend(children)


def checked_read_tree(value: str, *, purpose: str = "path") -> str:
    """Decode a local path and preflight its complete readable tree."""
    path = decode_local_path(value, purpose=purpose)
    check_allowed_read_tree(path, purpose=purpose)
    return str(path)


def _plain_tree(root: Path, *, purpose: str) -> tuple[list[Path], list[Path]]:
    """List a directory tree that contains no links or special files."""
    try:
        root_info = root.lstat()
    except OSError as exc:
        raise PathNotAllowed(
            f"cannot inspect `{purpose}` directory {root}: {exc}"
        ) from exc
    if stat.S_ISLNK(root_info.st_mode) or not stat.S_ISDIR(root_info.st_mode):
        raise PathNotAllowed(
            f"`{purpose}` must be a real directory, not a link or file"
        )

    directories: list[Path] = []
    files: list[Path] = []
    for directory, names, filenames in os.walk(root, topdown=True, followlinks=False):
        base = Path(directory)
        for name in names:
            child = base / name
            try:
                info = child.lstat()
            except OSError as exc:
                raise PathNotAllowed(
                    f"cannot inspect `{purpose}` entry {child}: {exc}"
                ) from exc
            if stat.S_ISLNK(info.st_mode):
                raise PathNotAllowed(f"`{purpose}` contains a link: {child}")
            if not stat.S_ISDIR(info.st_mode):
                raise PathNotAllowed(f"`{purpose}` contains a special entry: {child}")
            directories.append(child.relative_to(root))
        for name in filenames:
            child = base / name
            try:
                info = child.lstat()
            except OSError as exc:
                raise PathNotAllowed(
                    f"cannot inspect `{purpose}` entry {child}: {exc}"
                ) from exc
            if stat.S_ISLNK(info.st_mode):
                raise PathNotAllowed(f"`{purpose}` contains a link: {child}")
            if not stat.S_ISREG(info.st_mode):
                raise PathNotAllowed(f"`{purpose}` contains a special entry: {child}")
            files.append(child.relative_to(root))
    directories.sort()
    files.sort()
    return directories, files


def _rebase_writer_result(result: _T, staging: Path, output: Path) -> _T:
    """Rewrite the directory and file paths returned by powerio writers."""
    if not isinstance(result, dict):
        return result

    staging_text = str(staging)
    output_text = str(output)

    def rebase(value: Any) -> Any:
        if isinstance(value, str) and (
            value == staging_text or value.startswith(staging_text + os.sep)
        ):
            return output_text + value[len(staging_text) :]
        return value

    rebased = dict(result)
    if "path" in rebased:
        rebased["path"] = rebase(rebased["path"])
    if "dir" in rebased:
        rebased["dir"] = rebase(rebased["dir"])
    if "files" in rebased and isinstance(rebased["files"], list):
        rebased["files"] = [rebase(item) for item in rebased["files"]]
    return cast(_T, rebased)


def staged_file_write(
    destination: str, overwrite: bool, write: Callable[[str], _T]
) -> _T:
    """Write one file privately, then install it at ``destination``.

    The writer receives a nonexistent sibling path because PowerIO emitters
    refuse collisions. The staged output must be one regular file. With
    ``overwrite=False``, a hard link installs it without replacing a target
    created concurrently. With ``overwrite=True``, ``os.replace`` performs one
    atomic replacement. Links, directories, and special files are refused as
    both staged output and existing destinations.
    """
    output = Path(os.path.abspath(destination))
    parent = output.parent
    if not parent.is_dir():
        raise PathNotAllowed(
            f"cannot resolve `destination`: parent does not exist: {parent}"
        )
    check_allowed_path(output, for_write=True, purpose="destination")

    if os.path.lexists(output):
        mode = output.lstat().st_mode
        if not stat.S_ISREG(mode):
            raise ValueError(f"destination is not a regular file: {output}")
        if not overwrite:
            raise ValueError(
                f"refusing to overwrite existing file: {output}; pass overwrite=true"
            )

    workspace = Path(tempfile.mkdtemp(prefix=f".{output.name}.stage-", dir=parent))
    staging = workspace / "out"
    try:
        result = write(str(staging))
        if not os.path.lexists(staging) or not stat.S_ISREG(staging.lstat().st_mode):
            raise ValueError(f"file writer did not produce one regular file: {staging}")
        if overwrite:
            os.replace(staging, output)
        else:
            try:
                os.link(staging, output, follow_symlinks=False)
            except FileExistsError as exc:
                raise ValueError(
                    f"refusing to overwrite existing file: {output}; pass overwrite=true"
                ) from exc
            staging.unlink()
        return _rebase_writer_result(result, staging, output)
    finally:
        shutil.rmtree(workspace, ignore_errors=True)


def staged_directory_write(
    destination: str, overwrite: bool, write: Callable[[str], _T]
) -> _T:
    """Run a directory writer privately, preflight it, then install it.

    A new output is installed by one same-filesystem rename. Updating an
    existing directory builds a complete sibling replacement, preserving
    unrelated files, then swaps it in. If the second rename fails, the original
    directory is restored. ``overwrite=False`` refuses every file collision
    before changing the output. Existing and generated trees must contain only
    real directories and regular files: links and special files are refused.

    The sibling rename prevents readers from seeing a partially copied output.
    As with the read-tree preflight, callers needing protection from a hostile
    concurrent process must also use an operating-system sandbox.
    """
    output = Path(os.path.abspath(destination))
    parent = output.parent
    if not parent.is_dir():
        raise PathNotAllowed(
            f"cannot resolve `destination`: parent does not exist: {parent}"
        )
    check_allowed_path(output, for_write=True, purpose="destination")

    workspace = Path(tempfile.mkdtemp(prefix=f".{output.name}.stage-", dir=parent))
    staging_installed = False
    os.chmod(workspace, 0o755)
    # The writers stage and commit themselves and refuse an existing target,
    # so the path handed to the writer must not exist yet: it is a child of
    # the private workspace, never the workspace directory itself.
    staging = workspace / "out"
    replacement: Path | None = None
    backup: Path | None = None
    try:
        result = write(str(staging))
        staged_dirs, staged_files = _plain_tree(staging, purpose="staged output")
        for relative in [*staged_dirs, *staged_files]:
            check_allowed_path(output / relative, purpose="destination")

        if not os.path.lexists(output):
            os.replace(staging, output)
            staging_installed = True
            shutil.rmtree(workspace, ignore_errors=True)
            return _rebase_writer_result(result, staging, output)

        existing_dirs, existing_files = _plain_tree(output, purpose="destination")
        existing_dir_set = set(existing_dirs)
        existing_file_set = set(existing_files)
        for relative in staged_dirs:
            if relative in existing_file_set:
                raise ValueError(
                    f"cannot replace file with directory: {output / relative}"
                )
        for relative in staged_files:
            target = output / relative
            if relative in existing_dir_set:
                raise ValueError(f"cannot replace directory with file: {target}")
            if not overwrite and relative in existing_file_set:
                raise ValueError(
                    f"refusing to overwrite existing file: {target}; pass overwrite=true"
                )

        replacement = Path(
            tempfile.mkdtemp(prefix=f".{output.name}.install-", dir=parent)
        )
        replacement.rmdir()
        shutil.copytree(output, replacement, copy_function=shutil.copy2)
        for relative in staged_dirs:
            (replacement / relative).mkdir(parents=True, exist_ok=True)
        for relative in staged_files:
            target = replacement / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(staging / relative, target)

        backup = Path(tempfile.mkdtemp(prefix=f".{output.name}.backup-", dir=parent))
        backup.rmdir()
        os.replace(output, backup)
        try:
            os.replace(replacement, output)
            replacement = None
        except BaseException:
            os.replace(backup, output)
            backup = None
            raise
        shutil.rmtree(backup)
        backup = None
        return _rebase_writer_result(result, staging, output)
    finally:
        if not staging_installed:
            shutil.rmtree(workspace, ignore_errors=True)
        if replacement is not None:
            shutil.rmtree(replacement, ignore_errors=True)
        if backup is not None:
            # A backup remains only when an exceptional concurrent filesystem
            # change prevented rollback. Never delete the user's old output.
            pass
