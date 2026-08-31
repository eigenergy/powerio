"""Tests for the powerio Python bindings.

Run with `pytest python/tests` after `maturin develop`. The matrix and graph
tests need the optional extras: `pip install '.[all]'`.
"""

import ast
import json
import math
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
import pytest
import scipy.io
import scipy.sparse as sp

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"


def _parse(
    source,
    format=None,
    *,
    include_root=None,
    value_type=None,
    name=None,
):
    """Exercise the canonical path or text parser for test fixtures."""
    if isinstance(source, (bytes, bytearray, memoryview)):
        data = bytes(source)
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            suffix = {
                "pwb": ".pwb",
                "powerworld-pwb": ".pwb",
            }.get(format, ".bin")
            with tempfile.NamedTemporaryFile(suffix=suffix) as handle:
                handle.write(data)
                handle.flush()
                return powerio.parse_file(
                    handle.name,
                    format,
                    value_type=value_type,
                )
        return powerio.parse_text(
            text,
            name=name or "fixture",
            format=format,
            value_type=value_type,
        )
    return powerio.parse_file(
        source,
        format,
        include_root=include_root,
        value_type=value_type,
    )


def parse_text(text, format):
    return powerio.parse_text(
        text,
        name="fixture",
        format=format,
        value_type=powerio.BalancedNetwork,
    ).value


def _emit_value(value, format, destination=None):
    return powerio.PioModule.from_value(value).emit(format, destination)


def _emit_module(module):
    return module.emit("pio-json").text


def _parse_module(text):
    return powerio.parse_text(text, name="module.pio.json")


def _emit_file(path, format, from_=None, destination=None, **_options):
    module = powerio.parse_file(path, from_)
    return module.emit(format, destination)


def _emit_text(text, format, from_="matpower", *, name="fixture", **_options):
    module = powerio.parse_text(text, name=name, format=from_)
    return module.emit(format)


SMALL = ["case9", "case30"]

# A 3-bus case authored inline so tests can reach paths the vendored fixtures
# don't cover (no generators, two reference buses, an out-of-service branch).
# bus types: 1=PQ, 2=PV, 3=ref. Branch 1->2->3 radial.
TINY = """function mpc = tiny
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t90\t30\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t3\t2\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t250\t250\t250\t0\t0\t1\t-360\t360;
\t2\t3\t0.01\t0.1\t0\t250\t250\t250\t0\t0\t1\t-360\t360;
];
mpc.gen = [
\t1\t0\t0\t300\t-300\t1\t100\t1\t250\t10;
];
mpc.gencost = [
\t2\t0\t0\t3\t0.01\t5\t0;
];
"""

PSSE_START_OF_MARKERS = """0, 100.00, 33, 0, 0, 60.00 / synthetic v33 export
CASE
COMMENT
1,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
2,'BUS2        ', 230.0000,1,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / End of Bus Data, Start of Load Data
2,'1 ',1,1,1,10.0,5.0
0 / End of Load Data, Start of Fixed Shunt Data
0 / End of Fixed Shunt Data, Start of Gen Data
1,'1 ',50.0,5.0,20.0,-10.0,1.0,0,100.0,0.0,1.0,0.0,0.0,1.0,1,100.0,80.0,10.0
0 / End of Gen Data, Start of Branch Data
1,2,'1 ',0.01,0.05,0.001,100.0,90.0,80.0,0.0,0.0,0.0,0.0,1,1,0.0,1,1
0 / End of Branch Data, Start of Transformer Data
0 / End of Transformer Data, Start of Area Interchange Data
Q
"""


def load(name):
    return powerio.parse_file(
        DATA / f"{name}.m", value_type=powerio.BalancedNetwork
    ).value


def is_symmetric(m, tol=1e-9):
    return (abs(m - m.T) > tol).nnz == 0


def id_to_dense(case):
    return {bus["id"]: i for i, bus in enumerate(case.buses)}


@pytest.fixture(scope="module")
def case9():
    return load("case9")


# --- parsing & metadata -------------------------------------------------


def test_parse_metadata(case9):
    assert case9.name == "case9"
    assert case9.n_buses == 9
    assert case9.n_branches == 9
    assert case9.n_generators == 3
    assert case9.base_mva == 100.0
    assert not case9.is_radial  # case9 is meshed
    assert case9.n_islands == 1


def test_preferred_balanced_names_and_complete_tables():
    net = load("api_conformance")
    assert net.n_generators == net.n_generators == len(net.generators) == 2
    assert net.n_islands == net.n_islands == 1
    assert net.base_frequency == 60.0
    for table, count in [
        ("storage", "n_storage"),
        ("hvdc", "n_hvdc"),
        ("transformers_3w", "n_transformers_3w"),
        ("areas", "n_areas"),
    ]:
        rows = getattr(net, table)
        assert isinstance(rows, list)
        assert len(rows) == getattr(net, count)


def test_public_type_is_balanced_network(case9):
    assert isinstance(case9, powerio.BalancedNetwork)
    assert "BalancedNetwork" in powerio.__all__
    assert "EmitResult" in powerio.__all__
    assert not hasattr(powerio, "Conversion")
    assert not hasattr(powerio, "Case")
    # The 0.8 bridge alias is gone at 1.0.0.
    assert not hasattr(powerio, "Network")
    assert "Network" not in powerio.__all__
    assert repr(case9).startswith("BalancedNetwork(")


def test_features_reports_compiled_in_surface():
    features = powerio.features()
    assert features.keys() == {"arrow", "matrix", "gridfm", "dist", "prob"}
    assert all(isinstance(v, bool) for v in features.values())
    # matrix/dist/prob are unconditional dependencies of the extension; arrow
    # has no Python binding surface at all.
    assert features["matrix"] is True
    assert features["dist"] is True
    assert features["prob"] is True
    assert features["arrow"] is False
    assert "features" in powerio.__all__


def test_private_extension_stub_hides_legacy_boundary_helpers():
    stub_path = Path(powerio.__file__).with_name("_powerio.pyi")
    tree = ast.parse(stub_path.read_text())
    exports = next(
        ast.literal_eval(node.value)
        for node in tree.body
        if isinstance(node, ast.Assign)
        and any(
            isinstance(target, ast.Name) and target.id == "__all__"
            for target in node.targets
        )
    )
    hidden_functions = {
        "convert_file",
        "convert_str",
        "dist_convert_file",
        "dist_convert_str",
        "dist_parse_file",
        "dist_parse_str",
        "parse_display_bytes",
        "read_gridfm",
        "read_gridfm_scenarios",
        "write_gridfm_batch",
    }
    assert hidden_functions.isdisjoint(exports)

    module_class = next(
        node
        for node in tree.body
        if isinstance(node, ast.ClassDef) and node.name == "_PioModule"
    )
    stubbed_methods = {
        node.name
        for node in module_class.body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    }
    assert {
        "from_file",
        "from_str",
        "from_bytes",
        "from_json",
        "to_json",
        "to_format",
        "write_file",
        "diagnostics",
    }.isdisjoint(stubbed_methods)

    for class_name in ("_BalancedNetwork", "_MulticonductorNetwork"):
        value_class = next(
            node
            for node in tree.body
            if isinstance(node, ast.ClassDef) and node.name == class_name
        )
        value_methods = {
            node.name
            for node in value_class.body
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        }
        assert {"to_format", "to_canonical_format", "write_file"}.isdisjoint(
            value_methods
        )

    # The implementation retains these operations only for the high level
    # wrapper's private boundary; users discover the typed facade instead.
    assert hasattr(powerio._powerio._PioModule, "from_file")
    assert not hasattr(powerio, "convert_file")


def test_parse_infers_format_from_extension():
    # parse_file dispatches on the extension; a .m file lands on MATPOWER.
    case = powerio.parse_file(
        DATA / "case9.m", value_type=powerio.BalancedNetwork
    ).value
    assert case.n_buses == 9
    assert case.source_format == "matpower"


def test_parse_file_and_parse_text_are_the_only_public_parse_entries():
    path = DATA / "case9.m"
    assert powerio.parse_file(path, format="matpower").kind == "balanced_network"
    with pytest.raises(TypeError, match="path must be text, not bytes"):
        powerio.parse_file(path.read_bytes(), format="matpower")
    with pytest.raises(TypeError, match="path must be text, not bytes"):
        powerio.parse_file(str(path).encode(), format="matpower")
    in_memory = powerio.parse_text(path.read_text(), name="case9.m", format="matpower")
    assert in_memory.kind == "balanced_network"
    assert in_memory.value.n_buses == 9
    assert not hasattr(powerio, "parse")
    for name in ("from_file", "from_str", "from_bytes", "from_json"):
        assert not hasattr(powerio.PioModule, name)
    assert not hasattr(powerio.PioModule, "to_json")


def test_opfdata_parses_to_its_solved_calculation():
    path = DATA / "opfdataset" / "example_0.json"
    module = powerio.parse_file(path)
    assert module.kind == "ac_opf_solution"
    with pytest.raises(powerio.PowerIODataError, match="ac_opf_solution"):
        module.as_balanced_network()

    # .value reads back the typed instance/solution wrapper for a kind with
    # no dedicated handle type: a thin object holding the owning module.
    value = module.value
    assert isinstance(value, powerio.AcOpfSolution)
    assert value.kind == "ac_opf_solution"
    assert value.module is module

    with pytest.raises(powerio.PowerIOError, match="no matpower writer"):
        module.emit("matpower")
    assert (
        json.loads(module.emit("pio-json").text)["value"]["kind"] == "ac_opf_solution"
    )


