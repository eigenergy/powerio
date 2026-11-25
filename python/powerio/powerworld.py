"""Write PowerWorld binary cases (``.pwb``) through PowerWorld Simulator.

PowerWorld does not publish the ``.pwb`` layout, so PowerIO reads the binary
format but does not write it itself. :func:`write_pwb` writes the module as
PowerWorld auxiliary text with :func:`powerio.emit`, has Simulator load that
text through its SimAuto automation server, and has Simulator save the binary
case. It is the conversion Easy SimAuto (ESA) users make by hand with
``OpenCaseType`` and ``SaveCase``, with PowerIO's own aux writer in front::

    import powerio
    from powerio import powerworld

    module = powerio.parse("case14.m")
    result = powerworld.write_pwb(module, "case14.pwb")
    for diagnostic in result.diagnostics:
        print(diagnostic.code, diagnostic.message)

It needs Windows, a PowerWorld Simulator license that includes SimAuto, and
pywin32 (``pip install 'powerio[simauto]'``). Nothing here is imported or
started until :func:`write_pwb` runs, so ``import powerio`` stays free of
those requirements on every platform.
"""

from __future__ import annotations

import importlib as _importlib
import os as _os
import re as _re
import tempfile as _tempfile
from pathlib import Path as _Path
from typing import Any, Optional

from . import Artifact, EmitResult, PioModule, PowerIOError, _powerio, emit
from ._guard import guard as _guard

__all__ = ["SimAutoError", "write_pwb"]

#: The diagnostic code a :class:`SimAutoError` carries.
SIMAUTO_CODE: str = _powerio.SIMAUTO_CODE

# A network name used as the file stem of an in-memory artifact.
_SAFE_STEM = _re.compile(r"[A-Za-z0-9_.-]{1,100}")
# Lines of Simulator's message log that say the loaded aux lost data: a field
# Simulator does not define, an object it could not create, a record it
# skipped. PowerIO's writer states none of them, so any is a failure rather
# than a silent drop.
_LOG_LOSSES =("is not defined", "unable to create", "skipped", "not all required fields")


class SimAutoError(PowerIOError):
    """PowerWorld Simulator refused a step of a ``.pwb`` write, or is absent.

    ``code`` is ``BIND.PY.SIMAUTO``. ``log`` holds the lines of Simulator's
    message log that report lost data when that is the reason, and is empty
    otherwise.
    """

    def __init__(self, message: str, log: tuple[str, ...] = ()) -> None:
        super().__init__(message)
        self.code = SIMAUTO_CODE
        self.diagnostics = []
        self.log = log


def _call(session: Any, name: str, *args: Any) -> None:
    """Call one SimAuto function and raise on the error string it returns.

    A pywin32 SimAuto dispatch returns a tuple whose first element is an
    error string, empty on success. ESA's ``SAW`` returns ``None`` and raises
    its own exception, which propagates unchanged.
    """
    result = getattr(session, name)(*args)
    if isinstance(result, tuple) and result and isinstance(result[0], str) and result[0].strip():
        raise SimAutoError(f"SimAuto {name} failed: {result[0].strip()}")


class _Session:
    """A SimAuto server this module starts and stops for one write."""

    def __init__(self) -> None:
        # Imported by name: pywin32 exists only on Windows and ships no type
        # information, and the type gate runs everywhere.
        try:
            pythoncom = _importlib.import_module("pythoncom")
            win32com_client = _importlib.import_module("win32com.client")
        except ImportError as exc:
            raise SimAutoError(
                "writing .pwb needs PowerWorld Simulator's SimAuto server, which runs on "
                "Windows through pywin32; install it with pip install 'powerio[simauto]'"
            ) from exc
        self._pythoncom = pythoncom
        pythoncom.CoInitialize()
        try:
            self.server: Any = win32com_client.dynamic.Dispatch("pwrworld.SimulatorAuto")
        except Exception as exc:
            pythoncom.CoUninitialize()
            raise SimAutoError(
                "could not start PowerWorld Simulator's SimAuto server (pwrworld.SimulatorAuto); "
                f"check that Simulator and its SimAuto add-on are installed and licensed: {exc}"
            ) from exc
        self.server.UIVisible = False

    def close(self) -> None:
        try:
            self.server.CloseCase()
        finally:
            # Dropping the last reference stops the server process.
            self.server = None
            self._pythoncom.CoUninitialize()


def _file_type(version: Optional[int]) -> str:
    if version is None:
        return "PWB"
    if isinstance(version, bool) or not isinstance(version, int):
        raise TypeError("version must be an int Simulator release, such as 23, or None")
    if version < 1:
        raise ValueError("version must be a positive Simulator release, such as 23")
    return f"PWB{version}"


