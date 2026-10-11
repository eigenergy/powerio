"""Tests for the optional PowerIO MCP server."""

import asyncio
import io
import json
import os
import shutil
import subprocess
import sys
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


@pytest.fixture(autouse=True)
def configured_test_roots(monkeypatch, tmp_path):
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, os.pathsep.join((str(DATA), str(tmp_path))))


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
    assert selected["network"]["elements"]["buses"] == 9
    with pytest.raises(ValueError, match="BalancedNetwork"):
        server.calc_matrix("bprime", powerio_ir=time_series_powerio_ir)


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
    assert about["powerio_ir"]["schema"] == "pio-ir"
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


def test_matrix_response_names_every_axis():
    bprime = server.calc_matrix("bprime", path=str(DATA / "case9.m"))
    assert bprime["row_ids"] == bprime["col_ids"]
    assert len(bprime["row_ids"]) == 9
    assert bprime["skipped_branch_rows"] == []
    assert bprime["skip_zero_impedance"] is False

    ptdf = server.calc_matrix("ptdf", path=str(DATA / "case9.m"))
    assert ptdf["shape"] == [len(ptdf["row_ids"]), len(ptdf["col_ids"])]
    assert all(isinstance(identity, str) for identity in ptdf["row_ids"])
    assert ptdf["col_ids"] == bprime["col_ids"]

    lacpf = server.calc_matrix("lacpf", path=str(DATA / "case9.m"))
    assert len(lacpf["row_ids"]) == 18
    assert lacpf["row_ids"][0].endswith(":p") and lacpf["row_ids"][9].endswith(":q")
    assert lacpf["col_ids"][0].endswith(":vm") and lacpf["col_ids"][9].endswith(":va")


def test_matrix_tool_serves_the_dc_calculations_by_name():
    # case14 has 14 buses and 20 branches, so a transposed result changes the
    # shape; case9's 9 by 9 incidence would hide the swap.
    case14 = str(DATA / "case14.m")
    index_map = powerio.parse(case14).value.calc_dc_index_map()

    incidence = server.calc_matrix("incidence", path=case14)
    assert incidence["format"] == "coo"
    assert incidence["shape"] == [20, 14]
    assert incidence["row_ids"] == list(index_map["branch_ids"])
    assert incidence["col_ids"] == list(index_map["bus_ids"])

    susceptances = server.calc_matrix("branch_susceptances", path=case14)
    assert susceptances["format"] == "vector"
    assert susceptances["shape"] == [20]
    assert len(susceptances["data"]) == 20
    assert susceptances["row_ids"] == incidence["row_ids"]
    assert "col_ids" not in susceptances

    injection = server.calc_matrix(
        "bus_phase_shift_injection", path=case14, formula="reactance_only"
    )
    assert injection["formula"] == "reactance_only"
    assert injection["row_ids"] == incidence["col_ids"]


def _resistive_case_ir(resistance=0.1):
    document = json.loads(powerio.serialize(powerio.parse(DATA / "case9.m")).text)
    for branch in document["value"]["data"]["branches"]:
        branch["r"] = resistance
        branch["x"] = 0.0
    return json.dumps(document)


def test_adjacency_axes_do_not_require_dc_impedance():
    result = server.calc_matrix("adjacency", powerio_ir=_resistive_case_ir(0.0))
    assert result["shape"] == [9, 9]
    assert result["row_ids"] == result["col_ids"] == list(range(1, 10))
    assert result["nnz"] > 0


def test_multiconductor_operating_point_summary_does_not_request_a_balanced_network():
    document = json.loads(powerio.serialize(powerio.parse(DSS)).text)
    document["value"] = {
        "type": "powerio.OperatingPoint<powerio.MulticonductorNetwork>",
        "data": {"network": document["value"]["data"], "quantities": {}},
    }
    text = json.dumps(document)
    result = server.summarize(powerio_ir=text)
    assert result["operating_point"] is True
    assert result["network"] is None
    with pytest.raises(ValueError, match="BalancedNetwork"):
        server.calc_matrix("adjacency", powerio_ir=text)


@pytest.mark.parametrize("matrix,scheme", [
    ("bprime", "bx"), ("bdoubleprime", "xb"),
    ("admittance_real", "bx"), ("admittance_imag", "bx"), ("lacpf", "bx"),
])
def test_ac_axes_do_not_apply_the_dc_reactance_formula(matrix, scheme):
    result = server.calc_matrix(
        matrix, powerio_ir=_resistive_case_ir(), scheme=scheme, formula="reactance_only"
    )
    assert result["shape"] == [len(result["row_ids"]), len(result["col_ids"])]
    assert result["skipped_branch_rows"] == []