def test_value_type_is_an_assertion_returning_the_module():
    # value_type narrows what parse_file() asserts, not what it returns: the call
    # always hands back the PioModule, and .value reads the typed value.
    module = _parse(DATA / "case9.m", value_type=powerio.BalancedNetwork)
    assert isinstance(module, powerio.PioModule)
    assert module.kind == "balanced_network"
    net = module.value
    assert isinstance(net, powerio.BalancedNetwork)
    assert net.n_buses == 9

    # None (the default) and PioModule itself both skip the assertion.
    assert _parse(DATA / "case9.m").kind == "balanced_network"
    assert (
        _parse(DATA / "case9.m", value_type=powerio.PioModule).kind
        == "balanced_network"
    )

    solved = _parse(
        DATA / "opfdataset" / "example_0.json",
        value_type=powerio.AcOpfSolution,
    )
    assert solved.kind == "ac_opf_solution"
    assert isinstance(solved.value, powerio.AcOpfSolution)


def test_value_type_mismatch_names_both_kinds():
    path = DATA / "dist" / "micro" / "xfmr_single_phase.dss"
    with pytest.raises(ValueError) as excinfo:
        _parse(path, value_type=powerio.BalancedNetwork)
    message = str(excinfo.value)
    # The detected kind and the requested type both appear, so the caller
    # sees what it got and what it asked for.
    assert "multiconductor_network" in message
    assert "BalancedNetwork" in message

    with pytest.raises(ValueError, match="AcOpfSolution"):
        _parse(DATA / "case9.m", value_type=powerio.AcOpfSolution)


def test_native_diagnostics_fields_and_types():
    # The pandapower fixture carries a switch table the reader cannot model,
    # so the parse reports it; read that finding as a native Diagnostic from
    # the PioModule directly.
    module = _parse(DATA / "pandapower" / "example.json")
    diagnostics = module.diagnostics
    assert diagnostics
    with pytest.raises(TypeError):
        diagnostics()
    for d in diagnostics:
        assert isinstance(d, powerio.Diagnostic)
        assert isinstance(d.code, str) and d.code
        assert d.severity in ("error", "warning", "remark", "note")
        assert isinstance(d.message, str) and d.message
        assert d.id is None or isinstance(d.id, str)
        assert d.target is None or isinstance(d.target, str)
        assert d.suggested_action is None or isinstance(d.suggested_action, str)
        assert isinstance(d.related, list)
        assert all(isinstance(r, str) for r in d.related)
        assert isinstance(d.spans, list)
        for span in d.spans:
            assert isinstance(span, powerio.SourceSpan)
            assert isinstance(span.source, str)
            assert isinstance(span.byte_start, int) and isinstance(span.byte_end, int)
            assert span.byte_start <= span.byte_end
        assert d.details is None or isinstance(d.details, dict)

    switch = next(d for d in diagnostics if "switch" in d.message)
    assert switch.severity == "warning"
    # The Rust side renders __repr__ with Debug-formatted (double quoted)
    # strings, not Python's repr() quoting.
    assert repr(switch) == (
        f'Diagnostic(code="{switch.code}", severity="{switch.severity}", '
        f'message="{switch.message}")'
    )
    assert str(switch) == f"WARNING {switch.code}: {switch.message}"


def test_network_diagnostics_belong_only_to_the_module(case9):
    assert not hasattr(case9, "diagnostics")
    assert not hasattr(case9, "read_warnings")


def test_new_typed_value_classes_are_exported():
    for name in [
        "TimeSeries",
        "ScenarioSet",
        "DcPfInstance",
        "AcPfInstance",
        "DcOpfInstance",
        "AcOpfInstance",
        "McAcPfInstance",
        "McAcOpfInstance",
        "AcScucInstance",
        "DcPfSolution",
        "AcPfSolution",
        "DcOpfSolution",
        "AcOpfSolution",
        "McAcPfSolution",
        "McAcOpfSolution",
        "AcScucSolution",
        "UnknownValue",
    ]:
        assert name in powerio.__all__, name
        assert hasattr(powerio, name), name


def test_parse_powerworld_display_file():
    path = DATA / "powerworld" / "ACTIVSg200.pwd"
    parsed = powerio.parse_display_file(path)

    assert not hasattr(powerio, "parse_display_bytes")
    assert not hasattr(powerio, "parse_display_text")
    assert parsed.kind == "powerworld"
    assert isinstance(parsed.data, powerio.PwdDisplay)
    assert parsed.data.canvas_width == 200
    assert parsed.data.canvas_height == 200
    assert parsed.data.stamp == 43068
    assert len(parsed.data.substations) == 111

    first = parsed.data.substations[0]
    assert isinstance(first, powerio.PwdSubstation)
    assert first.number == 50
    assert first.name == "CHAMPAIGN 3"
    assert first.x == pytest.approx(-47299.112519818635)
    assert first.y == pytest.approx(23498.080802557866)


def test_case_tables(case9):
    assert len(case9.buses) == 9
    assert len(case9.branches) == 9
    assert len(case9.generators) == 9 - 6  # 3 gens
    bus = case9.buses[0]
    assert bus["id"] == 1 and bus["kind"] == "REF"
    gen = case9.generators[0]
    assert gen["cost"]["model"] == 2
    assert gen["cost"]["coeffs"] == [0.11, 5.0, 150.0]


def test_branch_table_b_is_terminal_projection():
    case = parse_text(
        json.dumps(
            {
                "name": "terminal-projection",
                "baseMVA": 100.0,
                "per_unit": False,
                "bus": {
                    "1": {
                        "index": 1,
                        "bus_i": 1,
                        "bus_type": 3,
                        "vm": 1.0,
                        "va": 0.0,
                        "vmax": 1.1,
                        "vmin": 0.9,
                        "base_kv": 230.0,
                    },
                    "2": {
                        "index": 2,
                        "bus_i": 2,
                        "bus_type": 1,
                        "vm": 1.0,
                        "va": 0.0,
                        "vmax": 1.1,
                        "vmin": 0.9,
                        "base_kv": 230.0,
                    },
                },
                "branch": {
                    "1": {
                        "index": 1,
                        "f_bus": 1,
                        "t_bus": 2,
                        "br_r": 0.01,
                        "br_x": 0.1,
                        "g_fr": 0.01,
                        "b_fr": 0.02,
                        "g_to": 0.03,
                        "b_to": 0.05,
                        "tap": 1.0,
                        "shift": 0.0,
                        "br_status": 1,
                        "angmin": -6.283185307179586,
                        "angmax": 6.283185307179586,
                        "transformer": False,
                    }
                },
                "gen": {},
                "load": {},
                "shunt": {},
            }
        ),
        "powermodels-json",
    )

    br = case.branches[0]
    assert br["b"] == pytest.approx(0.07)
    assert br["g_fr"] == pytest.approx(0.01)
    assert br["b_fr"] == pytest.approx(0.02)
    assert br["g_to"] == pytest.approx(0.03)
    assert br["b_to"] == pytest.approx(0.05)


def test_loads_and_shunts_are_first_class():
    case = _parse(DATA / "case30.m", value_type=powerio.BalancedNetwork).value
    # MATPOWER folds demand onto the bus row; powerio splits it back out.
    assert case.n_loads > 0
    assert all({"bus", "p", "q", "in_service"} <= set(load) for load in case.loads)
    # buses carry no pd/qd (that's what loads are for)
    assert "pd" not in case.buses[0]


def test_parse_text_roundtrip(case9):
    text = (DATA / "case9.m").read_text()
    c = _parse(
        text.encode(),
        "matpower",
        name="case9.m",
        value_type=powerio.BalancedNetwork,
    ).value
    assert c.name == "case9"
    assert c.n_buses == case9.n_buses
    assert np.allclose(
        c.calc_bprime_matrix().toarray(), case9.calc_bprime_matrix().toarray()
    )


def test_parse_text_general():
    text = (DATA / "case9.m").read_text()
    c = _parse((text).encode(), "matpower", value_type=powerio.BalancedNetwork).value
    assert c.n_buses == 9


def test_parse_file_reaches_the_binary_reader():
    # PowerWorld binary has no text form, so binary input uses a temporary path
    # and the public file parser.
    pwb = (DATA / "powerworld" / "ACTIVSg200.pwb").read_bytes()
    c = _parse(pwb, "pwb", value_type=powerio.BalancedNetwork).value
    assert c.n_buses == 200
    assert c.n_branches == 246

    # Text formats agree with the path parse.
    m = (DATA / "case9.m").read_bytes()
    assert _parse(m, "matpower", value_type=powerio.BalancedNetwork).value.n_buses == 9

    # Bytes a text format cannot decode raise, rather than blaming the case.
    with pytest.raises(powerio.PowerIOError, match="UTF-8"):
        _parse(b"\xff\xfe\x00", "matpower", value_type=powerio.BalancedNetwork)


def test_parse_text_name_reaches_detection_and_source_naming():
    data = (DATA / "case9.m").read_bytes()

    # A name with a recognized extension lets format detection run without an
    # explicit from_, and the retained source records the given name.
    named = _parse(data, name="mycase.m")
    assert named.kind == "balanced_network"
    sources = json.loads(_emit_module(named))["sources"]
    assert [s["name"] for s in sources] == ["mycase.m"]

    with pytest.raises(TypeError, match="name"):
        powerio.parse_text(data.decode(), format="matpower")


