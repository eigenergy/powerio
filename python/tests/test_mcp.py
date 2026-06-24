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

# The gridfm Parquet surface is a compile-time feature; skip its tools when the
# extension was built without it (mirrors test_powerio.py).
HAS_GRIDFM = bool(getattr(powerio._powerio, "_has_gridfm", False))
gridfm_only = pytest.mark.skipif(
    not HAS_GRIDFM, reason="extension built without the gridfm feature"
)


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


def test_convert_case_pandapower_json_and_alias():
    r = convert_case(to="pandapower-json", path=str(DATA / "case9.m"))
    assert json.loads(r["text"])["_class"] == "pandapowerNet"
    alias = convert_case(to="pp", path=str(DATA / "case9.m"))
    assert json.loads(alias["text"])["_class"] == "pandapowerNet"
    back = case_summary(content=r["text"], format="pandapower-json")
    assert back["n_buses"] == 9
    assert back["source_format"] == "PandapowerJson"


def test_pypsa_folder_path_inputs():
    net = powerio.parse_file(str(DATA / "case9.m"))
    with tempfile.TemporaryDirectory() as tmp:
        folder = str(Path(tmp) / "case9-pypsa")
        net.write_pypsa_csv_folder(folder)
        s = case_summary(path=folder)
        assert s["n_buses"] == 9
        assert s["source_format"] == "PypsaCsv"
        r = convert_case(to="matpower", path=folder, from_="pypsa-csv")
        assert r["text"].startswith("function mpc =")


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