def _load_losses(log_path: str) -> tuple[str, ...]:
    try:
        text = _Path(log_path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ()
    return tuple(
        line.strip() for line in text.splitlines() if any(k in line.lower() for k in _LOG_LOSSES)
    )


def _save_pwb(session: Any, aux_path: str, pwb_path: str, file_type: str, log_path: str) -> None:
    _call(session, "RunScriptCommand", "NewCase;")
    _call(session, "RunScriptCommand", "LogClear;")
    _call(session, "ProcessAuxFile", aux_path)
    _call(session, "RunScriptCommand", f'LogSave("{log_path}", NO);')
    losses = _load_losses(log_path)
    if losses:
        shown = "\n  ".join(losses[:10])
        more = f"\n  ... {len(losses) - 10} more" if len(losses) > 10 else ""
        raise SimAutoError(
            f"Simulator did not load the written aux completely:\n  {shown}{more}", log=losses
        )
    _call(session, "SaveCase", pwb_path, file_type, True)


@_guard
def write_pwb(
    module: PioModule,
    destination: Optional[Any] = None,
    *,
    version: Optional[int] = None,
    simauto: Optional[Any] = None,
) -> EmitResult:
    """Write ``module`` as a PowerWorld binary case through Simulator.

    The module is emitted as PowerWorld aux with :func:`powerio.emit`, loaded
    into a new Simulator case, and saved as ``.pwb``. The result's diagnostics
    are the aux emission's: they report what the aux, and so the binary case,
    cannot state. Its fidelity is ``canonical``, since Simulator writes the
    bytes. If Simulator's message log reports a field it does not define or
    an object it could not create, no file is written and
    :class:`SimAutoError` lists those log lines.

    ``destination`` is a path, a binary file object, or ``None`` for an
    in-memory artifact, as in :func:`powerio.emit`. ``version`` selects an
    older binary layout (``23`` saves Simulator 23's ``PWB23``); ``None``
    saves the running Simulator's own. ``simauto`` reuses a SimAuto server
    you already started, a pywin32 ``pwrworld.SimulatorAuto`` dispatch or an
    ESA ``SAW``; its open case is replaced by the written case, which stays
    open. Without it, a server is started for this call and stopped after.
    """
    if not isinstance(module, PioModule):
        raise TypeError("module must be a PioModule")
    file_type = _file_type(version)
    path = None
    write = None
    if destination is not None:
        path = _path_of(destination)
        if path is None:
            write = getattr(destination, "write", None)
            if write is None:
                raise TypeError("destination must be a path or writable binary file object")
    aux = emit(module, "powerworld")
    aux_artifact = aux.artifacts[0]
    stem = _stem(module)
    with _tempfile.TemporaryDirectory(prefix="powerio-pwb-") as work:
        aux_path = _os.path.join(work, f"{stem}.aux")
        pwb_path = _os.path.join(work, f"{stem}.pwb")
        log_path = _os.path.join(work, "simulator.log")
        with open(aux_path, "wb") as handle:
            handle.write(aux_artifact.data or b"")
        if simauto is not None:
            _save_pwb(simauto, aux_path, pwb_path, file_type, log_path)
        else:
            session = _Session()
            try:
                _save_pwb(session.server, aux_path, pwb_path, file_type, log_path)
            finally:
                session.close()
        try:
            data = _Path(pwb_path).read_bytes()
        except OSError as exc:
            raise SimAutoError(f"Simulator reported a saved case but wrote no {pwb_path}") from exc
    if path is not None:
        with open(path, "wb") as handle:
            handle.write(data)
        artifact = Artifact(name=_os.path.basename(path), data=None, path=path)
    else:
        if write is not None:
            write(data)
        artifact = Artifact(name=f"{stem}.pwb", data=data, path=None)
    return EmitResult(
        artifacts=(artifact,),
        layout="file",
        fidelity="canonical",
        diagnostics=aux.diagnostics,
    )


def _stem(module: PioModule) -> str:
    """The network's name when it is a safe file stem, else ``case``."""
    name = getattr(module.value, "name", None)
    if isinstance(name, str) and _SAFE_STEM.fullmatch(name):
        return name
    return "case"


def _path_of(destination: Any) -> Optional[str]:
    if isinstance(destination, str):
        return destination
    try:
        path = _os.fspath(destination)
    except TypeError:
        return None
    if isinstance(path, bytes):
        raise TypeError("a path-like destination must return str, not bytes")
    return path