def test_parse_diagnostics_are_module_records():
    # The genuine pandapower fixture carries a switch table the reader cannot
    # model, so the parse reports it; the MATPOWER reader is total and reports
    # nothing.
    module = _parse(
        DATA / "pandapower" / "example.json", value_type=powerio.BalancedNetwork
    )
    assert any("switch" in diagnostic.message for diagnostic in module.diagnostics)
    assert _parse(DATA / "case9.m").diagnostics == []


def test_pio_module_multiconductor_accessor_keeps_every_diagnostic():
    """A module's diagnostics, including one whose span references the
    module's own declared source, survive `as_multiconductor_network` in
    source order. Regression: the accessor used to build the network handle
    with no sources carried over, so a span validated against an empty source
    list and the first span-bearing diagnostic silently dropped itself and
    every diagnostic after it."""
    path = DATA / "dist" / "micro" / "xfmr_single_phase.dss"
    module = powerio.parse_file(path)
    doc = json.loads(_emit_module(module))
    source_id = doc["sources"][0]["id"]
    doc["diagnostics"] = [
        {"id": "d0", "severity": "warning", "code": "A.B.C", "message": "no span here"},
        {
            "id": "d1",
            "severity": "error",
            "code": "D.E.F",
            "message": "in range span",
            "spans": [{"source": source_id, "byte_start": 0, "byte_end": 10}],
        },
        {"id": "d2", "severity": "error", "code": "G.H.I", "message": "trailing error"},
    ]
    reloaded = _parse_module(json.dumps(doc))
    assert [diagnostic.message for diagnostic in reloaded.diagnostics] == [
        "no span here",
        "in range span",
        "trailing error",
    ]
    assert not hasattr(reloaded.value, "diagnostics")


def test_json_roundtrip_and_parsed_conversion():
    c = _parse(DATA / "case9.m", value_type=powerio.BalancedNetwork).value
    back = powerio.from_json(c.to_json())
    assert back.n_buses == c.n_buses
    assert back.base_mva == c.base_mva

    conv = powerio.PioModule.from_value(c).emit("powermodels-json")
    assert json.loads(conv.text)["name"] == "case9"
    assert conv.diagnostics == []
    assert not hasattr(conv, "warnings")
    assert powerio.PioModule.from_value(c).emit("matpower").text


def test_pio_module_from_value_keeps_records_and_wraps_generated_networks():
    source = DATA / "api_conformance.m"
    parsed = powerio.parse_file(source)
    wrapped = powerio.PioModule.from_value(parsed.value)
    assert wrapped.kind == "balanced_network"
    assert wrapped.diagnostics == parsed.diagnostics
    assert wrapped.emit("matpower").text.encode() == source.read_bytes()

    generated = powerio.from_json(parsed.value.to_json())
    generated_module = powerio.PioModule.from_value(generated)
    assert generated_module.value.n_buses == generated.n_buses
    assert "mpc.baseMVA" in generated_module.emit("matpower").text

    with pytest.raises(TypeError, match="BalancedNetwork or MulticonductorNetwork"):
        powerio.PioModule.from_value(object())


def test_pio_module_emit_uses_dynamic_writer(tmp_path):
    source = DATA / "api_conformance.m"
    module = powerio.parse_file(source)

    conversion = module.emit("matpower")
    assert conversion.text.encode() == source.read_bytes()
    assert conversion.diagnostics == []

    echoed = tmp_path / "echo.m"
    result = module.emit("matpower", echoed)
    assert result.text is None
    assert result.diagnostics == []
    assert echoed.read_bytes() == source.read_bytes()

    stored_text = module.emit("pio-json").text
    stored_doc = json.loads(stored_text)
    assert stored_doc["source_map"]

    stored = tmp_path / "case.pio.json"
    result = module.emit("pio-json", stored)
    assert result.text is None
    assert result.diagnostics == []
    assert json.loads(stored.read_text())["value"]["kind"] == "balanced_network"

    # A nonnetwork calculation has the same stored writer through PioModule.
    solved = powerio.parse_file(DATA / "opfdataset" / "example_0.json")
    solved_text = solved.emit("pio-json").text
    assert json.loads(solved_text)["value"]["kind"] == "ac_opf_solution"


def test_pio_module_emit_covers_memory_and_file_destinations(tmp_path):
    source = DATA / "api_conformance.m"
    module = powerio.parse_file(source)

    conversion = module.emit("matpower")
    assert conversion.text.encode() == source.read_bytes()
    assert conversion.diagnostics == []

    destination = tmp_path / "case.raw"
    result = module.emit("psse", destination)
    assert result.text is None
    assert all(
        isinstance(diagnostic, powerio.Diagnostic) for diagnostic in result.diagnostics
    )
    assert destination.read_text().startswith("0, 100,")


def test_inspect_advertises_only_operations_that_resolve_on_pio_module():
    modules = [
        powerio.parse_file(DATA / "api_conformance.m"),
        powerio.parse_file(DATA / "dist" / "micro" / "fourwire_linecode.dss"),
        _parse_module((DATA / "package" / "frozen-0.9-series.pio.json").read_text()),
        _parse_module(
            (DATA / "module-v1" / "mc-operating-point-series.pio.json").read_text()
        ),
    ]
    for module in modules:
        for operation in module.inspect()["operations"]:
            assert hasattr(module, operation), (module.kind, operation)

    balanced, multiconductor, series, multiconductor_series = modules
    assert "emit" in balanced.inspect()["operations"]
    assert {
        "emit",
        "to_balanced_report",
        "to_balanced",
    } <= set(multiconductor.inspect()["operations"])
    assert {
        "emit",
        "list_states",
        "inspect_state",
        "export_state",
    } <= set(series.inspect()["operations"])
    assert "emit" in multiconductor_series.inspect()["operations"]
    assert not {
        "list_states",
        "inspect_state",
        "export_state",
    } & set(multiconductor_series.inspect()["operations"])
    assert isinstance(multiconductor_series.value, powerio.UnknownValue)
    assert not hasattr(multiconductor_series.value, "__getitem__")
    assert "to_balanced_inspect" not in multiconductor.inspect()["operations"]
    assert "select_state" not in series.inspect()["operations"]


def test_typed_collection_protocols_and_inspect_state():
    series_module = _parse_module(
        (DATA / "package" / "frozen-0.9-series.pio.json").read_text()
    )
    series = series_module.value
    assert isinstance(series, powerio.TimeSeries)
    assert len(series) == 2
    assert series[-1].kind == "balanced_network"
    assert [item.kind for item in series] == ["balanced_network", "balanced_network"]
    assert series_module.list_states()["time_points"]
    with pytest.raises(IndexError):
        _ = series[2]
    assert (
        series_module.inspect_state(time_position=0)["item"]
        == "balanced_operating_point"
    )

    parsed = json.loads(_emit_module(powerio.parse_file(DATA / "api_conformance.m")))
    network = parsed["value"]["data"]
    scenario_doc = {
        "schema": "powerio.module",
        "version": 1,
        "producer": parsed["producer"],
        "value": {
            "kind": "balanced_network_scenario_set",
            "data": {
                "scenarios": [
                    {"id": "base", "probability": 0.6, "value": network},
                    {"id": "peak", "probability": 0.4, "value": network},
                ]
            },
        },
    }
    scenarios = _parse_module(json.dumps(scenario_doc)).value
    assert isinstance(scenarios, powerio.ScenarioSet)
    assert len(scenarios) == 2
    assert scenarios.keys() == ("base", "peak")
    assert list(scenarios) == ["base", "peak"]
    assert "peak" in scenarios and "winter" not in scenarios
    assert scenarios["peak"].kind == "balanced_network"
    with pytest.raises(KeyError):
        _ = scenarios["winter"]


def test_source_format_round_trips_through_emit(case9):
    pm = parse_text(_emit_value(case9, "powermodels-json").text, "powermodels-json")
    assert pm.source_format == "powermodels-json"
    eg = parse_text(_emit_value(case9, "egret-json").text, "egret-json")
    assert eg.source_format == "egret-json"
    for other in (case9, pm, eg):
        assert _emit_value(case9, other.source_format).text


def test_write_is_byte_exact():
    src = (DATA / "case9.m").read_text()
    case = _parse(DATA / "case9.m", value_type=powerio.BalancedNetwork).value
    assert _emit_value(case, "matpower").text == src


def test_to_normalized_is_per_unit_and_in_memory(case9):
    n = case9.to_normalized()
    # case9 is fully in service with one reference bus, so nothing is dropped.
    assert n.n_buses == case9.n_buses
    assert n.n_generators == case9.n_generators
    # A derived product with no retained source: it serializes from the model.
    assert n.source_format == "normalized"
    # Powers are per unit (divided by baseMVA).
    g, rg = n.generators[0], case9.generators[0]
    assert abs(g["pmax"] - rg["pmax"] / case9.base_mva) < 1e-9
    # The result is a full BalancedNetwork, so matrix calculations work on it.
    assert n.calc_bprime_matrix().shape == (n.n_buses, n.n_buses)


def test_to_normalized_filters_out_of_service():
    case = _parse(str(DATA / "t_case9_oos.m"), value_type=powerio.BalancedNetwork).value
    n = case.to_normalized()
    # The fixture marks one generator and one branch out of service; no isolated
    # buses, so every bus survives.
    assert n.n_generators == case.n_generators - 1
    assert n.n_branches == case.n_branches - 1
    assert n.n_buses == 9
    assert n.source_format == "normalized"


