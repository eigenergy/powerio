"""Tests for the optional MCP server (``powerio.mcp``).

Run with ``pytest python/tests`` after ``maturin develop`` and ``pip install
'.[mcp]'``. The whole module skips cleanly when the ``mcp`` extra is absent
(e.g. on Python 3.9, where the SDK is unavailable). The FastMCP-decorated tools
stay ordinary callables, so we exercise them in-process without a transport.

The surface was unified in 0.3.3 to the bare verbs (``convert``/``save``/
``summary``/``parse``/...); transmission and distribution share them, routed by
format. The old ``*_case`` and ``*_pypsa_csv_folder`` names are deprecated
aliases (see ``test_deprecated_aliases``), removed in 0.4.0.
"""

import json
import tempfile
from pathlib import Path

import pytest

pytest.importorskip("mcp", reason="powerio[mcp] not installed (needs Python 3.10+)")

import powerio  # noqa: E402
from powerio.mcp.server import convert, summary  # noqa: E402

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
DSS = DATA / "dist" / "micro" / "xfmr_single_phase.dss"

HAS_GRIDFM = bool(getattr(powerio._powerio, "_has_gridfm", False))
gridfm_only = pytest.mark.skipif(
    not HAS_GRIDFM, reason="extension built without the gridfm feature"
)


# --- summary ---------------------------------------------------------------


def test_summary_path():
    s = summary(path=str(DATA / "case9.m"))
    assert s["n_buses"] == 9
    assert s["source_format"] == "Matpower"
    assert s["base_mva"] == 100.0
    assert s["n_connected_components"] == 1


def test_summary_inline():
    s = summary(content=(DATA / "case9.m").read_text(), format="matpower")
    assert s["n_buses"] == 9


def test_summary_format_forwarded_for_path(tmp_path):
    bare = tmp_path / "case9_no_extension"
    bare.write_text((DATA / "case9.m").read_text())
    assert summary(path=str(bare), format="matpower")["n_buses"] == 9


def test_summary_json_extension_infers_transmission():
    assert summary(path=str(DATA / "pandapower" / "example.json"))["source_format"] == "PandapowerJson"


def test_summary_exactly_one_input():
    with pytest.raises(ValueError):
        summary()
    with pytest.raises(ValueError):
        summary(path="x", content="y")


def test_summary_distribution_dss():
    s = summary(path=str(DSS))
    assert s["source_format"] == "dss"
    assert s["n_buses"] == 2
    assert s["n_transformers"] == 1
    assert s["n_loads"] == 1
    # distribution counts + the same read_warnings key as transmission
    assert isinstance(s["read_warnings"], list)
    assert "base_mva" not in s


def test_summary_distribution_inline_requires_format():
    with pytest.raises(ValueError):
        summary(content=DSS.read_text())  # ambiguous, needs format
    assert summary(content=DSS.read_text(), format="dss")["n_buses"] == 2


# --- convert ---------------------------------------------------------------


def test_convert_transmission_path():
    r = convert(to="powermodels-json", path=str(DATA / "case30.m"))
    assert len(json.loads(r["text"])["bus"]) == 30
    assert isinstance(r["warnings"], list)


def test_convert_inline_requires_format():
    text = (DATA / "case30.m").read_text()
    with pytest.raises(ValueError):
        convert(to="psse", content=text)
    assert convert(to="psse", content=text, format="matpower")["text"]


def test_convert_format_alias():
    r = convert(to="pp", path=str(DATA / "case9.m"))
    assert json.loads(r["text"])["_class"] == "pandapowerNet"


def test_convert_reports_lossy_warnings():
    text = (DATA / "case30.m").read_text()
    lossy = convert(to="psse", content=text, format="matpower")
    assert any("cost" in w.lower() for w in lossy["warnings"])
    assert convert(to="powermodels-json", content=text, format="matpower")["warnings"] == []


def test_convert_exactly_one_input():
    with pytest.raises(ValueError):
        convert(to="matpower")
    with pytest.raises(ValueError):
        convert(to="matpower", path="x", content="y")


def test_convert_errors_map_cleanly():
    with pytest.raises(ValueError):  # FileNotFoundError → ValueError
        convert(to="psse", path=str(DATA / "does_not_exist.m"))
    with pytest.raises(ValueError):  # PowerIOError → ValueError
        convert(to="psse", content="not a case", format="matpower")