def test_save_case_echo_keeps_read_warnings(tmp_path):
    # Deliberate divergence from convert_case: a byte exact echo reports no
    # warnings there, but save_case describes the written file end to end, so
    # the read side stays.
    from powerio.mcp.server import convert_case, save_case

    src = DATA / "pandapower" / "example.json"
    out = tmp_path / "echo.json"
    saved = save_case(to="pandapower-json", out_path=str(out), path=str(src))
    assert any("switch" in w for w in saved["warnings"]), saved["warnings"]
    echoed = convert_case(to="pandapower-json", path=str(src))
    assert echoed["warnings"] == []


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
    # The PowerMCP bundle re-exports this server verbatim
    # (powerio/powerio_mcp.py in Power-Agent/PowerMCP); powerio.mcp.server is
    # canonical. The set below is the shared surface; a tool added or removed
    # here fails this test until the set, and the PowerMCP re-export, move with it.
    import asyncio

    from powerio.mcp import server

    names = {t.name for t in asyncio.run(server.mcp.list_tools())}
    assert names == {
        "convert_case", "save_case", "case_summary", "parse_case",
        "normalize_case", "case_to_json", "compute_matrix", "dense_view",
        "read_pypsa_csv_folder", "write_pypsa_csv_folder",
        "read_gridfm", "write_gridfm",
        "convert_dist_case", "dist_case_summary", "save_dist_case",
        "read_display_file",
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


# ---------------------------------------------------------------------------
# Folder / Parquet tools: PyPSA static CSV folders and gridfm Parquet datasets,
# which have no single-file text form and so get dedicated read/write tools.
# ---------------------------------------------------------------------------

def test_pypsa_csv_folder_round_trip(tmp_path):
    from powerio.mcp.server import read_pypsa_csv_folder, write_pypsa_csv_folder

    out_dir = tmp_path / "pypsa_csv"
    w = write_pypsa_csv_folder(str(out_dir), path=str(DATA / "case9.m"))
    assert w["files"], w
    assert (out_dir / "buses.csv").exists()
    r = read_pypsa_csv_folder(str(out_dir))
    assert r["summary"]["n_buses"] == 9
    assert json.loads(r["json"])


def test_pypsa_csv_folder_accepts_transport(tmp_path):
    from powerio.mcp.server import parse_case, write_pypsa_csv_folder

    transport = parse_case(path=str(DATA / "case9.m"))["json"]
    out_dir = tmp_path / "from_json"
    write_pypsa_csv_folder(str(out_dir), json=transport)
    assert (out_dir / "generators.csv").exists()


def test_read_pypsa_csv_missing_folder_maps_cleanly(tmp_path):
    from powerio.mcp.server import read_pypsa_csv_folder

    with pytest.raises(ValueError):
        read_pypsa_csv_folder(str(tmp_path / "nope"))


@gridfm_only
def test_gridfm_round_trip(tmp_path):
    from powerio.mcp.server import read_gridfm, write_gridfm

    out_dir = tmp_path / "gfm"
    w = write_gridfm(str(out_dir), path=str(DATA / "case9.m"))
    assert w["files"], w
    r = read_gridfm(str(out_dir))
    assert r["summary"]["n_buses"] == 9
    assert r["scenario"] == 0
    assert json.loads(r["json"])


@gridfm_only
def test_read_gridfm_missing_dir_maps_cleanly(tmp_path):
    from powerio.mcp.server import read_gridfm

    with pytest.raises(ValueError):
        read_gridfm(str(tmp_path / "nope"))


# ---------------------------------------------------------------------------
# Distribution tools: multiconductor cases (powerio.dist) in OpenDSS .dss,
# PowerModelsDistribution ENGINEERING JSON, and IEEE BMOPF JSON. A DistCase has
# no JSON transport, so these tools take only path/content (never json), and
# inline content requires an explicit format.
# ---------------------------------------------------------------------------

DSS = DATA / "dist" / "micro" / "xfmr_single_phase.dss"


def test_dist_case_summary_counts():
    from powerio.mcp.server import dist_case_summary

    s = dist_case_summary(path=str(DSS))
    assert s["source_format"] == "dss"
    assert s["n_buses"] == 2
    assert s["n_transformers"] == 1
    assert s["n_loads"] == 1
    assert isinstance(s["warnings"], list)


def test_convert_dist_case_dss_to_pmd_and_bmopf():
    from powerio.mcp.server import convert_dist_case

    pmd = convert_dist_case(to="pmd-json", path=str(DSS))
    assert json.loads(pmd["text"])["data_model"] == "ENGINEERING"
    assert isinstance(pmd["warnings"], list)
    bmopf = convert_dist_case(to="bmopf-json", path=str(DSS))
    assert "bus" in json.loads(bmopf["text"])


def test_convert_dist_case_same_format_echoes():
    from powerio.mcp.server import convert_dist_case

    r = convert_dist_case(to="dss", path=str(DSS))
    assert r["text"] == DSS.read_text()
    assert r["warnings"] == []


def test_convert_dist_case_inline_requires_from():
    from powerio.mcp.server import convert_dist_case

    text = DSS.read_text()
    with pytest.raises(ValueError):
        convert_dist_case(to="pmd-json", content=text)  # missing from_
    r = convert_dist_case(to="pmd-json", content=text, from_="dss")
    assert json.loads(r["text"])["data_model"] == "ENGINEERING"


def test_dist_case_summary_inline_requires_format():
    from powerio.mcp.server import dist_case_summary

    with pytest.raises(ValueError):
        dist_case_summary(content=DSS.read_text())  # missing format
    s = dist_case_summary(content=DSS.read_text(), format="dss")
    assert s["n_buses"] == 2


def test_dist_tools_exactly_one_input():
    from powerio.mcp.server import convert_dist_case, dist_case_summary

    with pytest.raises(ValueError):
        dist_case_summary()  # neither path nor content
    with pytest.raises(ValueError):
        dist_case_summary(path="x", content="y")  # both
    with pytest.raises(ValueError):
        convert_dist_case(to="dss")  # neither
    with pytest.raises(ValueError):
        convert_dist_case(to="dss", path="x", content="y")  # both


def test_dist_errors_map_cleanly():
    from powerio.mcp.server import convert_dist_case, dist_case_summary

    with pytest.raises(ValueError):  # FileNotFoundError → ValueError
        dist_case_summary(path=str(DATA / "dist" / "does_not_exist.dss"))
    with pytest.raises(ValueError):  # PowerIOParseError → ValueError
        convert_dist_case(to="dss", content="{not json", from_="bmopf-json")


def test_save_dist_case_writes_and_refuses_overwrite(tmp_path):
    from powerio.mcp.server import save_dist_case

    out = tmp_path / "feeder.json"
    r = save_dist_case(to="pmd-json", out_path=str(out), path=str(DSS))
    assert r["path"] == str(out)
    assert r["bytes_written"] == out.stat().st_size
    assert json.loads(out.read_text())["data_model"] == "ENGINEERING"
    with pytest.raises(ValueError):
        save_dist_case(to="pmd-json", out_path=str(out), path=str(DSS))
    r2 = save_dist_case(to="dss", out_path=str(out), path=str(DSS), overwrite=True)
    assert r2["bytes_written"] == out.stat().st_size
    assert out.read_text() == DSS.read_text()  # same-format echo is byte exact


# ---------------------------------------------------------------------------
# Display tool: PowerWorld .pwd one-line geometry, which travels separately
# from the network case (its own display API, not parse_file).
# ---------------------------------------------------------------------------

PWD = DATA / "powerworld" / "ACTIVSg200.pwd"


def test_read_display_file_decodes_pwd():
    from powerio.mcp.server import read_display_file

    d = read_display_file(str(PWD))
    assert d["kind"] == "powerworld"
    assert d["canvas_width"] > 0 and d["canvas_height"] > 0
    assert d["substations"]
    assert {"number", "name", "x", "y"} <= set(d["substations"][0])


def test_read_display_file_errors_map_cleanly(tmp_path):
    from powerio.mcp.server import read_display_file

    with pytest.raises(ValueError):  # FileNotFoundError → ValueError
        read_display_file(str(tmp_path / "nope.pwd"))