def test_to_normalized_preserves_source_bus_ids():
    src = """function mpc = sparseids
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t3\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t4\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t10\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t2\t3\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t3\t4\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t4\t10\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
"""
    n = _parse(
        (src).encode(), "matpower", value_type=powerio.BalancedNetwork
    ).value.to_normalized()
    assert [bus["id"] for bus in n.buses] == [1, 2, 3, 4, 10]
    assert n.loads[0]["bus"] == 10
    assert n.branches[-1]["from_id"] == 4
    assert n.branches[-1]["to_id"] == 10


def test_to_normalized_clamps_angle_bounds_with_keywords():
    case = _parse(
        DATA / "angle_bounds_clamp.m", value_type=powerio.BalancedNetwork
    ).value

    plain = case.to_normalized()
    assert plain.branches[0]["angmin"] == pytest.approx(-2.0 * math.pi)
    assert plain.branches[0]["angmax"] == pytest.approx(2.0 * math.pi)
    assert plain.branches[1]["angmin"] == pytest.approx(0.0)
    assert plain.branches[1]["angmax"] == pytest.approx(0.0)
    assert plain.branches[3]["angmin"] == pytest.approx(-120.0 * math.pi / 180.0)
    assert plain.branches[3]["angmax"] == pytest.approx(-100.0 * math.pi / 180.0)
    assert plain.branches[4]["angmin"] == pytest.approx(100.0 * math.pi / 180.0)
    assert plain.branches[4]["angmax"] == pytest.approx(120.0 * math.pi / 180.0)

    repaired = case.to_normalized(clamp_angle_bounds=True)
    repaired_diagnostics = powerio.PioModule.from_value(repaired).diagnostics
    assert repaired.branches[0]["angmin"] == pytest.approx(-1.0472)
    assert repaired.branches[0]["angmax"] == pytest.approx(1.0472)
    assert repaired.branches[1]["angmin"] == pytest.approx(-1.0472)
    assert repaired.branches[1]["angmax"] == pytest.approx(1.0472)
    assert repaired.branches[2]["angmin"] == pytest.approx(-math.pi / 6.0)
    assert repaired.branches[2]["angmax"] == pytest.approx(math.pi / 6.0)
    assert repaired.branches[3]["angmin"] == pytest.approx(-1.0472)
    assert repaired.branches[3]["angmax"] == pytest.approx(1.0472)
    assert repaired.branches[4]["angmin"] == pytest.approx(-1.0472)
    assert repaired.branches[4]["angmax"] == pytest.approx(1.0472)
    assert all(branch["angmin"] <= branch["angmax"] for branch in repaired.branches)
    assert any(
        "branch 0 angle difference bounds clamped" in warning
        for warning in (diagnostic.message for diagnostic in repaired_diagnostics)
    )
    assert any(
        "branch 1 angle difference bounds clamped" in warning
        for warning in (diagnostic.message for diagnostic in repaired_diagnostics)
    )
    assert any(
        "branch 3 angle difference bounds clamped" in warning
        for warning in (diagnostic.message for diagnostic in repaired_diagnostics)
    )
    assert any(
        "branch 4 angle difference bounds clamped" in warning
        for warning in (diagnostic.message for diagnostic in repaired_diagnostics)
    )

    with pytest.raises(powerio.PowerIODataError):
        case.to_normalized(
            clamp_angle_bounds=True, angle_bound_pad=math.pi / 2.0
        )


def test_parse_bad_path_raises():
    # I/O failures map to the standard OSError subclass, not PowerIOError.
    with pytest.raises(FileNotFoundError):
        _parse(DATA / "does_not_exist.m", value_type=powerio.BalancedNetwork)


def test_bad_parse_raises_powerio_error():
    with pytest.raises(powerio.PowerIOError):
        _parse(
            ("this is not a matpower case").encode(),
            "matpower",
            value_type=powerio.BalancedNetwork,
        )


def test_error_subclasses_are_powerio_errors():
    # The categorized errors subclass PowerIOError, so existing `except
    # PowerIOError` keeps catching them (backward compatible).
    assert issubclass(powerio.PowerIOParseError, powerio.PowerIOError)
    assert issubclass(powerio.PowerIODataError, powerio.PowerIOError)
    # And `ValueError`, so the handler callers wrote before the hierarchy
    # existed still catches every failure it used to.
    assert issubclass(powerio.PowerIOError, ValueError)


def test_malformed_case_raises_parse_error():
    # A malformed/unparseable case file is a parse-category error.
    with pytest.raises(powerio.PowerIOParseError):
        _parse(
            ("this is not a matpower case").encode(),
            "matpower",
            value_type=powerio.BalancedNetwork,
        )


def test_unmet_precondition_raises_data_error(tmp_path):
    # A well-formed case that can't satisfy an operation (here: DC-OPF with no
    # generators) is a data-category error, not a parse error.
    genless = TINY[: TINY.index("mpc.gen = [")]
    case = _parse(
        (genless).encode(), "matpower", value_type=powerio.BalancedNetwork
    ).value
    with pytest.raises(powerio.PowerIODataError):
        case.emit_dcopf_bundle(str(tmp_path))


def test_reference_bus_count_is_data_error():
    two_ref = TINY.replace("\t3\t2\t0", "\t3\t3\t0")  # bus 3: PV -> ref
    with pytest.raises(powerio.PowerIODataError):
        _parse(
            (two_ref).encode(), "matpower", value_type=powerio.BalancedNetwork
        ).value.reference_bus_index()


def test_dcopf_bundle_paths_are_clean_unicode(case9, tmp_path):
    # The returned dir/files must be exact strings that re-open the written
    # files, never lossily mangled (no U+FFFD).
    out = case9.emit_dcopf_bundle(str(tmp_path))
    assert "�" not in out["dir"]
    for f in out["files"]:
        assert "�" not in f
        assert Path(f).exists()


def test_delegated_surface_resolves(case9):
    # Pin the attributes/methods that reach through __getattr__ to the compiled
    # handle, so a Rust-side getter rename can't silently desync the API.
    for attr in [
        "name",
        "base_mva",
        "source_format",
        "n_buses",
        "n_branches",
        "n_generators",
        "n_loads",
        "n_shunts",
        "is_radial",
        "n_islands",
        "buses",
        "loads",
        "shunts",
        "branches",
        "generators",
        "reference_bus_index",
        "reference_bus_indices",
        "calc_connectivity_report",
        "to_geo_layer",
        "emit_dcopf_bundle",
    ]:
        assert hasattr(case9, attr), attr
    for removed in [
        "n_gens",
        "n_connected_components",
        "connectivity_report",
        "geo_layer",
        "to_matpower",
        "to_format",
        "write_file",
        "dc_data",
        "bprime",
        "bdoubleprime",
        "ybus",
        "ybus_parts",
        "adjacency",
        "ptdf",
        "lodf",
        "lacpf",
        "weighted_laplacian",
        "incidence",
        "write_gridfm",
        "write_dcopf_bundle",
        "write_pypsa_csv_folder",
    ]:
        assert not hasattr(case9, removed), removed
    assert "lowered" not in dir(case9)
    assert not hasattr(case9, "lowered")
    with pytest.raises(AttributeError):
        case9.does_not_exist  # noqa: B018 -- the attribute access is the assertion


def test_import_and_parse_pull_in_no_optional_deps():
    # The zero-dep promise: parse and emit need nothing but the
    # interpreter. Run in a fresh process so another test importing scipy can't
    # pollute it, and parse + write a real case so the whole IO path is covered.
    # `mcp` is checked too: the powerio.mcp submodule must never be imported from
    # powerio/__init__.py, so the optional MCP SDK stays out of `import powerio`.
    optional_modules = [
        "numpy",
        "scipy",
        "networkx",
        "polars",
        "pandas",
        "pyarrow",
        "mcp",
    ]
    code = (
        "import sys, powerio\n"
        f"m = powerio.parse_file(r'{DATA / 'case9.m'}', value_type=powerio.BalancedNetwork)\n"
        "assert m.emit('matpower').text\n"
        f"for name in {optional_modules!r}:\n"
        "    assert name not in sys.modules, f'powerio dragged in {name}'\n"
    )
    r = subprocess.run(
        [sys.executable, "-c", code], capture_output=True, text=True, check=False
    )
    assert r.returncode == 0, r.stderr


def test_missing_matrix_extra_raises_clear_importerror(case9, monkeypatch):
    def missing_module(name):
        if name in {"numpy", "scipy.sparse"}:
            raise ImportError(f"No module named {name!r}", name=name)
        return original_import(name)

    original_import = powerio.importlib.import_module
    monkeypatch.setattr(powerio.importlib, "import_module", missing_module)

    with pytest.raises(ImportError, match=r"powerio\[matrix\]"):
        case9.calc_bdoubleprime_matrix()
    with pytest.raises(ImportError, match=r"powerio\[matrix\]"):
        case9.calc_bprime_matrix()


def test_missing_graph_extra_raises_clear_importerror(case9, monkeypatch):
    def missing_module(name):
        if name == "networkx":
            raise ImportError(f"No module named {name!r}", name=name)
        return original_import(name)

    original_import = powerio.importlib.import_module
    monkeypatch.setattr(powerio.importlib, "import_module", missing_module)

    with pytest.raises(ImportError, match=r"powerio\[graph\]"):
        case9.to_networkx()


# --- matrix structure & values -----------------------------------------


