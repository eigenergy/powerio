"""Tests for the optional MCP server (``powerio.mcp``)."""

import asyncio
import json
from pathlib import Path

import pytest

pytest.importorskip("mcp", reason="powerio[mcp] not installed (needs Python 3.10+)")

import powerio  # noqa: E402
from powerio.mcp import server  # noqa: E402

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
DSS = DATA / "dist" / "micro" / "xfmr_single_phase.dss"
BMOPF = DATA / "dist" / "bmopf" / "example_ieee13.json"
PMD = DATA / "dist" / "pmd" / "ieee13.json"
PWD = DATA / "powerworld" / "ACTIVSg200.pwd"
MINIMAL_BMOPF = '{"bus":{"a":{"terminal_names":["1"]}}}'

HAS_GRIDFM = bool(getattr(powerio._powerio, "_has_gridfm", False))
gridfm_only = pytest.mark.skipif(
    not HAS_GRIDFM, reason="extension built without the gridfm feature"
)


def test_tool_surface_is_semantic():
    names = {t.name for t in asyncio.run(server.mcp.list_tools())}
    assert names == {
        "convert",
        "save",
        "summary",
        "parse",
        "normalize",
        "matrix",
        "display",
    }


def test_summary_transmission_schema():
    s = server.summary(path=str(DATA / "case9.m"))
    assert s["domain"] == "transmission"
    assert s["json_format"] == "powerio-json"
    assert s["source_format"] == "Matpower"
    assert s["base_mva"] == 100.0
    assert s["elements"]["buses"] == 9
    assert s["elements"]["branches"] == 9
    assert s["topology"]["connected_components"] == 1
    assert s["topology"]["reference_buses"] == [0]
    assert s["warnings"] == []


def test_summary_distribution_schema_and_json_sniffing():
    for path in (DSS, BMOPF, PMD):
        s = server.summary(path=str(path))
        assert s["domain"] == "distribution"
        assert s["json_format"] == "bmopf-json"
        assert s["elements"]["buses"] > 0
        assert s["elements"]["sources"] >= 0
        assert s["topology"]["connected_components"] is None


def test_distribution_aliases_route_to_core_parser():
    text = DSS.read_text()
    for fmt in ("dss", "opendss"):
        assert server.summary(content=text, format=fmt)["domain"] == "distribution"
    assert json.loads(server.convert(to="pmd", path=str(DSS))["text"])["data_model"] == "ENGINEERING"
    assert "bus" in json.loads(server.convert(to="bmopf", path=str(DSS))["text"])


def test_parse_transmission_transport_round_trip(tmp_path):
    parsed = server.parse(path=str(DATA / "case9.m"))
    assert parsed["domain"] == "transmission"
    assert parsed["json_format"] == "powerio-json"
    assert powerio.from_json(parsed["json"]).n_buses == 9
    assert server.summary(json=parsed["json"], json_format="powerio-json")[
        "elements"
    ]["buses"] == 9
    assert server.summary(json=parsed["json"], json_format="powerio_json")[
        "elements"
    ]["buses"] == 9

    out = tmp_path / "case9.m"
    server.save(to="matpower", out_path=str(out), json=parsed["json"], json_format=parsed["json_format"])
    assert powerio.parse_file(out).n_buses == 9


def test_parse_distribution_uses_bmopf_transport(tmp_path):
    parsed = server.parse(path=str(DSS))
    assert parsed["domain"] == "distribution"
    assert parsed["json_format"] == "bmopf-json"
    doc = json.loads(parsed["json"])
    assert "bus" in doc and "voltage_source" in doc
    assert server.summary(json=parsed["json"], json_format="bmopf-json")[
        "elements"
    ]["sources"] >= 1

    out = tmp_path / "feeder.dss"
    server.save(to="dss", out_path=str(out), json=parsed["json"], json_format=parsed["json_format"])
    assert "new circuit" in out.read_text().lower()


def test_minimal_bmopf_json_routes_without_format(tmp_path):
    parsed = server.parse(content=MINIMAL_BMOPF)
    assert parsed["domain"] == "distribution"
    assert parsed["json_format"] == "bmopf-json"
    assert parsed["summary"]["elements"]["buses"] == 1

    s = server.summary(content=MINIMAL_BMOPF)
    assert s["domain"] == "distribution"

    out = tmp_path / "minimal.json"
    server.save(to="bmopf-json", out_path=str(out), json=MINIMAL_BMOPF)
    assert json.loads(out.read_text())["bus"]["a"]["terminal_names"] == ["1"]


def test_powermodels_json_still_routes_as_transmission():
    pm = powerio.parse_file(str(DATA / "case9.m")).to_format("powermodels-json").text
    parsed = server.parse(content=pm)
    assert parsed["domain"] == "transmission"
    assert parsed["json_format"] == "powerio-json"
    assert parsed["summary"]["elements"]["buses"] == 9


