"""Tests for the optional PowerIO MCP server."""

import asyncio
import io
import json
import shutil
from pathlib import Path

import pytest

pytest.importorskip("mcp", reason="powerio[mcp] not installed (needs Python 3.10+)")

from powerio.mcp import sandbox, server

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
DSS = DATA / "dist" / "micro" / "xfmr_single_phase.dss"
BMOPF = DATA / "dist" / "bmopf" / "example_ieee13.json"
PWD = DATA / "powerworld" / "ACTIVSg200.pwd"

LOWERABLE_DSS = """Clear
Set DefaultBaseFrequency=60
New Circuit.tiny basekv=12.47 pu=1.0 phases=3 bus1=src MVAsc3=2000 MVAsc1=2100
New Transformer.t1 phases=3 windings=2 buses=(src, sec) conns=(delta, wye) kvs=(12.47, 0.416) kvas=(500, 300) %Rs=(0.5, 0.5) xhl=6
New Load.l1 bus1=sec phases=3 conn=wye kv=0.416 kw=90 pf=0.95 model=1
Set VoltageBases=[12.47, 0.416]
"""


def test_tool_surface_uses_powerio_operations_and_types():
    tools = {tool.name: tool for tool in asyncio.run(server.mcp.list_tools())}
    assert set(tools) == {
        "parse",
        "emit",
        "summarize",
        "diagnostics",
        "to_normalized",
        "calc_matrix",
        "display",
        "to_balanced_report",
        "to_balanced",
        "about",
    }
    for removed in (
        "inspect",
        "list_states",
        "inspect_state",
        "export_state",
        "materialize_network",
    ):
        assert removed not in tools

    parse_properties = tools["parse"].input_schema["properties"]
    assert set(parse_properties) == {"path", "content", "format"}
    assert parse_properties["content"]["type"] == "string"
    assert parse_properties["content"]["default"] == ""
    assert "transport" not in parse_properties

    emit_properties = tools["emit"].input_schema["properties"]
    assert "powerio_ir" in emit_properties
    assert "source_format" in emit_properties
    assert "json" not in emit_properties
    assert "json_format" not in emit_properties
    assert "from_format" not in emit_properties

    matrix = tools["calc_matrix"].input_schema["properties"]["matrix"]
    assert matrix["enum"] == sorted(server._MATRIX_NAMES)


def test_parse_returns_powerio_ir_and_actual_value_type():
    parsed = server.parse(path=str(DATA / "case9.m"))

    assert parsed["value_type"] == "powerio.BalancedNetwork"
    assert "json" not in parsed and "json_format" not in parsed
    document = json.loads(parsed["powerio_ir"])
    assert document["schema"] == "pio-ir"
    module = powerio.deserialize(io.StringIO(parsed["powerio_ir"]))
    assert isinstance(module.value, powerio.BalancedNetwork)
    assert module.value.n_buses == 9


def test_powerio_ir_is_read_only_by_powerio_deserialize():
    powerio_ir = powerio.serialize(powerio.parse(DATA / "case9.m")).text
    summary = server.summarize(powerio_ir=powerio_ir)
    diagnostics = server.diagnostics(powerio_ir)
    emitted = server.emit("psse", powerio_ir=powerio_ir)

    assert summary["module_value_type"] == "powerio.BalancedNetwork"
    assert summary["elements"]["buses"] == 9
    assert diagnostics["value_type"] == "powerio.BalancedNetwork"
    assert emitted["text"].lstrip().startswith("0,")


def test_content_is_parsed_as_a_file_object_not_as_a_path():
    text = (DATA / "case9.m").read_text()
    parsed = server.parse(content=text, format="matpower")
    emitted = server.emit("matpower", content=text, source_format="matpower")
    assert parsed["summary"]["elements"]["buses"] == 9
    assert emitted["text"] == text


def test_bmopf_parse_returns_a_network_and_instance_construction_is_explicit():
    parsed = server.parse(path=str(BMOPF))
    module = powerio.deserialize(io.StringIO(parsed["powerio_ir"]))
    assert isinstance(module.value, powerio.MulticonductorNetwork)
    assert parsed["summary"]["electrical_model"] == "multiconductor"

    instance = module.to_mc_ac_opf_instance()
    assert isinstance(instance.value, powerio.McAcOpfInstance)


def test_distribution_network_uses_the_same_module_path():
    parsed = server.parse(path=str(DSS))
    summary = server.summarize(powerio_ir=parsed["powerio_ir"])
    assert parsed["value_type"] == "powerio.MulticonductorNetwork"
    assert summary["electrical_model"] == "multiconductor"
    assert summary["elements"]["buses"] > 0


def test_collection_summary_uses_normal_indexing_without_conversion(
    time_series_powerio_ir,
):
    collection = server.summarize(powerio_ir=time_series_powerio_ir)
    selected = server.summarize(powerio_ir=time_series_powerio_ir, time_index=1)

    assert collection["collection"] == "TimeSeries"
    assert collection["length"] == 2
    assert selected["selection"] == {"time_index": 1}
    assert selected["value_type"] == "OperatingPoint"
    assert "elements" not in selected
    with pytest.raises(ValueError, match="BalancedNetwork"):
        server.calc_matrix(
            "bprime", powerio_ir=time_series_powerio_ir, time_index=1
        )


def test_collection_selector_refuses_the_wrong_collection_operation(
    time_series_powerio_ir,
):
    with pytest.raises(ValueError, match="ScenarioSet"):
        server.summarize(powerio_ir=time_series_powerio_ir, scenario_id="base")