@pytest.mark.parametrize("name", SMALL)
def test_bprime_is_singular_laplacian(name):
    c = load(name)
    b = c.calc_bprime_matrix()
    assert sp.issparse(b) and b.format == "csr"
    assert b.shape == (c.n_buses, c.n_buses)
    assert b.indices.dtype == np.int32  # COO indices emitted as i32
    assert is_symmetric(b)
    # Shuntless Laplacian: rows sum to zero, positive diagonal, M-matrix sign.
    row_sums = np.asarray(b.sum(axis=1)).ravel()
    assert np.allclose(row_sums, 0.0, atol=1e-8)
    diag = b.diagonal()
    assert np.all(diag > 0)
    off = b - sp.diags(diag)
    assert off.max() <= 1e-12


def test_bprime_xb_equals_weighted_laplacian(case9):
    # Exact cross-check across two boundary paths: Bp in the XB scheme is the
    # Reactance only weighted Laplacian (b = 1/x). Catches a shared bug in
    # the COO conversion that the symmetric self-check can't.
    assert np.allclose(
        case9.calc_bprime_matrix("xb").toarray(),
        case9.calc_weighted_laplacian("reactance_only").toarray(),
    )


def test_bdoubleprime_shunts_and_scheme():
    c = load("case30")  # has bus shunts
    bpp = c.calc_bdoubleprime_matrix()
    assert bpp.shape == (c.n_buses, c.n_buses)
    # Bpp keeps shunts, so it differs from Bp on this case.
    assert not np.allclose(bpp.toarray(), c.calc_bprime_matrix().toarray())
    # The scheme kwarg is wired: BX zeroes line resistance, XB does not.
    assert not np.allclose(
        c.calc_bdoubleprime_matrix("bx").toarray(),
        c.calc_bdoubleprime_matrix("xb").toarray(),
    )


@pytest.mark.parametrize("name", SMALL)
def test_admittance_matrix_is_complex(name):
    c = load(name)
    y = c.calc_admittance_matrix()
    assert y.dtype == np.complex128 and y.shape == (c.n_buses, c.n_buses)


def test_kwargs_change_output():
    # case14 carries nonzero taps, so taps/scheme are observable here.
    c = load("case14")
    assert not np.allclose(
        c.calc_bprime_matrix("xb").toarray(),
        c.calc_bprime_matrix("bx").toarray(),
    )
    assert not np.allclose(
        c.calc_admittance_matrix(include_taps=True).toarray(),
        c.calc_admittance_matrix(include_taps=False).toarray(),
    )


def test_adjacency_is_binary_symmetric(case9):
    a = case9.calc_adjacency_matrix()
    assert a.shape == (9, 9)
    assert is_symmetric(a)
    assert set(np.unique(a.data)).issubset({0.0, 1.0})
    assert a.diagonal().sum() == 0  # no self loops


def test_lacpf_block_shape(case9):
    block = case9.calc_lacpf_matrix()
    assert block.shape == (2 * case9.n_buses, 2 * case9.n_buses)


@pytest.mark.parametrize("name", SMALL)
def test_sensitivities(name):
    c = load(name)
    ptdf, lodf = c.calc_ptdf(), c.calc_lodf()
    m, n = ptdf.shape
    assert n == c.n_buses
    assert lodf.shape == (m, m)
    # LODF diagonal is -1 on the monitored = outaged branch.
    assert np.allclose(lodf.diagonal(), -1.0)
    # PTDF references injections to the slack, so the slack column is zero.
    assert np.allclose(ptdf.toarray()[:, c.reference_bus_index()], 0.0, atol=1e-9)


def test_weighted_laplacian_is_negative_bus_susceptance(case9):
    laplacian = case9.calc_weighted_laplacian().toarray()
    bus_susceptance = case9.calc_bus_susceptance_matrix().toarray()
    assert np.allclose(laplacian, -bus_susceptance)


def _float_bits(value):
    return f"0x{np.asarray([value], dtype=np.float64).view(np.uint64)[0]:016x}"


def _real_matrix_arrow_payload(matrix, table):
    csr = matrix.tocsr()
    row_index = []
    col_index = []
    value_bits = []
    for row in range(csr.shape[0]):
        start, end = csr.indptr[row], csr.indptr[row + 1]
        for col, value in zip(csr.indices[start:end], csr.data[start:end]):
            row_index.append(row)
            col_index.append(int(col))
            value_bits.append(_float_bits(value))
    return {
        "col_count": csr.shape[1],
        "col_index": col_index,
        "col_axis": "matrix_branch" if table == "incidence" else "matrix_bus",
        "format": "coo",
        "row_count": csr.shape[0],
        "row_index": row_index,
        "row_axis": "matrix_bus",
        "powerio_version": powerio.__version__,
        "table": table,
        "value_bits": value_bits,
    }


def _ybus_arrow_payload(case):
    ybus = case.calc_admittance_matrix()
    g, b = ybus.real, ybus.imag
    entries = {}
    for key, matrix in [("g_bits", g.tocsr()), ("b_bits", b.tocsr())]:
        for row in range(matrix.shape[0]):
            start, end = matrix.indptr[row], matrix.indptr[row + 1]
            for col, value in zip(matrix.indices[start:end], matrix.data[start:end]):
                entries.setdefault((row, int(col)), {})[key] = _float_bits(value)

    row_index = []
    col_index = []
    g_bits = []
    b_bits = []
    for row, col in sorted(entries):
        row_index.append(row)
        col_index.append(col)
        values = entries[(row, col)]
        g_bits.append(values.get("g_bits", "0x0000000000000000"))
        b_bits.append(values.get("b_bits", "0x0000000000000000"))

    return {
        "col_count": g.shape[1],
        "col_index": col_index,
        "col_axis": "matrix_bus",
        "format": "coo",
        "row_count": g.shape[0],
        "row_index": row_index,
        "row_axis": "matrix_bus",
        "powerio_version": powerio.__version__,
        "table": "ybus",
        "g_bits": g_bits,
        "b_bits": b_bits,
    }


def _matrix_axis_payload(case):
    active_branches = [
        index for index, branch in enumerate(case.branches) if branch["in_service"]
    ]
    return {
        "matrix_bus": {
            "bus_id": [bus["id"] for bus in case.buses],
            "component": [0] * case.n_buses,
            "format": "axis_map",
            "index": list(range(case.n_buses)),
            "is_reference": [1 if bus["kind"] == "REF" else 0 for bus in case.buses],
            "row_axis": "matrix_bus",
            "powerio_version": powerio.__version__,
            "source_row": list(range(case.n_buses)),
            "table": "matrix_bus",
        },
        "matrix_branch": {
            "format": "axis_map",
            "from_bus_id": [
                case.branches[index]["from_id"] for index in active_branches
            ],
            "index": list(range(len(active_branches))),
            "row_axis": "matrix_branch",
            "powerio_version": powerio.__version__,
            "source_row": active_branches,
            "table": "matrix_branch",
            "to_bus_id": [case.branches[index]["to_id"] for index in active_branches],
        },
    }


@pytest.mark.parametrize("name", ["case9", "case30"])
def test_matrix_methods_match_rust_arrow_golden(name):
    case = load(name)
    actual = {
        "axes": _matrix_axis_payload(case),
        "case": f"{name}.m",
        "tables": {
            "bdoubleprime": _real_matrix_arrow_payload(
                case.calc_bdoubleprime_matrix(), "bdoubleprime"
            ),
            "bprime": _real_matrix_arrow_payload(case.calc_bprime_matrix(), "bprime"),
            "incidence": _real_matrix_arrow_payload(
                # ABI 6 Arrow keeps its released bus by branch table layout;
                # Python exposes the PowerModels branch by bus orientation.
                case.calc_incidence_matrix().T,
                "incidence",
            ),
            "ybus": _ybus_arrow_payload(case),
        },
    }
    expected = json.loads((DATA / "capi_matrix" / f"{name}_arrow_coo.json").read_text())
    assert actual == expected


# --- string-kwarg parsing (aliases + errors) ---------------------------


def test_ppc_round_trip(case9):
    ppc = case9.to_ppc()
    assert ppc["version"] == "2"
    assert ppc["baseMVA"] == 100.0
    assert ppc["bus"].shape == (9, 13)
    assert ppc["gen"].shape == (3, 21)
    assert ppc["branch"].shape == (9, 13)
    assert ppc["gencost"].shape == (3, 7)
    # case9 loads: 90 MW at bus 5, 100 at 7, 125 at 9, summed into PD.
    pd_by_bus = {int(row[0]): row[2] for row in ppc["bus"]}
    assert pd_by_bus[5] == 90.0 and pd_by_bus[7] == 100.0 and pd_by_bus[9] == 125.0

    back = powerio.from_ppc(ppc)
    assert back.n_buses == case9.n_buses
    assert back.n_branches == case9.n_branches
    assert back.n_generators == case9.n_generators
    # The ppc projection is a fixed point: to_ppc(from_ppc(ppc)) == ppc.
    again = back.to_ppc()
    for key in ("bus", "gen", "branch", "gencost"):
        np.testing.assert_allclose(again[key], ppc[key], atol=0.0)
    assert again["baseMVA"] == ppc["baseMVA"]


def test_from_ppc_rejects_missing_tables(case9):
    ppc = case9.to_ppc()
    del ppc["branch"]
    with pytest.raises(ValueError):
        powerio.from_ppc(ppc)


def test_from_ppc_drops_result_columns(case9):
    # PYPOWER's runpf appends result columns (LAM_P, MU_*); from_ppc reads
    # the case back as inputs, so the extra columns must not break parsing.
    ppc = case9.to_ppc()
    ppc["bus"] = np.hstack([ppc["bus"], np.ones((9, 4))])
    ppc["branch"] = np.hstack([ppc["branch"], np.ones((9, 5))])
    back = powerio.from_ppc(ppc)
    assert back.n_buses == 9 and back.n_branches == 9


