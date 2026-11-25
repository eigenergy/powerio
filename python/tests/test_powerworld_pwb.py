"""powerio.powerworld.write_pwb: a .pwb written by PowerWorld Simulator.

The fake SimAuto server below records the calls and writes stand-in bytes, so
the call sequence, the error handling, and the destinations are checked on
every platform. ``test_live_simauto_*`` drives a real Simulator and runs only
with ``POWERIO_SIMAUTO=1`` on a Windows machine that has one.
"""

import io
import os
import re
import sys
from pathlib import Path

import pytest

import powerio
from powerio import powerworld

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
PWB_BYTES = b"\x98:\x00\x00stand-in pwb"


class FakeSimAuto:
    """A pywin32 style SimAuto dispatch: every call returns a tuple whose
    first element is an error string, empty on success."""

    def __init__(self, log="", errors=None):
        self.calls = []
        self.log = log
        self.errors = errors or {}
        self.aux_text = None
        self.UIVisible = True

    def _result(self, name):
        return (self.errors.get(name, ""),)

    def RunScriptCommand(self, statement):
        self.calls.append(("RunScriptCommand", statement))
        saved = re.match(r'LogSave\("(.*)", NO\);', statement)
        if saved:
            Path(saved.group(1)).write_text(self.log, encoding="utf-8")
        return self._result("RunScriptCommand")

    def ProcessAuxFile(self, path):
        self.calls.append(("ProcessAuxFile", path))
        self.aux_text = Path(path).read_text(encoding="utf-8")
        return self._result("ProcessAuxFile")

    def SaveCase(self, path, file_type, overwrite):
        self.calls.append(("SaveCase", file_type, overwrite))
        if not self.errors.get("SaveCase"):
            Path(path).write_bytes(PWB_BYTES)
        return self._result("SaveCase")

    def CloseCase(self):
        self.calls.append(("CloseCase",))
        return ("",)


class FakeSaw:
    """An ESA ``SAW``: success returns None, failure raises ESA's own error."""

    class PowerWorldError(Exception):
        pass

    def __init__(self, fail=None):
        self.inner = FakeSimAuto()
        self.fail = fail

    def __getattr__(self, name):
        method = getattr(self.inner, name)

        def call(*args):
            if name == self.fail:
                raise self.PowerWorldError(f"{name} failed in ESA")
            method(*args)

        return call


@pytest.fixture
def case14():
    return powerio.parse(DATA / "case14.m")


def test_writes_the_aux_emission_through_simulator_to_a_path(case14, tmp_path):
    sa = FakeSimAuto()
    target = tmp_path / "case14.pwb"
    result = powerworld.write_pwb(case14, target, simauto=sa)

    assert target.read_bytes() == PWB_BYTES
    assert result.artifacts == (powerio.Artifact(name="case14.pwb", data=None, path=str(target)),)
    assert result.layout == "file"
    assert result.fidelity == "canonical"
    aux = powerio.emit(case14, "powerworld")
    assert sa.aux_text == aux.text
    assert [d.code for d in result.diagnostics] == [d.code for d in aux.diagnostics]
    names = [call[0] for call in sa.calls]
    assert names == [
        "RunScriptCommand",
        "RunScriptCommand",
        "ProcessAuxFile",
        "RunScriptCommand",
        "SaveCase",
    ]
    assert sa.calls[0] == ("RunScriptCommand", "NewCase;")
    assert sa.calls[-1] == ("SaveCase", "PWB", True)
    # A session the caller supplied keeps the written case open.
    assert ("CloseCase",) not in sa.calls


def test_memory_and_file_object_destinations(case14):
    result = powerworld.write_pwb(case14, simauto=FakeSimAuto())
    assert result.artifacts == (powerio.Artifact(name="case14.pwb", data=PWB_BYTES, path=None),)

    sink = io.BytesIO()
    result = powerworld.write_pwb(case14, sink, simauto=FakeSimAuto())
    assert sink.getvalue() == PWB_BYTES
    assert result.artifacts[0].data == PWB_BYTES

    with pytest.raises(TypeError):
        powerworld.write_pwb(case14, object(), simauto=FakeSimAuto())


@pytest.mark.parametrize(("version", "file_type"), [(None, "PWB"), (23, "PWB23"), (20, "PWB20")])
def test_version_selects_the_saved_layout(case14, version, file_type):
    sa = FakeSimAuto()
    powerworld.write_pwb(case14, version=version, simauto=sa)
    assert sa.calls[-1] == ("SaveCase", file_type, True)


@pytest.mark.parametrize(("version", "error"), [(True, TypeError), ("23", TypeError), (2.0, TypeError), (0, ValueError)])
def test_a_version_that_is_not_a_release_is_refused(case14, version, error):
    sa = FakeSimAuto()
    with pytest.raises(error):
        powerworld.write_pwb(case14, version=version, simauto=sa)
    assert sa.calls == []


def test_a_simauto_error_string_raises_with_the_function_named(case14, tmp_path):
    sa = FakeSimAuto(errors={"SaveCase": "SaveCase: Error trying to save - aborted"})
    target = tmp_path / "x.pwb"
    with pytest.raises(powerworld.SimAutoError) as raised:
        powerworld.write_pwb(case14, target, simauto=sa)
    assert raised.value.code == "BIND.PY.SIMAUTO"
    assert raised.value.code == powerworld.SIMAUTO_CODE
    assert "SaveCase" in str(raised.value) and "aborted" in str(raised.value)
    assert isinstance(raised.value, powerio.PowerIOError)
    assert not target.exists()