def test_emit_returns_artifact_inventory_and_fidelity(tmp_path):
    memory = server.emit("psse", path=str(DATA / "case9.m"))
    assert memory["layout"] == "file"
    assert memory["fidelity"] in {"canonical", "exact_same_format"}
    assert len(memory["artifacts"]) == 1
    assert memory["artifacts"][0]["text"] == memory["text"]

    destination = tmp_path / "case9.raw"
    written = server.emit(
        "psse", destination=str(destination), path=str(DATA / "case9.m")
    )
    assert written["path"] == str(destination)
    assert destination.read_text().lstrip().startswith("0,")
    with pytest.raises(ValueError, match="overwrite"):
        server.emit(
            "psse", destination=str(destination), path=str(DATA / "case9.m")
        )
    replaced = server.emit(
        "matpower",
        destination=str(destination),
        overwrite=True,
        path=str(DATA / "case9.m"),
    )
    assert replaced["path"] == str(destination)
    assert destination.read_text().lstrip().startswith("function mpc")


def test_directory_emit_is_staged_and_lists_files(tmp_path):
    destination = tmp_path / "pypsa"
    emitted = server.emit(
        "pypsa-csv", destination=str(destination), path=str(DATA / "case9.m")
    )
    assert emitted["layout"] == "directory"
    assert emitted["dir"] == str(destination)
    assert emitted["files"]
    assert all(Path(path).is_file() for path in emitted["files"])


def test_matrix_response_names_the_calculation():
    matrix = server.calc_matrix("bprime", path=str(DATA / "case9.m"))
    assert matrix["matrix"] == "bprime"
    assert matrix["shape"] == [9, 9]
    assert matrix["nnz"] > 0
    assert "kind" not in matrix


def test_matrix_names_and_unknown_name():
    assert server.calc_matrix("bdoubleprime", path=str(DATA / "case9.m"))[
        "matrix"
    ] == "bdoubleprime"
    with pytest.raises(ValueError, match="unknown matrix"):
        server.calc_matrix("b1", path=str(DATA / "case9.m"))


def test_to_normalized_returns_a_powerio_module():
    result = server.to_normalized(path=str(DATA / "case9.m"))
    module = powerio.deserialize(io.StringIO(result["powerio_ir"]))
    assert isinstance(module.value, powerio.BalancedNetwork)
    assert module.value.source_format == "normalized"


def test_to_balanced_report_and_conversion_use_module_methods():
    parsed = server.parse(content=LOWERABLE_DSS, format="dss")
    report = server._to_balanced_report_tool(powerio_ir=parsed["powerio_ir"])
    converted = server._to_balanced_tool(powerio_ir=parsed["powerio_ir"])

    assert report["ready"] is True
    module = powerio.deserialize(io.StringIO(converted["powerio_ir"]))
    assert isinstance(module.value, powerio.BalancedNetwork)


def test_display_decodes_powerworld_pwd():
    result = server.display(str(PWD))
    assert result["format"] == "powerworld-pwd"
    assert result["canvas"]["width"] > 0
    assert result["substations"]


def test_about_reports_exact_tool_list():
    about = server._about_tool()
    assert about["powerio_version"] == powerio.__version__
    # `about` passes the library's own version report through unchanged;
    # `scripts/wheel-smoke.py` owns pinning that report to the release version.
    assert about["powerio_ir"] == powerio.versions()["powerio_ir"]
    assert about["powerio_ir"]["name"] == "pio-ir"
    assert "parse" in about["tools"]
    assert "export_state" not in about["tools"]


def test_inputs_are_mutually_exclusive():
    with pytest.raises(ValueError, match="exactly one"):
        server.summarize()
    with pytest.raises(ValueError, match="exactly one"):
        server.summarize(path=str(DATA / "case9.m"), content="case")


def test_allowed_roots_restrict_read_and_write(monkeypatch, tmp_path):
    allowed = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed.mkdir()
    outside.mkdir()
    case = allowed / "case9.m"
    shutil.copy2(DATA / "case9.m", case)
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(allowed))

    assert server.summarize(path=str(case))["elements"]["buses"] == 9
    with pytest.raises(sandbox.PathNotAllowed):
        server.summarize(path=str(DATA / "case9.m"))
    with pytest.raises(sandbox.PathNotAllowed):
        server.emit("psse", destination=str(outside / "case.raw"), path=str(case))


def test_directory_input_preflight_refuses_a_symlink_escape(monkeypatch, tmp_path):
    allowed = tmp_path / "allowed"
    outside = tmp_path / "outside"
    dataset = allowed / "dataset"
    dataset.mkdir(parents=True)
    outside.mkdir()
    (outside / "secret.csv").write_text("secret")
    (dataset / "escape.csv").symlink_to(outside / "secret.csv")
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(allowed))

    with pytest.raises(sandbox.PathNotAllowed, match="outside"):
        server.parse(path=str(dataset), format="pypsa-csv")


def test_file_uri_paths_are_accepted(tmp_path):
    case = tmp_path / "case9.m"
    shutil.copy2(DATA / "case9.m", case)
    assert server.summarize(path=case.as_uri())["elements"]["buses"] == 9


def test_public_module_has_no_removed_mcp_callables():
    for name in (
        "inspect",
        "list_states",
        "inspect_state",
        "export_state",
        "convert",
        "save",
    ):
        assert not hasattr(server, name)