def test_ppc_keeps_demand_on_a_de_energized_bus():
    # The MATPOWER reader marks a load on an isolated bus out of service while
    # keeping its PD/QD, and the writer still sums it onto the bus row. to_ppc
    # has to agree: filtering on in_service silently dropped 50 MW that the
    # same network writes out as MATPOWER text.
    src = (
        "function mpc = iso\n"
        "mpc.version = '2';\n"
        "mpc.baseMVA = 100;\n"
        "mpc.bus = [\n"
        "\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n"
        "\t2\t4\t50\t10\t3\t7\t1\t1\t0\t230\t1\t1.1\t0.9;\n"
        "];\n"
        "mpc.gen = [\n"
        "\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0;\n"
        "];\n"
        "mpc.branch = [\n"
        "\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;\n"
        "];\n"
    )
    net = _parse((src).encode(), "matpower", value_type=powerio.BalancedNetwork).value
    assert not net.loads[0]["in_service"] and not net.shunts[0]["in_service"]

    row = net.to_ppc()["bus"][1]
    assert list(row[2:6]) == [50.0, 10.0, 3.0, 7.0]
    back = powerio.from_ppc(net.to_ppc())
    assert back.loads[0]["p"] == 50.0 and back.shunts[0]["b"] == 7.0


def test_ppc_gen_width_follows_the_capability_columns(case9):
    # case9 states the OPF capability columns (as zeros), so the table stays 21
    # wide and the round trip keeps them. A 10 column source states none, and
    # widening it to 21 would invent eleven zero limits — a ramp aware solver
    # reads ramp_10 = 0 as a generator that cannot move.
    assert case9.to_ppc()["gen"].shape == (3, 21)
    assert all(c is not None for c in case9.generators[0]["caps"])

    src = _emit_value(case9, "matpower").text.split("mpc.gen = [")
    rows = src[1].split("];")[0].strip().split("\n")
    narrow = "\n".join(
        "\t" + "\t".join(r.strip().rstrip(";").split()[:10]) + ";" for r in rows
    )
    net = parse_text(
        f"{src[0]}mpc.gen = [\n{narrow}\n];{src[1].split('];', 1)[1]}", "matpower"
    )
    assert all(c is None for c in net.generators[0]["caps"])

    ppc = net.to_ppc()
    assert ppc["gen"].shape == (3, 10)
    assert all(c is None for c in powerio.from_ppc(ppc).generators[0]["caps"])


def test_ppc_omits_gencost_unless_every_generator_is_costed(case9):
    # MATPOWER takes cost rows for all generators or none, so one costless
    # generator drops the whole table. Mixed coverage is ordinary input (the
    # pandapower reader sets cost per generator), and the network keeps the
    # costs it has — the omission is the ppc's alone.
    assert "gencost" in case9.to_ppc()

    doc = json.loads(case9.to_json())
    doc["generators"][1]["cost"] = None
    mixed = powerio.from_json(json.dumps(doc))
    assert [g["cost"] is None for g in mixed.generators] == [False, True, False]

    assert "gencost" not in mixed.to_ppc()


def test_from_ppc_refuses_a_truncated_table(case9):
    # Zero padding a short bus row would invent a bus at 0 p.u. and 0 kV; the
    # MATPOWER reader refuses such a row in a .m file, and so does this.
    ppc = case9.to_ppc()
    ppc["bus"] = ppc["bus"][:, :7]
    with pytest.raises(ValueError, match=r"'bus' row 0 has 7 columns"):
        powerio.from_ppc(ppc)


def test_from_ppc_names_the_table_and_row_for_malformed_input(case9):
    one_d = case9.to_ppc()
    one_d["bus"] = one_d["bus"][0]
    with pytest.raises(ValueError, match=r"'bus' row 0 is not a sequence"):
        powerio.from_ppc(one_d)

    lettered = case9.to_ppc()
    table = lettered["bus"].astype(object)
    table[2, 0] = "abc"
    lettered["bus"] = table
    with pytest.raises(ValueError, match=r"'bus' row 2 has a non-numeric value"):
        powerio.from_ppc(lettered)


FORMULAS = [
    "series_susceptance",
    "tap_adjusted_reactance",
    "reactance_only",
]


@pytest.mark.parametrize("formula", FORMULAS)
def test_branch_susceptance_formulas(case9, formula):
    assert sp.issparse(case9.calc_ptdf(formula))
    assert sp.issparse(case9.calc_lodf(formula))
    assert sp.issparse(case9.calc_weighted_laplacian(formula))
    assert sp.issparse(case9.calc_incidence_matrix(formula))


@pytest.mark.parametrize(
    "old",
    [
        "series",
        "series-impedance",
        "matpower",
        "mp",
        "reactance-only",
        "paper-pure",
        "paper",
        "SERIES_SUSCEPTANCE",
    ],
)
def test_branch_susceptance_formula_aliases_are_refused(case9, old):
    with pytest.raises(ValueError, match="branch susceptance formula"):
        case9.calc_ptdf(old)


def test_scheme_aliases(case9):
    for scheme in ["bx", "XB"]:
        assert sp.issparse(case9.calc_bprime_matrix(scheme))


# Every matrix calculation that threads a skip_zero_impedance keyword through to
# BuildOptions. Only the acceptance half is pinned here: on this vintage
# BuildOptions::default() still silently skips a zero impedance branch, so a
# case with none (case9) is unaffected by either value, and asserting the
# refusal-by-default behavior has to wait for the lower branch's default flip
# to cascade in.
SKIP_ZERO_IMPEDANCE_METHODS = [
    "calc_bprime_matrix",
    "calc_bdoubleprime_matrix",
    "calc_lacpf_matrix",
    "calc_admittance_matrix",
]


@pytest.mark.parametrize("method", SKIP_ZERO_IMPEDANCE_METHODS)
def test_skip_zero_impedance_kwarg_is_accepted(case9, method):
    def matrices(result):
        if method == "calc_admittance_matrix":
            return [result]
        return [result]

    call = getattr(case9, method)
    default = matrices(call())
    explicit_false = matrices(call(skip_zero_impedance=False))
    explicit_true = matrices(call(skip_zero_impedance=True))
    for a, b in zip(default, explicit_false):
        assert np.allclose(a.toarray(), b.toarray())
    for a, b in zip(default, explicit_true):
        assert np.allclose(a.toarray(), b.toarray())


def test_sensitivity_solver_kwarg(case9):
    # On a small case the auto policy picks the dense path, so the explicit
    # spellings must agree with the default. The sparse path agrees only
    # to its 1e-10 relative residual.
    base = case9.calc_ptdf().toarray()
    assert np.allclose(case9.calc_ptdf(solver="dense").toarray(), base, atol=1e-9)
    assert np.allclose(case9.calc_ptdf(solver="sparse").toarray(), base, atol=1e-6)
    lodf = case9.calc_lodf().toarray()
    assert np.allclose(case9.calc_lodf(solver="dense").toarray(), lodf, atol=1e-9)
    assert np.allclose(case9.calc_lodf(solver="sparse").toarray(), lodf, atol=1e-6)
    # The CG path is retired; its spelling is refused with the accepted set.
    with pytest.raises(ValueError, match="expected 'auto', 'dense', or 'sparse'"):
        case9.calc_lodf(solver="CG")


def test_bad_enum_strings_raise(case9, tmp_path):
    with pytest.raises(ValueError):
        case9.calc_bprime_matrix(scheme="nonsense")
    with pytest.raises(ValueError):
        case9.calc_ptdf(formula="nope")
    with pytest.raises(ValueError):
        case9.calc_ptdf(solver="bogus")
    with pytest.raises(ValueError):
        case9.emit_dcopf_bundle(str(tmp_path), units="bogus")


# --- graph view ---------------------------------------------------------


def test_to_networkx_attrs_and_status_filter():
    c = _parse((TINY).encode(), "matpower", value_type=powerio.BalancedNetwork).value
    g = c.to_networkx()
    assert g.number_of_nodes() == 3 and g.number_of_edges() == 2
    # Edge attributes mirror the branch table.
    assert g.edges[1, 2]["branch"] == 0
    assert g.edges[1, 2]["x"] == c.branches[0]["x"]
    # An out-of-service branch is dropped from the graph.
    oos = TINY.replace(
        "2\t3\t0.01\t0.1\t0\t250\t250\t250\t0\t0\t1\t-360\t360",
        "2\t3\t0.01\t0.1\t0\t250\t250\t250\t0\t0\t0\t-360\t360",
    )
    assert (
        _parse((oos).encode(), "matpower", value_type=powerio.BalancedNetwork)
        .value.to_networkx()
        .number_of_edges()
        == 1
    )


# --- connectivity & reference bus --------------------------------------


def test_connectivity_report(case9):
    rep = case9.calc_connectivity_report()
    assert rep["n_buses"] == 9
    assert rep["n_components"] == 1
    assert rep["isolated_buses"] == []


def test_reference_bus_index(case9):
    assert case9.reference_bus_index() == 0
    assert case9.reference_bus_indices() == [0]


def test_reference_bus_error_on_two_refs():
    two_ref = TINY.replace("\t3\t2\t0", "\t3\t3\t0")  # bus 3: PV -> ref
    case = _parse(
        (two_ref).encode(), "matpower", value_type=powerio.BalancedNetwork
    ).value
    # The single-ref query raises; the reference-set query returns both, so a
    # multi-slack case stays legible from Python.
    with pytest.raises(powerio.PowerIOError):
        case.reference_bus_index()
    assert len(case.reference_bus_indices()) == 2