def test_convert_distribution_dss_to_pmd_and_bmopf():
    pmd = convert(to="pmd-json", path=str(DSS))
    assert json.loads(pmd["text"])["data_model"] == "ENGINEERING"
    bmopf = convert(to="bmopf-json", path=str(DSS))
    assert "bus" in json.loads(bmopf["text"])


def test_convert_distribution_same_format_echoes():
    r = convert(to="dss", path=str(DSS))
    assert r["text"] == DSS.read_text()
    assert r["warnings"] == []


def test_convert_distribution_inline_requires_format():
    with pytest.raises(ValueError):
        convert(to="pmd-json", content=DSS.read_text())
    r = convert(to="pmd-json", content=DSS.read_text(), format="dss")
    assert json.loads(r["text"])["data_model"] == "ENGINEERING"


def test_convert_rejects_cross_domain():
    # transmission source, distribution target (and the reverse)
    with pytest.raises(ValueError, match="boundary"):
        convert(to="dss", path=str(DATA / "case9.m"))
    with pytest.raises(ValueError, match="boundary"):
        convert(to="matpower", path=str(DSS))


def test_convert_rejects_directory_targets():
    with pytest.raises(ValueError, match="pypsa-csv"):
        convert(to="pypsa-csv", path=str(DATA / "case9.m"))
    with pytest.raises(ValueError, match="gridfm"):
        convert(to="gridfm", path=str(DATA / "case9.m"))


# --- parse / to_json / normalize (transmission only) -----------------------


def test_parse_json_round_trips():
    from powerio.mcp.server import parse

    r = parse(path=str(DATA / "case9.m"))
    assert r["summary"]["n_buses"] == 9
    assert powerio.from_json(r["json"]).n_buses == 9


def test_parse_surfaces_read_warnings():
    from powerio.mcp.server import parse

    r = parse(path=str(DATA / "pandapower" / "example.json"), format="pandapower-json")
    assert any("switch" in w for w in r["summary"]["read_warnings"])
    assert parse(path=str(DATA / "case9.m"))["summary"]["read_warnings"] == []


def test_parse_reads_pypsa_folder(tmp_path):
    net = powerio.parse_file(str(DATA / "case9.m"))
    folder = tmp_path / "case9-pypsa"
    net.write_pypsa_csv_folder(str(folder))
    from powerio.mcp.server import parse

    assert parse(path=str(folder))["summary"]["n_buses"] == 9


def test_to_json_accepted_downstream():
    from powerio.mcp.server import to_json, compute_matrix

    transport = to_json(path=str(DATA / "case9.m"))["json"]
    assert compute_matrix("bprime", json=transport)["shape"] == [9, 9]


def test_normalize_returns_dense_one_based_ids():
    from powerio.mcp.server import normalize

    case = powerio.from_json(normalize(path=str(DATA / "case9.m"))["json"])
    assert [b["id"] for b in case.buses] == list(range(1, 10))


def test_transport_tools_reject_distribution():
    from powerio.mcp.server import parse, to_json, normalize

    for tool in (parse, to_json, normalize):
        with pytest.raises(ValueError, match="transport"):
            tool(path=str(DSS))


# --- compute_matrix / dense_view (transmission only) -----------------------


def test_compute_matrix_kinds_and_plain_types():
    from powerio.mcp.server import compute_matrix

    m = compute_matrix("bprime", path=str(DATA / "case9.m"))
    assert m["shape"] == [9, 9] and m["format"] == "coo"
    assert type(m["data"][0]) is float and type(m["row"][0]) is int
    for kind in ("bdoubleprime", "ybus_real", "ybus_imag", "adjacency",
                 "ptdf", "lodf", "laplacian", "lacpf"):
        assert compute_matrix(kind, path=str(DATA / "case9.m"))["nnz"] > 0
    with pytest.raises(ValueError):
        compute_matrix("nope", path=str(DATA / "case9.m"))


def test_dense_view_counts():
    from powerio.mcp.server import dense_view

    d = dense_view(path=str(DATA / "case9.m"))
    assert d["n"] == 9 and d["m"] == 9 and d["base_mva"] == 100.0
    assert type(d["bus_ids"][0]) is int and type(d["is_radial"]) is bool


def test_matrix_tools_reject_distribution():
    from powerio.mcp.server import compute_matrix, dense_view

    with pytest.raises(ValueError, match="positive-sequence"):
        compute_matrix("bprime", path=str(DSS))
    with pytest.raises(ValueError, match="positive-sequence"):
        dense_view(path=str(DSS))