def test_normalize_rejects_distribution():
    with pytest.raises(ValueError, match="not defined for distribution"):
        server.normalize(path=str(DSS))


def test_parse_reads_pypsa_folder(tmp_path):
    net = powerio.parse_file(str(DATA / "case9.m"))
    folder = tmp_path / "case9-pypsa"
    net.write_pypsa_csv_folder(str(folder))

    parsed = server.parse(path=str(folder))
    assert parsed["summary"]["domain"] == "transmission"
    assert parsed["summary"]["elements"]["buses"] == 9


@gridfm_only
def test_gridfm_routes_through_generic_verbs(tmp_path):
    out_dir = tmp_path / "gfm"
    write = server.save(to="gridfm", out_path=str(out_dir), path=str(DATA / "case9.m"))
    assert write["files"]

    parsed = server.parse(path=str(out_dir), format="gridfm", options={"scenario": 0})
    assert parsed["summary"]["domain"] == "transmission"
    assert parsed["summary"]["elements"]["buses"] == 9

    converted = server.convert(to="matpower", path=str(out_dir), format="gridfm")
    assert "mpc.bus" in converted["text"]


def test_matrix_kinds_aliases_and_errors():
    m = server.matrix("b", path=str(DATA / "case9.m"))
    assert m["kind"] == "bprime"
    assert m["shape"] == [9, 9]
    assert type(m["data"][0]) is float and type(m["row"][0]) is int

    for alias, canonical in (
        ("b2", "bdoubleprime"),
        ("g", "ybus_real"),
        ("negB", "ybus_imag"),
        ("adj", "adjacency"),
        ("ptdf", "ptdf"),
        ("lodf", "lodf"),
        ("laplacian", "laplacian"),
        ("lacpf", "lacpf"),
    ):
        assert server.matrix(alias, path=str(DATA / "case9.m"))["kind"] == canonical

    with pytest.raises(ValueError, match="bprime"):
        server.matrix("nope", path=str(DATA / "case9.m"))
    with pytest.raises(ValueError, match="transmission"):
        server.matrix("b", path=str(DSS))


def test_bad_json_transport_maps_cleanly():
    for bad in ("{}", "[]", "null", '{"buses": "nope"}'):
        with pytest.raises(ValueError, match="parse failed"):
            server.matrix("bprime", json=bad, json_format="powerio-json")


def test_save_text_folder_and_overwrite(tmp_path):
    out = tmp_path / "case9.json"
    r = server.save(to="powermodels-json", out_path=str(out), path=str(DATA / "case9.m"))
    assert r["path"] == str(out)
    assert r["bytes_written"] == out.stat().st_size
    with pytest.raises(ValueError):
        server.save(to="powermodels-json", out_path=str(out), path=str(DATA / "case9.m"))
    server.save(to="matpower", out_path=str(out), path=str(DATA / "case9.m"), overwrite=True)

    folder = tmp_path / "pypsa"
    w = server.save(to="pypsa-csv", out_path=str(folder), path=str(DATA / "case9.m"))
    assert w["files"] and (folder / "buses.csv").exists()


def test_file_uri_paths_are_accepted(tmp_path):
    source_uri = (DATA / "case9.m").as_uri()
    assert server.summary(path=source_uri)["elements"]["buses"] == 9

    out = tmp_path / "case9.json"
    server.save(to="powermodels-json", out_path=out.as_uri(), path=source_uri)
    assert json.loads(out.read_text())["name"] == "case9"


def test_mcp_allowed_roots_restrict_filesystem_paths(monkeypatch, tmp_path):
    local_case = tmp_path / "case9.m"
    local_case.write_text((DATA / "case9.m").read_text())
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOTS", str(tmp_path))

    assert server.summary(path=str(local_case))["elements"]["buses"] == 9
    with pytest.raises(ValueError, match="outside allowed MCP roots"):
        server.summary(path=str(DATA / "case9.m"))


def test_display_decodes_powerworld_pwd():
    d = server.display(str(PWD))
    assert d["domain"] == "display"
    assert d["source_format"] == "powerworld-pwd"
    assert d["canvas"]["width"] > 0
    assert d["substations"]
    assert {"number", "name", "x", "y"} <= set(d["substations"][0])


def test_display_errors_map_cleanly(tmp_path):
    with pytest.raises(ValueError):
        server.display(str(tmp_path / "nope.pwd"))
    bad = tmp_path / "bad.pwd"
    bad.write_bytes(b"not a powerworld display")
    with pytest.raises(ValueError):
        server.display(str(bad))


def test_compatibility_aliases_are_not_tools(tmp_path):
    assert server.case_summary(path=str(DATA / "case9.m")) == server.summary(path=str(DATA / "case9.m"))
    assert server.compute_matrix("b", path=str(DATA / "case9.m"))["kind"] == "bprime"
    out_dir = tmp_path / "pypsa"
    assert server.write_pypsa_csv_folder(str(out_dir), path=str(DATA / "case9.m"))["files"]