@pytest.mark.parametrize("matrix,scheme", [("bprime", "xb"), ("bdoubleprime", "bx")])
def test_fdpf_skipped_rows_follow_the_selected_scheme(matrix, scheme):
    result = server.calc_matrix(
        matrix, powerio_ir=_resistive_case_ir(), scheme=scheme, skip_zero_impedance=True
    )
    assert result["skipped_branch_rows"] == list(range(9))


def test_lacpf_axes_match_the_power_voltage_blocks():
    np = pytest.importorskip("numpy")
    sparse = pytest.importorskip("scipy.sparse")
    net = powerio.parse(DATA / "case9.m").value
    result = server.calc_matrix("lacpf", path=str(DATA / "case9.m"))
    matrix = sparse.coo_matrix((result["data"], (result["row"], result["col"])), shape=result["shape"])
    vm = np.linspace(-0.01, 0.02, 9)
    va = np.linspace(0.02, -0.03, 9)
    ybus = net.calc_admittance_matrix()
    expected = np.r_[ybus.real @ vm - ybus.imag @ va, -ybus.imag @ vm - ybus.real @ va]
    np.testing.assert_allclose(matrix @ np.r_[vm, va], expected)


def test_matrix_tool_rejects_skip_zero_impedance_where_it_would_be_ignored():
    for name in ("ptdf", "lodf", "adjacency", "weighted_laplacian"):
        with pytest.raises(ValueError, match="does not take skip_zero_impedance"):
            server.calc_matrix(
                name, path=str(DATA / "case9.m"), skip_zero_impedance=True
            )
    # The message names the DC calculations that do take it.
    with pytest.raises(ValueError, match="bus_phase_shift_injection"):
        server.calc_matrix(
            "ptdf", path=str(DATA / "case9.m"), skip_zero_impedance=True
        )
    # The flag still reaches the calculations that accept it.
    incidence = server.calc_matrix(
        "incidence", path=str(DATA / "case9.m"), skip_zero_impedance=True
    )
    assert incidence["skip_zero_impedance"] is True


def test_matrix_tool_computes_over_an_operating_point_entry(time_series_powerio_ir):
    matrix = server.calc_matrix(
        "bprime", powerio_ir=time_series_powerio_ir, time_index=1
    )
    assert matrix["selection"] == {"time_index": 1}
    assert matrix["shape"] == [9, 9]
    summary = server.summarize(powerio_ir=time_series_powerio_ir, time_index=1)
    assert summary["value_type"] == "OperatingPoint"
    assert summary["network"]["elements"]["buses"] == 9


LIMIT_ENVS = ("POWERIO_MAX_PRIMARY_BYTES", "POWERIO_MAX_REFERENCED_BYTES")


def test_server_start_defaults_unset_limits_to_256_mib(monkeypatch):
    environ = {"POWERIO_MAX_PRIMARY_BYTES": ""}
    server._apply_service_limits(environ)
    assert environ == {name: str(256 << 20) for name in LIMIT_ENVS}

    # Values the operator set stay as they are, valid or not; PowerIO
    # validates them when it reads a source.
    environ = {"POWERIO_MAX_PRIMARY_BYTES": "2048", "POWERIO_MAX_REFERENCED_BYTES": "x"}
    server._apply_service_limits(environ)
    assert environ == {
        "POWERIO_MAX_PRIMARY_BYTES": "2048",
        "POWERIO_MAX_REFERENCED_BYTES": "x",
    }

    # `main` applies the defaults to the process before it serves. Setting
    # each variable through monkeypatch first restores the original afterward.
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", "")
    monkeypatch.setenv("POWERIO_MAX_REFERENCED_BYTES", "4096")
    monkeypatch.setattr(sandbox, "_DEFAULT_ROOT", sandbox._DEFAULT_ROOT)
    served = []
    monkeypatch.setattr(server.mcp, "run", lambda *args, **kwargs: served.append(True))
    server.main()
    assert served == [True]
    assert os.environ["POWERIO_MAX_PRIMARY_BYTES"] == str(256 << 20)
    assert os.environ["POWERIO_MAX_REFERENCED_BYTES"] == "4096"