# --- DC-OPF bundle ------------------------------------------------------


def test_emit_dcopf_bundle_content(case9, tmp_path):
    out = case9.emit_dcopf_bundle(str(tmp_path))
    files = out["files"]
    assert Path(out["dir"]).is_dir()
    names = {Path(f).name for f in files}
    assert {"A.mtx", "L.mtx", "q.mtx", "pd.mtx", "dcopf_meta.json"} <= names
    by_name = {Path(f).name: f for f in files}
    # Files are real and loadable, not just present.
    a = scipy.io.mmread(by_name["A.mtx"])
    assert a.shape[0] == case9.n_buses
    json.loads(Path(by_name["dcopf_meta.json"]).read_text())


def test_dcopf_units_change_cost(case9, tmp_path):
    pu = scipy.io.mmread(_bundle_file(case9, tmp_path / "pu", "q.mtx", units="perunit"))
    native = scipy.io.mmread(
        _bundle_file(case9, tmp_path / "na", "q.mtx", units="native")
    )
    assert not np.allclose(np.asarray(pu).ravel(), np.asarray(native).ravel())


def _bundle_file(case, out_dir, name, **kw):
    out_dir.mkdir()
    out = case.emit_dcopf_bundle(str(out_dir), **kw)
    return next(f for f in out["files"] if Path(f).name == name)


def test_dcopf_requires_generators(tmp_path):
    genless = TINY[: TINY.index("mpc.gen = [")]
    case = _parse(
        (genless).encode(), "matpower", value_type=powerio.BalancedNetwork
    ).value
    assert case.n_generators == 0
    with pytest.raises(powerio.PowerIOError):
        case.emit_dcopf_bundle(str(tmp_path))


# --- convert -----------------------------------------------------------


def test_emit_matpower_echo_is_byte_exact():
    src = (DATA / "case14.m").read_text()
    conv = _emit_file(DATA / "case14.m", "matpower")
    assert conv.text == src
    assert conv.diagnostics == []


def test_emit_matpower_to_each_format():
    for fmt in [
        "powermodels-json",
        "egret-json",
        "psse",
        "powerworld",
        "pandapower-json",
    ]:
        r = _emit_file(str(DATA / "case30.m"), fmt)
        assert isinstance(r.text, str) and len(r.text) > 0
        assert isinstance(r.diagnostics, list)
    # PowerModels JSON output parses as JSON and keeps the bus count.
    pm = json.loads(_emit_file(str(DATA / "case30.m"), "powermodels-json").text)
    assert len(pm["bus"]) == 30
    pp = json.loads(_emit_file(str(DATA / "case30.m"), "pandapower-json").text)
    assert pp["_class"] == "pandapowerNet"


def test_emit_round_trip_through_psse(tmp_path):
    raw = _emit_file(str(DATA / "case30.m"), "psse").text
    p = tmp_path / "case30.raw"
    p.write_text(raw)
    back = _emit_file(str(p), "matpower")  # PSS/E inferred from .raw extension
    case = _parse(
        (back.text).encode(), "matpower", value_type=powerio.BalancedNetwork
    ).value
    assert case.n_buses == 30


def test_emit_destination_preserves_the_crlf_echo(tmp_path):
    # A same-format echo of a CRLF source must reach disk byte exact.
    # Writing emitted text through open(path, "w") corrupts it on Windows
    # (text mode turns each \r\n into \r\r\n); module.emit(path) bypasses that.
    src = DATA / "psse" / "case14.raw"
    assert b"\r\n" in src.read_bytes()
    case = _parse(src, value_type=powerio.BalancedNetwork).value
    out = tmp_path / "echo.raw"
    result = _emit_value(case, "psse", out)
    assert result.text is None
    assert result.diagnostics == []
    assert out.read_bytes() == src.read_bytes()


def test_emit_file_destination_writes_the_text_byte_exact(tmp_path):
    src = DATA / "psse" / "case14.raw"
    out = tmp_path / "echo.raw"
    result = _emit_file(src, "psse", destination=out)
    assert result.text is None
    assert result.diagnostics == []
    assert out.read_bytes() == src.read_bytes()


def test_emit_psse_start_of_markers_to_powermodels(tmp_path):
    p = tmp_path / "start_markers.raw"
    p.write_text(PSSE_START_OF_MARKERS)

    pm = json.loads(_emit_file(p, "powermodels-json", from_="psse").text)

    assert len(pm["bus"]) == 2
    assert len(pm["load"]) == 1
    assert len(pm["gen"]) == 1
    assert len(pm["branch"]) == 1


def test_emit_unknown_format_raises():
    with pytest.raises(ValueError):
        _emit_file(str(DATA / "case30.m"), "nonsense")


def test_emit_text_matches_emit_file():
    text = (DATA / "case30.m").read_text()
    for fmt in [
        "powermodels-json",
        "egret-json",
        "psse",
        "powerworld",
        "pandapower-json",
    ]:
        from_text = _emit_text(text, fmt, name="case30.m")
        from_path = _emit_file(str(DATA / "case30.m"), fmt)
        assert from_text.text == from_path.text
        assert from_text.diagnostics == from_path.diagnostics


def test_emit_text_matpower_echo_is_byte_exact():
    src = (DATA / "case14.m").read_text()
    conv = _emit_text(src, "matpower")
    assert conv.text == src
    assert conv.diagnostics == []


def test_emit_preserves_matpower_source_echo():
    src = (DATA / "case14.m").read_text()
    net = _parse((src).encode(), "matpower", value_type=powerio.BalancedNetwork).value
    assert _emit_value(net, "matpower").text == src


def test_emit_text_named_input_format():
    raw = _emit_file(str(DATA / "case30.m"), "psse").text
    back = _emit_text(raw, "matpower", from_="psse")
    assert (
        _parse(
            (back.text).encode(), "matpower", value_type=powerio.BalancedNetwork
        ).value.n_buses
        == 30
    )


def test_single_file_writes_never_replace_an_existing_entry(tmp_path):
    case = _parse(DATA / "case9.m", value_type=powerio.BalancedNetwork).value

    # emit: an existing entry keeps its bytes; a fresh path commits.
    target = tmp_path / "case9.raw"
    target.write_text("precious")
    with pytest.raises(powerio.PowerIOError) as refusal:
        _emit_value(case, "psse", target)
    assert getattr(refusal.value, "code", "").startswith("REQUEST.OUTPUT")
    assert target.read_text() == "precious"
    fresh = tmp_path / "fresh.raw"
    _emit_value(case, "psse", fresh)
    assert fresh.read_text().strip()

    # A symbolic link at the target is neither followed nor replaced.
    designated = tmp_path / "designated.raw"
    designated.write_text("designated")
    linked = tmp_path / "linked.raw"
    linked.symlink_to(designated)
    with pytest.raises(powerio.PowerIOError):
        _emit_value(case, "psse", linked)
    assert linked.is_symlink()
    assert designated.read_text() == "designated"

    # A file emission destination follows the same rule.
    out = tmp_path / "emitted.m"
    out.write_text("precious")
    with pytest.raises(powerio.PowerIOError):
        _emit_file(str(DATA / "case9.m"), "matpower", destination=str(out))
    assert out.read_text() == "precious"


def test_pypsa_csv_folder_never_replaces_an_existing_target(tmp_path):
    case = _parse(DATA / "case9.m", value_type=powerio.BalancedNetwork).value
    out = tmp_path / "pypsa"
    out.mkdir()
    (out / "buses.csv").write_text("precious")
    with pytest.raises(powerio.PowerIOError) as refusal:
        _emit_value(case, "pypsa-csv", out)
    assert getattr(refusal.value, "code", "").startswith("REQUEST.OUTPUT")
    assert (out / "buses.csv").read_text() == "precious"


def test_pypsa_csv_folder_wrapper(tmp_path):
    case = _parse(DATA / "case9.m", value_type=powerio.BalancedNetwork).value
    out = tmp_path / "pypsa"
    result = _emit_value(case, "pypsa-csv", out)
    assert (out / "network.csv").is_file()
    assert (out / "buses.csv").is_file()
    assert result.text is None
    assert result.diagnostics
    assert all(
        isinstance(diagnostic, powerio.Diagnostic) for diagnostic in result.diagnostics
    )

    back = _parse(out, "pypsa-csv", value_type=powerio.BalancedNetwork).value
    assert back.n_buses == case.n_buses
    assert back.n_branches == case.n_branches
    assert back.n_generators == case.n_generators


def test_emit_text_errors():
    with pytest.raises(powerio.PowerIOError):
        _emit_text("not a case", "psse")
    with pytest.raises(ValueError):
        _emit_text((DATA / "case14.m").read_text(), "nonsense")


def test_missing_json_file_raises_oserror():
    # The non-MATPOWER read path must raise OSError too: a missing file is a
    # missing file, not a ValueError, regardless of the inferred format.
    with pytest.raises(OSError):
        _emit_file(DATA / "definitely_missing.json", "matpower")


# --- large case integration --------------------------------------------


def test_large_case_pegase():
    path = DATA / "case2869pegase.m"
    if not path.is_file():
        pytest.skip("case2869pegase.m not vendored")
    c = _parse(str(path), value_type=powerio.BalancedNetwork).value
    assert c.n_buses == 2869
    b = c.calc_bprime_matrix()
    assert b.shape == (2869, 2869)
    # MATPOWER Bp keeps phase shifts. This case has phase shifters, so the
    # off diagonal entries are asymmetric.
    assert not is_symmetric(b)
    assert np.isfinite(b.data).all()