def test_data_simulator_did_not_load_is_an_error_not_a_silent_drop(case14, tmp_path):
    log = (
        "DATA (LOAD, [BUSNUM, LOADID, LOADMW]):\n"
        " because not all required fields have been specified\n"
        "      Unable to create [Load 2 1]\n"
        "0 records read from file.\n"
        "11 records skipped when reading file because the object could not be found/created.\n"
    )
    sa = FakeSimAuto(log=log)
    target = tmp_path / "x.pwb"
    with pytest.raises(powerworld.SimAutoError) as raised:
        powerworld.write_pwb(case14, target, simauto=sa)
    assert "Unable to create [Load 2 1]" in raised.value.log
    assert any("skipped" in line for line in raised.value.log)
    assert all(call[0] != "SaveCase" for call in sa.calls)
    assert not target.exists()


def test_an_esa_saw_works_as_the_session(case14):
    saw = FakeSaw()
    result = powerworld.write_pwb(case14, simauto=saw)
    assert result.artifacts[0].data == PWB_BYTES
    assert saw.inner.calls[-1] == ("SaveCase", "PWB", True)

    with pytest.raises(FakeSaw.PowerWorldError):
        powerworld.write_pwb(case14, simauto=FakeSaw(fail="ProcessAuxFile"))


def test_a_started_session_is_closed_even_when_the_write_fails(case14, monkeypatch):
    started = []

    class StubSession:
        def __init__(self):
            self.server = FakeSimAuto(errors={"ProcessAuxFile": "ProcessAuxFile: bad"})
            self.closed = False
            started.append(self)

        def close(self):
            self.closed = True

    monkeypatch.setattr(powerworld, "_Session", StubSession)
    with pytest.raises(powerworld.SimAutoError):
        powerworld.write_pwb(case14)
    assert started and started[0].closed


def test_a_module_is_required():
    with pytest.raises(TypeError):
        powerworld.write_pwb(powerio.parse(DATA / "case14.m").value, simauto=FakeSimAuto())


def test_import_powerio_does_not_import_pywin32():
    import subprocess

    probe = "import sys, powerio; print('win32com' in sys.modules or 'pythoncom' in sys.modules)"
    out = subprocess.run([sys.executable, "-c", probe], capture_output=True, text=True, check=True)
    assert out.stdout.strip() == "False"


live = pytest.mark.skipif(
    os.environ.get("POWERIO_SIMAUTO") != "1" or sys.platform != "win32",
    reason="drives a real PowerWorld Simulator; set POWERIO_SIMAUTO=1 on Windows",
)


@live
@pytest.mark.parametrize("case", ["case14.m", "case33bw.m"])
def test_live_simauto_writes_a_case_simulator_reopens(case, tmp_path):
    import pythoncom
    import win32com.client

    module = powerio.parse(DATA / case)
    target = tmp_path / (Path(case).stem + ".pwb")
    powerworld.write_pwb(module, target)
    assert target.read_bytes()[:2] == b"\x98:"

    pythoncom.CoInitialize()
    sa = win32com.client.dynamic.Dispatch("pwrworld.SimulatorAuto")
    try:
        assert sa.OpenCase(str(target))[0] == ""
        net = module.value
        for obj, count in [
            ("Bus", net.n_buses),
            ("Gen", net.n_generators),
            ("Load", net.n_loads),
            ("Shunt", net.n_shunts),
            ("Branch", net.n_branches),
        ]:
            result = sa.GetParametersMultipleElement(obj, ["BusNum"], "")
            held = len(result[1][0]) if result[1] else 0
            assert held == count, f"{obj}: {held} in the .pwb, {count} written"
        sbase = sa.GetParametersSingleElement("Sim_Solution_Options", ["SBase"], [""])[1][0]
        assert float(sbase) == net.base_mva
    finally:
        sa.CloseCase()
        del sa
        pythoncom.CoUninitialize()


@live
@pytest.mark.parametrize("version", [21, 23])
def test_live_simauto_older_layouts_read_back_through_powerio(version):
    # PowerIO's .pwb reader decodes the Simulator 21 and 23 layouts (format
    # words 508 and 554); Simulator 24's own (577) is not decoded yet.
    module = powerio.parse(DATA / "case118.m")
    data = powerworld.write_pwb(module, version=version).artifacts[0].data
    back = powerio.parse(data, format="pwb", name="case118.pwb").value
    net = module.value
    assert (back.n_buses, back.n_generators, back.n_loads, back.n_shunts, back.n_branches) == (
        net.n_buses,
        net.n_generators,
        net.n_loads,
        net.n_shunts,
        net.n_branches,
    )


@live
def test_live_simauto_saves_an_older_layout():
    # The header's format word names the writer era: Simulator 23 saves 554.
    module = powerio.parse(DATA / "case14.m")
    result = powerworld.write_pwb(module, version=23)
    data = result.artifacts[0].data
    assert int.from_bytes(data[8:12], "little") == 554