def test_matrix_json_transport_ignores_stray_format():
    # json is always the transmission transport; a stray `format` must not trip
    # the distribution guard.
    from powerio.mcp.server import to_json, compute_matrix

    transport = to_json(path=str(DATA / "case9.m"))["json"]
    assert compute_matrix("bprime", json=transport, format="dss")["shape"] == [9, 9]


def test_exactly_one_of_path_content_json():
    from powerio.mcp.server import compute_matrix, dense_view, parse

    with pytest.raises(ValueError):
        parse()
    with pytest.raises(ValueError):
        compute_matrix("bprime", path="x", json="{}")
    with pytest.raises(ValueError):
        dense_view(path="x", content="y")


def test_wrong_schema_json_maps_cleanly():
    from powerio.mcp.server import compute_matrix

    for bad in ("{}", "[]", "null", '{"buses": "nope"}'):
        with pytest.raises(ValueError, match="parse failed"):
            compute_matrix("bprime", json=bad)


# --- save (text file, distribution file, pypsa folder) ---------------------


def test_save_transmission_file_and_overwrite(tmp_path):
    from powerio.mcp.server import save

    out = tmp_path / "case9.json"
    r = save(to="powermodels-json", out_path=str(out), path=str(DATA / "case9.m"))
    assert r["path"] == str(out)
    assert r["bytes_written"] == out.stat().st_size
    assert len(json.loads(out.read_text())["bus"]) == 9
    with pytest.raises(ValueError):
        save(to="powermodels-json", out_path=str(out), path=str(DATA / "case9.m"))
    r2 = save(to="matpower", out_path=str(out), path=str(DATA / "case9.m"), overwrite=True)
    assert r2["bytes_written"] == out.stat().st_size


def test_save_keeps_read_warnings_on_echo(tmp_path):
    from powerio.mcp.server import save

    out = tmp_path / "echo.json"
    saved = save(to="pandapower-json", out_path=str(out), path=str(DATA / "pandapower" / "example.json"))
    assert any("switch" in w for w in saved["warnings"]), saved["warnings"]
    assert convert(to="pandapower-json", path=str(DATA / "pandapower" / "example.json"))["warnings"] == []


def test_save_accepts_json_transport(tmp_path):
    from powerio.mcp.server import save, to_json

    transport = to_json(path=str(DATA / "case9.m"))["json"]
    out = tmp_path / "case9.m"
    save(to="matpower", out_path=str(out), json=transport)
    assert powerio.parse_file(out).n_buses == 9


def test_save_distribution_dss_and_inline(tmp_path):
    from powerio.mcp.server import save

    out = tmp_path / "feeder.json"
    r = save(to="pmd-json", out_path=str(out), path=str(DSS))
    assert r["bytes_written"] == out.stat().st_size
    assert json.loads(out.read_text())["data_model"] == "ENGINEERING"
    # inline content path (the OpenDSS on-ramp uses this)
    dss_out = tmp_path / "feeder.dss"
    r2 = save(to="dss", out_path=str(dss_out), content=DSS.read_text(), format="dss")
    assert dss_out.read_text() == DSS.read_text()  # same-format echo is byte exact
    assert r2["bytes_written"] == dss_out.stat().st_size
    with pytest.raises(ValueError):  # inline content needs a format
        save(to="dss", out_path=str(tmp_path / "x.dss"), content=DSS.read_text())


def test_save_distribution_rejects_json_transport(tmp_path):
    from powerio.mcp.server import save

    with pytest.raises(ValueError, match="transport"):
        save(to="dss", out_path=str(tmp_path / "x.dss"), json="{}")


def test_save_pypsa_csv_folder(tmp_path):
    from powerio.mcp.server import save, summary as _summary

    out_dir = tmp_path / "pypsa_csv"
    w = save(to="pypsa-csv", out_path=str(out_dir), path=str(DATA / "case9.m"))
    assert w["files"], w
    assert (out_dir / "buses.csv").exists()
    assert _summary(path=str(out_dir))["n_buses"] == 9


def test_save_gridfm_rejected():
    from powerio.mcp.server import save

    with pytest.raises(ValueError, match="gridfm"):
        save(to="gridfm", out_path="x", path=str(DATA / "case9.m"))


def test_save_pypsa_keeps_read_warnings(tmp_path):
    # The pypsa-csv folder write reports source read warnings end to end, like
    # the text branch (its docstring promises this).
    from powerio.mcp.server import save

    w = save(to="pypsa-csv", out_path=str(tmp_path / "p"),
             path=str(DATA / "pandapower" / "example.json"))
    assert any("switch" in s for s in w["warnings"]), w["warnings"]


