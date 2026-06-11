"""Tests for the optional MCP server (``powerio.mcp``).

Run with ``pytest python/tests`` after ``maturin develop`` and ``pip install
'.[mcp]'``. The whole module skips cleanly when the ``mcp`` extra is absent
(e.g. on Python 3.9, where the SDK is unavailable). The FastMCP-decorated tools
stay ordinary callables, so we exercise them in-process without a transport.
"""

import json
import tempfile
from pathlib import Path

import pytest

pytest.importorskip("mcp", reason="powerio[mcp] not installed (needs Python 3.10+)")

import powerio  # noqa: E402
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


def test_format_forwarded_for_path_inputs(tmp_path):
    # An extensionless path parses only when the explicit format reaches
    # parse_file; the old _parse dropped it and failed on the extension.
    bare = tmp_path / "case9_no_extension"
    bare.write_text((DATA / "case9.m").read_text())
    s = case_summary(path=str(bare), format="matpower")
    assert s["n_buses"] == 9


def test_format_still_inferred_without_one():
    # No format on a .json path: the extension sniff lands on pandapower.
    s = case_summary(path=str(DATA / "pandapower" / "example.json"))
    assert s["source_format"] == "PandapowerJson"


def test_parse_case_surfaces_read_warnings():
    from powerio.mcp.server import parse_case

    r = parse_case(path=str(DATA / "pandapower" / "example.json"),
                   format="pandapower-json")
    warnings = r["summary"]["read_warnings"]
    assert warnings and any("switch" in w for w in warnings)
    # A total reader yields an empty list, not a missing key.
    clean = parse_case(path=str(DATA / "case9.m"))
    assert clean["summary"]["read_warnings"] == []


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


def test_inline_convert_stages_no_temp_files(monkeypatch):
    # Inline conversion goes through powerio.convert_str entirely in memory;
    # touching tempfile would be a regression to the old staging path.
    def boom(*args, **kwargs):
        raise AssertionError("inline conversion must not create temp files")

    monkeypatch.setattr(tempfile, "mkstemp", boom)
    monkeypatch.setattr(tempfile, "NamedTemporaryFile", boom)
    text = (DATA / "case30.m").read_text()
    r = convert_case(to="psse", content=text, from_="matpower")
    assert r["text"]


def test_inline_convert_str_error_maps_cleanly(monkeypatch):
    def boom(*args, **kwargs):
        raise powerio.PowerIOError("boom")

    monkeypatch.setattr(powerio, "convert_str", boom)
    with pytest.raises(ValueError):
        convert_case(to="psse", content="whatever", from_="matpower")


# --- the full tool surface (parse_case .. save_case) -----------------------


def test_parse_case_json_round_trips():
    from powerio.mcp.server import parse_case

    r = parse_case(path=str(DATA / "case9.m"))
    assert r["summary"]["n_buses"] == 9
    assert powerio.from_json(r["json"]).n_buses == 9


def test_normalize_case_returns_dense_one_based_ids():
    from powerio.mcp.server import normalize_case

    r = normalize_case(path=str(DATA / "case9.m"))
    case = powerio.from_json(r["json"])
    assert [b["id"] for b in case.buses] == list(range(1, 10))


def test_case_to_json_accepted_downstream():
    from powerio.mcp.server import case_to_json, compute_matrix

    transport = case_to_json(path=str(DATA / "case9.m"))["json"]
    m = compute_matrix("bprime", json=transport)
    assert m["shape"] == [9, 9]


def test_compute_matrix_kinds_and_plain_types():
    from powerio.mcp.server import compute_matrix

    m = compute_matrix("bprime", path=str(DATA / "case9.m"))
    assert m["format"] == "coo"
    assert m["shape"] == [9, 9]
    assert m["nnz"] > 0 and isinstance(m["nnz"], int)
    assert type(m["data"][0]) is float
    assert type(m["row"][0]) is int
    lacpf = compute_matrix("lacpf", path=str(DATA / "case9.m"))
    assert lacpf["shape"] == [18, 18]
    for kind in ("bdoubleprime", "ybus_real", "ybus_imag", "adjacency",
                 "ptdf", "lodf", "laplacian"):
        assert compute_matrix(kind, path=str(DATA / "case9.m"))["nnz"] > 0
    with pytest.raises(ValueError):
        compute_matrix("nope", path=str(DATA / "case9.m"))


def test_dense_view_counts():
    from powerio.mcp.server import dense_view

    d = dense_view(path=str(DATA / "case9.m"))
    assert d["n"] == 9 and d["m"] == 9
    assert d["base_mva"] == 100.0
    assert type(d["bus_ids"][0]) is int
    assert type(d["branch"]["r"][0]) is float
    assert type(d["is_radial"]) is bool


def test_save_case_writes_and_refuses_overwrite(tmp_path):
    from powerio.mcp.server import save_case

    out = tmp_path / "case9.json"
    r = save_case(to="powermodels-json", out_path=str(out), path=str(DATA / "case9.m"))
    assert r["path"] == str(out)
    assert r["bytes_written"] == out.stat().st_size
    assert len(json.loads(out.read_text())["bus"]) == 9
    with pytest.raises(ValueError):
        save_case(to="powermodels-json", out_path=str(out), path=str(DATA / "case9.m"))
    r2 = save_case(
        to="matpower", out_path=str(out), path=str(DATA / "case9.m"), overwrite=True
    )
    assert r2["bytes_written"] == out.stat().st_size


def test_exactly_one_of_path_content_json():
    from powerio.mcp.server import compute_matrix, dense_view, parse_case

    with pytest.raises(ValueError):
        parse_case()
    with pytest.raises(ValueError):
        compute_matrix("bprime")
    with pytest.raises(ValueError):
        compute_matrix("bprime", path="x", json="{}")
    with pytest.raises(ValueError):
        dense_view(path="x", content="y")


def test_tool_surface_parity():
    # The PowerMCP bundle ships a standalone copy of this server
    # (powerio/powerio_mcp.py in Power-Agent/PowerMCP); powerio.mcp.server is
    # canonical. The set below is the shared surface; a tool added or removed
    # here fails this test until the set, and the PowerMCP copy, move with it.
    import asyncio

    from powerio.mcp import server

    names = {t.name for t in asyncio.run(server.mcp.list_tools())}
    assert names == {
        "convert_case", "save_case", "case_summary", "parse_case",
        "normalize_case", "case_to_json", "compute_matrix", "dense_view",
    }


def test_unreadable_file_maps_cleanly(tmp_path):
    # PermissionError must surface as the documented ValueError shape, like
    # FileNotFoundError, not leak raw through the tool.
    import os
    import sys

    if sys.platform == "win32" or os.geteuid() == 0:
        pytest.skip("permission bits are not enforceable here")
    locked = tmp_path / "locked.m"
    locked.write_text("function mpc = x\n")
    locked.chmod(0o000)
    try:
        with pytest.raises(ValueError, match="cannot read file"):
            convert_case(to="psse", path=str(locked))
        with pytest.raises(ValueError, match="cannot read file"):
            case_summary(path=str(locked))
    finally:
        locked.chmod(0o644)


def test_wrong_schema_json_maps_cleanly():
    from powerio.mcp.server import compute_matrix

    for bad in ("{}", "[]", "null", '{"buses": "nope"}'):
        with pytest.raises(ValueError, match="parse failed"):
            compute_matrix("bprime", json=bad)