def test_importing_the_server_keeps_the_library_limits():
    code = (
        "import os; from powerio.mcp import server; "
        "print([os.environ.get(name) for name in "
        "('POWERIO_MAX_PRIMARY_BYTES', 'POWERIO_MAX_REFERENCED_BYTES')])"
    )
    env = {key: value for key, value in os.environ.items() if key not in LIMIT_ENVS}
    out = subprocess.run(
        [sys.executable, "-c", code], check=True, capture_output=True, text=True, env=env
    )
    assert out.stdout.strip() == "[None, None]"


@pytest.mark.parametrize(
    "value,limit",
    [(None, 256 << 20), ("", 256 << 20), ("0", 256 << 20), ("-1", 256 << 20),
     (" 7", 256 << 20), ("1e3", 256 << 20), ("١", 256 << 20), ("123", 123)],
)
def test_primary_limit_falls_back_to_the_service_default(monkeypatch, value, limit):
    if value is None:
        monkeypatch.delenv("POWERIO_MAX_PRIMARY_BYTES", raising=False)
    else:
        monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", value)
    assert server._primary_byte_limit() == limit


def test_inline_arguments_are_refused_past_the_primary_limit_before_parsing(
    monkeypatch,
):
    text = (DATA / "case9.m").read_text()
    powerio_ir = powerio.serialize(powerio.parse(DATA / "case9.m")).text
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", str(len(text.encode())))
    assert server.parse(content=text, format="matpower")["summary"]["elements"][
        "buses"
    ] == 9

    def refuse(*args, **kwargs):
        raise AssertionError("an oversize argument reached the reader")

    monkeypatch.setattr(powerio, "parse", refuse)
    monkeypatch.setattr(powerio, "deserialize", refuse)
    limit = len(text.encode()) - 1
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", str(limit))
    message = rf"`content` exceeds the {limit} byte .*POWERIO_MAX_PRIMARY_BYTES"
    with pytest.raises(ValueError, match=message):
        server.parse(content=text, format="matpower")
    with pytest.raises(ValueError, match=message):
        server.emit("psse", content=text, source_format="matpower")

    limit = len(powerio_ir.encode()) - 1
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", str(limit))
    message = rf"`powerio_ir` exceeds the {limit} byte .*POWERIO_MAX_PRIMARY_BYTES"
    for call in (
        lambda: server.summarize(powerio_ir=powerio_ir),
        lambda: server.diagnostics(powerio_ir),
        lambda: server.calc_matrix("bprime", powerio_ir=powerio_ir),
        lambda: server._to_balanced_tool(powerio_ir=powerio_ir),
    ):
        with pytest.raises(ValueError, match=message):
            call()


def test_inline_size_counts_utf8_bytes(monkeypatch):
    # Ten two-byte characters and one lone surrogate, which counts as the
    # three bytes it would take if it were encoded.
    value = "é" * 10 + "\ud800"
    monkeypatch.setattr(server, "_UTF8_COUNT_CHUNK", 3)
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", "23")
    server._check_inline_size("content", value)
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", "22")
    with pytest.raises(ValueError, match="`content` exceeds the 22 byte"):
        server._check_inline_size("content", value)
    # More characters than the limit has bytes is refused without counting,
    # and a limit of four bytes per character is accepted without counting.
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", "10")
    with pytest.raises(ValueError, match="exceeds the 10 byte"):
        server._check_inline_size("content", value)
    monkeypatch.setattr(server, "_UTF8_COUNT_CHUNK", None)
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", "44")
    server._check_inline_size("content", value)


def test_display_requires_a_regular_file_within_the_primary_limit(
    monkeypatch, tmp_path
):
    size = PWD.stat().st_size
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", str(size))
    assert server.display(str(PWD))["format"] == "powerworld-pwd"

    def refuse(*args, **kwargs):
        raise AssertionError("the display reader ran")

    monkeypatch.setattr(powerio, "parse_display", refuse)
    monkeypatch.setenv("POWERIO_MAX_PRIMARY_BYTES", str(size - 1))
    with pytest.raises(
        ValueError, match=rf"exceeds the {size - 1} byte .*POWERIO_MAX_PRIMARY_BYTES"
    ):
        server.display(str(PWD))
    with pytest.raises(ValueError, match="regular file"):
        server.display(str(tmp_path))
    if os.name != "nt":
        # A named pipe is refused from its metadata, without opening it.
        fifo = tmp_path / "display.pwd"
        os.mkfifo(fifo)
        with pytest.raises(ValueError, match="regular file"):
            server.display(str(fifo))