def test_save_rejects_cross_domain(tmp_path):
    from powerio.mcp.server import save

    with pytest.raises(ValueError, match="boundary"):
        save(to="matpower", out_path=str(tmp_path / "x.m"), path=str(DSS))


# --- gridfm (its own tools) ------------------------------------------------


@gridfm_only
def test_gridfm_round_trip(tmp_path):
    from powerio.mcp.server import read_gridfm, write_gridfm

    out_dir = tmp_path / "gfm"
    assert write_gridfm(str(out_dir), path=str(DATA / "case9.m"))["files"]
    r = read_gridfm(str(out_dir))
    assert r["summary"]["n_buses"] == 9 and r["scenario"] == 0


@gridfm_only
def test_read_gridfm_missing_dir_maps_cleanly(tmp_path):
    from powerio.mcp.server import read_gridfm

    with pytest.raises(ValueError):
        read_gridfm(str(tmp_path / "nope"))


# --- display ---------------------------------------------------------------

PWD = DATA / "powerworld" / "ACTIVSg200.pwd"


def test_read_display_file_decodes_pwd():
    from powerio.mcp.server import read_display_file

    d = read_display_file(str(PWD))
    assert d["kind"] == "powerworld"
    assert d["canvas_width"] > 0 and d["substations"]
    assert {"number", "name", "x", "y"} <= set(d["substations"][0])


def test_read_display_file_errors_map_cleanly(tmp_path):
    from powerio.mcp.server import read_display_file

    with pytest.raises(ValueError):
        read_display_file(str(tmp_path / "nope.pwd"))
    bad = tmp_path / "bad.pwd"
    bad.write_bytes(b"not a powerworld display")
    with pytest.raises(ValueError):
        read_display_file(str(bad))


# --- tool surface + deprecated aliases -------------------------------------


def test_tool_surface_parity():
    # The PowerMCP bundle re-exports this server (powerio/powerio_mcp.py in
    # Power-Agent/PowerMCP); powerio.mcp.server is canonical. A tool added or
    # removed here fails this test until the set, and the PowerMCP re-export,
    # move with it.
    import asyncio

    from powerio.mcp import server

    names = {t.name for t in asyncio.run(server.mcp.list_tools())}
    canonical = {
        "convert", "save", "summary", "parse", "to_json", "normalize",
        "compute_matrix", "dense_view", "read_gridfm", "write_gridfm",
        "read_display_file",
    }
    deprecated = {
        "convert_case", "save_case", "case_summary", "parse_case",
        "normalize_case", "case_to_json",
        "read_pypsa_csv_folder", "write_pypsa_csv_folder",
    }
    assert names == canonical | deprecated


def test_deprecated_aliases_forward(tmp_path):
    from powerio.mcp import server

    assert server.summary(path=str(DATA / "case9.m")) == server.case_summary(path=str(DATA / "case9.m"))
    assert (server.convert(to="psse", path=str(DATA / "case9.m"))
            == server.convert_case(to="psse", path=str(DATA / "case9.m")))
    # convert_case keeps the old `from_` spelling
    text = (DATA / "case30.m").read_text()
    assert (server.convert_case(to="psse", content=text, from_="matpower")
            == server.convert(to="psse", content=text, format="matpower"))
    # write_pypsa_csv_folder forwards to save(to="pypsa-csv")
    out_dir = tmp_path / "pypsa"
    w = server.write_pypsa_csv_folder(str(out_dir), path=str(DATA / "case9.m"))
    assert (out_dir / "buses.csv").exists() and w["files"]
    r = server.read_pypsa_csv_folder(str(out_dir))
    assert r["summary"]["n_buses"] == 9 and isinstance(r["warnings"], list)
    for name in ("parse_case", "save_case", "normalize_case", "case_to_json"):
        assert hasattr(server, name)


def test_unreadable_file_maps_cleanly(tmp_path):
    import os
    import sys

    if sys.platform == "win32" or os.geteuid() == 0:
        pytest.skip("permission bits are not enforceable here")
    locked = tmp_path / "locked.m"
    locked.write_text("function mpc = x\n")
    locked.chmod(0o000)
    try:
        with pytest.raises(ValueError, match="cannot read file"):
            convert(to="psse", path=str(locked))
        with pytest.raises(ValueError, match="cannot read file"):
            summary(path=str(locked))
    finally:
        locked.chmod(0o644)