# --- gridfm Parquet surface --------------------------------------------

HAS_GRIDFM = bool(getattr(powerio._powerio, "_has_gridfm", False))
gridfm_only = pytest.mark.skipif(
    not HAS_GRIDFM, reason="extension built without the gridfm feature"
)


def test_gridfm_absent_raises_clean_importerror(case9, tmp_path):
    # Custom native builds can compile the emit path out, so the wrapper must
    # still raise ImportError rather than AttributeError.
    if HAS_GRIDFM:
        pytest.skip("extension built with gridfm; the absent-path is not exercised")
    with pytest.raises(ImportError, match="gridfm"):
        case9.emit_gridfm(str(tmp_path))


@gridfm_only
def test_gridfm_emit_single(case9, tmp_path):
    pl = pytest.importorskip("polars")
    out = case9.emit_gridfm(str(tmp_path))
    raw = Path(out["dir"])
    assert raw.is_dir()
    names = {Path(f).name for f in out["files"]}
    assert {
        "bus_data.parquet",
        "gen_data.parquet",
        "branch_data.parquet",
        "y_bus_data.parquet",
        "gridfm_meta.json",
    } <= names

    bus = pl.read_parquet(raw / "bus_data.parquet")
    assert len(bus) == case9.n_buses
    assert (bus["scenario"] == 0).all()
    assert bus["bus"].to_list() == list(range(case9.n_buses))


@gridfm_only
def test_gridfm_include_y_bus_false_omits_table(case9, tmp_path):
    # The include_y_bus kwarg crosses the native boundary: disabling it must drop
    # y_bus_data.parquet (the other three tables stay).
    out = case9.emit_gridfm(str(tmp_path), include_y_bus=False)
    names = {Path(f).name for f in out["files"]}
    assert "y_bus_data.parquet" not in names
    assert {"bus_data.parquet", "gen_data.parquet", "branch_data.parquet"} <= names


@gridfm_only
def test_gridfm_batch_stacks_and_keys_by_scenario(tmp_path):
    pl = pytest.importorskip("polars")
    # Same topology twice → two scenarios stacked in one dataset. (The Python
    # BalancedNetwork is read-only, so the two snapshots share values; the test pins the
    # row-stack and scenario keying, which the Rust tests pair with perturbation.)
    case = load("case9")
    out = powerio.emit_gridfm_batch([case, case], str(tmp_path))
    raw = Path(out["dir"])

    bus = pl.read_parquet(raw / "bus_data.parquet")
    assert len(bus) == 2 * case.n_buses
    assert bus["scenario"].to_list() == [0] * case.n_buses + [1] * case.n_buses
    # Same case twice → the two scenario blocks carry identical per-bus values
    # and the dense bus index resets to 0..n_buses within each scenario.
    n = case.n_buses
    for col in ["Pd", "Qd", "Pg", "Qg", "Vm", "Va"]:
        assert bus[col][:n].to_list() == bus[col][n:].to_list()
    assert bus["bus"][:n].to_list() == list(range(n))
    assert bus["bus"][n:].to_list() == list(range(n))

    meta = json.loads((raw / "gridfm_meta.json").read_text())
    assert meta["n_scenarios"] == 2
    assert meta["scenario"] == 0


@gridfm_only
def test_parse_file_reads_gridfm_scenarios_with_module_diagnostics(tmp_path):
    case = load("case9")
    out = powerio.emit_gridfm_batch([case, case], str(tmp_path))

    module = powerio.parse_file(out["dir"])
    assert module.kind == "balanced_network_scenario_set"
    assert module.diagnostics
    assert all(
        isinstance(diagnostic, powerio.Diagnostic) for diagnostic in module.diagnostics
    )
    assert module.list_states() == {
        "keyed_by": "scenario",
        "scenarios": [
            {"id": "0", "probability": None},
            {"id": "1", "probability": None},
        ],
    }
    for scenario in ("0", "1"):
        selected = module.export_state(scenario=scenario)
        assert selected.kind == "balanced_network"
        assert selected.value.n_buses == case.n_buses
        assert selected.value.source_format == "gridfm"


def test_gridfm_public_surface_uses_parse_and_emit():
    assert "emit_gridfm_batch" in powerio.__all__
    assert hasattr(powerio, "emit_gridfm_batch")
    for removed in (
        "write_gridfm_batch",
        "read_gridfm",
        "read_gridfm_scenarios",
        "GridfmRead",
    ):
        assert removed not in powerio.__all__
        assert not hasattr(powerio, removed)


def test_source_format_stubs_cover_every_variant():
    # The .pyi Literal must list every string the runtime can produce; a new
    # SourceFormat variant lands here and in both stubs together.
    variants = [
        "matpower",
        "powermodels-json",
        "opfdata-json",
        "egret-json",
        "psse",
        "powerworld",
        "powerworld-pwb",
        "gridfm",
        "in-memory",
        "normalized",
    ]
    root = Path(__file__).resolve().parents[1] / "powerio"
    for stub in ("__init__.pyi", "_powerio.pyi"):
        text = (root / stub).read_text()
        for v in variants:
            assert f'"{v}"' in text, f"{stub} missing source_format {v!r}"


def test_direct_dc_operations_follow_powermodels_orientation_and_sign():
    """Named DC calculations match the PowerModels orientation and sign."""
    net = powerio.parse_file(
        DATA / "api_conformance.m", value_type=powerio.BalancedNetwork
    ).value
    A = net.calc_incidence_matrix()
    Bf = net.calc_branch_susceptance_matrix()
    B = net.calc_bus_susceptance_matrix()
    assert A.shape == Bf.shape == (2, 3)
    assert B.shape == (3, 3)
    assert A.toarray().tolist() == [[1.0, -1.0, 0.0], [1.0, 0.0, -1.0]]
    susceptance = np.asarray(
        [
            -branch["x"] / (branch["r"] ** 2 + branch["x"] ** 2)
            for branch in net.branches
        ]
    )
    shift = np.radians([branch["shift"] for branch in net.branches])
    np.testing.assert_allclose(Bf.toarray(), np.diag(susceptance) @ A.toarray())
    np.testing.assert_allclose(B.toarray(), (A.T @ Bf).toarray())
    va = np.zeros(3)
    branch_flow = net.calc_branch_flow_dc(va)
    shift_injection = net.calc_phase_shift_injection()
    bus_injection = net.calc_bus_injection_dc(va)
    np.testing.assert_allclose(
        branch_flow,
        susceptance * shift,
    )
    np.testing.assert_allclose(bus_injection, A.T @ branch_flow)
    np.testing.assert_allclose(bus_injection, shift_injection)
    np.testing.assert_allclose(shift_injection, A.T @ (susceptance * shift))
    nonzero_va = np.asarray([0.01, -0.02, 0.03])
    nonzero_branch_flow = net.calc_branch_flow_dc(nonzero_va)
    np.testing.assert_allclose(
        nonzero_branch_flow,
        -(Bf @ nonzero_va) + susceptance * shift,
    )
    np.testing.assert_allclose(
        net.calc_bus_injection_dc(nonzero_va),
        -(B @ nonzero_va) + shift_injection,
    )
    np.testing.assert_allclose(
        net.calc_bus_injection_dc(nonzero_va),
        A.T @ nonzero_branch_flow,
    )
    np.testing.assert_allclose(
        net.calc_branch_susceptance_matrix("tap_adjusted_reactance").toarray(),
        net.calc_branch_susceptance_matrix("reactance_only").toarray(),
    )
    with pytest.raises(ValueError, match="length 3"):
        net.calc_branch_flow_dc([0.0, 0.0])
    with pytest.raises(ValueError, match="length 3"):
        net.calc_bus_injection_dc([0.0, 0.0])
    with pytest.raises(ValueError, match="susceptance formula"):
        net.calc_incidence_matrix(formula="mystery")
    assert not hasattr(net, "dc_data")


def test_canonical_matrix_methods_are_the_only_public_spellings(case9):
    assert case9.calc_admittance_matrix().shape == (9, 9)
    assert case9.calc_bprime_matrix().shape == (9, 9)
    assert case9.calc_bdoubleprime_matrix().shape == (9, 9)
    assert case9.calc_lacpf_matrix().shape == (18, 18)
    assert case9.calc_adjacency_matrix().shape == (9, 9)
    assert case9.calc_incidence_matrix().shape == (9, 9)
    assert case9.calc_connectivity_report()["n_components"] == 1


def test_direct_branch_axis_dc_operations_use_active_branch_order():
    net = powerio.parse_file(
        DATA / "t_case9_oos.m", value_type=powerio.BalancedNetwork
    ).value
    assert any(not branch["in_service"] for branch in net.branches)

    n_active = sum(branch["in_service"] for branch in net.branches)
    assert net.calc_incidence_matrix().shape == (n_active, net.n_buses)
    assert net.calc_branch_susceptance_matrix().shape == (n_active, net.n_buses)
    assert net.calc_branch_flow_dc(np.zeros(net.n_buses)).shape == (n_active,)

    # Bus axis calculations remain complete; inactive branches contribute no
    # operator row.
    assert net.calc_bus_susceptance_matrix().shape == (net.n_buses, net.n_buses)
    assert net.calc_phase_shift_injection().shape == (net.n_buses,)
    assert net.calc_bus_injection_dc(np.zeros(net.n_buses)).shape == (net.n_buses,)
