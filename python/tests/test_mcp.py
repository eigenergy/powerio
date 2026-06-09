"""Tests for the optional MCP server (``powerio.mcp``).

Run with ``pytest python/tests`` after ``maturin develop`` and ``pip install
'.[mcp]'``. The whole module skips cleanly when the ``mcp`` extra is absent
(e.g. on Python 3.9, where the SDK is unavailable). The FastMCP-decorated tools
stay ordinary callables, so we exercise them in-process without a transport.
"""

import json
import os
from pathlib import Path

import pytest

pytest.importorskip("mcp", reason="powerio[mcp] not installed (needs Python 3.10+)")

import powerio  # noqa: E402
from powerio.mcp import server  # noqa: E402
from powerio.mcp.server import case_summary, convert_case  # noqa: E402

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"


def test_case_summary_path():
    s = case_summary(path=str(DATA / "case9.m"))
    assert s["n_buses"] == 9
    assert s["source_format"] == "Matpower"
    assert s["base_mva"] == 100.0
    assert s["n_connected_components"] == 1
    assert s["connectivity_report"]["n_buses"] == 9


def test_case_summary_inline():
    text = (DATA / "case9.m").read_text()
    s = case_summary(content=text, format="matpower")
    assert s["n_buses"] == 9
    assert s["source_format"] == "Matpower"


def test_convert_case_path():
    r = convert_case(to="powermodels-json", path=str(DATA / "case30.m"))
    assert isinstance(r["text"], str) and r["text"]
    assert isinstance(r["warnings"], list)
    assert len(json.loads(r["text"])["bus"]) == 30


def test_convert_case_inline_requires_from():
    text = (DATA / "case30.m").read_text()
    with pytest.raises(ValueError):
        convert_case(to="psse", content=text)  # missing from_
    r = convert_case(to="psse", content=text, from_="matpower")
    assert r["text"]


def test_convert_case_exactly_one_input():
    with pytest.raises(ValueError):
        convert_case(to="matpower")  # neither path nor content
    with pytest.raises(ValueError):
        convert_case(to="matpower", path="x", content="y")  # both


def test_case_summary_exactly_one_input():
    with pytest.raises(ValueError):
        case_summary()  # neither
    with pytest.raises(ValueError):
        case_summary(path="x", content="y")  # both


def test_errors_map_cleanly():
    with pytest.raises(ValueError):  # FileNotFoundError → ValueError
        case_summary(path=str(DATA / "does_not_exist.m"))
    with pytest.raises(ValueError):  # PowerIOError → ValueError
        case_summary(content="not a case", format="matpower")


def test_convert_case_errors_map_cleanly():
    # convert_case has its own except arms, separate from case_summary's.
    with pytest.raises(ValueError):  # FileNotFoundError → ValueError
        convert_case(to="psse", path=str(DATA / "does_not_exist.m"))
    with pytest.raises(ValueError):  # PowerIOError → ValueError
        convert_case(to="psse", content="not a case", from_="matpower")


def test_convert_case_reports_lossy_warnings():
    # PSS/E has no cost curves, so case30 (which carries gencost) drops them and
    # the warning rides through conv.warnings → the tool's "warnings" list;
    # PowerModels JSON represents everything, so its conversion is warning-free.
    text = (DATA / "case30.m").read_text()
    lossy = convert_case(to="psse", content=text, from_="matpower")
    assert lossy["warnings"]
    assert any("cost" in w.lower() for w in lossy["warnings"])
    faithful = convert_case(to="powermodels-json", content=text, from_="matpower")
    assert faithful["warnings"] == []


def test_inline_convert_removes_temp_file(monkeypatch):
    # The inline path stages content to a temp file and unlinks it in a finally.
    # Capture the staged path and assert it's gone on both the success and the
    # failure path — the finally is the only guard against leaking temp files.
    staged = []
    real_mkstemp = server.tempfile.mkstemp

    def spy_mkstemp(*args, **kwargs):
        fd, path = real_mkstemp(*args, **kwargs)
        staged.append(path)
        return fd, path

    monkeypatch.setattr(server.tempfile, "mkstemp", spy_mkstemp)
    text = (DATA / "case30.m").read_text()

    r = convert_case(to="psse", content=text, from_="matpower")
    assert r["text"]
    assert staged and not os.path.exists(staged[-1])

    staged.clear()

    def boom(*args, **kwargs):
        raise powerio.PowerIOError("boom")

    monkeypatch.setattr(powerio, "convert_file", boom)
    with pytest.raises(ValueError):
        convert_case(to="psse", content=text, from_="matpower")
    assert staged and not os.path.exists(staged[-1])
