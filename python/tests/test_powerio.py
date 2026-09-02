"""Tests for the powerio Python bindings.

Run with `pytest python/tests` after `maturin develop`. The matrix and graph
tests need the optional extras: `pip install '.[all]'`.
"""

import ast
import importlib
import io
import json
import math
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest
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
    """Exercise the canonical source parser for test fixtures."""
    assert include_root is None
    module = powerio.parse(source, format=format, name=name)
    if value_type not in (None, powerio.PioModule) and not isinstance(
        module.value, value_type
    ):
        raise ValueError(
            f"parsed value is {type(module.value).__name__}; expected {value_type.__name__}"
        )
    return module


def parse_text(text, format):
    return powerio.parse(
        io.StringIO(text),
        name="fixture",
        format=format,
    ).value


def _emit_value(value, format, destination=None):
    return powerio.emit(powerio.PioModule.from_value(value), format, destination)


def _emit_module(module):
    return powerio.serialize(module).text


def _parse_module(text):
    return powerio.deserialize(io.StringIO(text))


def _emit_file(path, format, from_=None, destination=None, **_options):
    module = powerio.parse(path, format=from_)
    return powerio.emit(module, format, destination)


def _emit_text(text, format, from_="matpower", *, name="fixture", **_options):
    module = powerio.parse(io.StringIO(text), name=name, format=from_)
    return powerio.emit(module, format)


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
    return powerio.parse(DATA / f"{name}.m").value


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
    assert net.n_generators == len(net.generators) == 2
    assert net.n_islands == 1
    assert net.base_frequency == 60.0
    for table, count in [
        ("storage", "n_storage"),
        ("static_var_compensators", "n_static_var_compensators"),
        ("hvdc", "n_hvdc"),
        ("transformers_3w", "n_transformers_3w"),
        ("areas", "n_areas"),
    ]:
        rows = getattr(net, table)
        assert isinstance(rows, list)
        assert len(rows) == getattr(net, count)


def test_xiidm_balanced_equipment_is_available(tmp_path):
    source = tmp_path / "svc.xiidm"
    source.write_text(
        """<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" xmlns:apc="http://www.powsybl.org/schema/iidm/ext/active_power_control/1_2" id="svc" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="225" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B" v="225" angle="0"/></iidm:busBreakerTopology>
      <iidm:generator id="GEN" energySource="OTHER" minP="0" maxP="200" voltageRegulatorOn="false" targetP="50" targetQ="0" bus="B" connectableBus="B"><iidm:regulatingTerminal id="BAT"/><iidm:minMaxReactiveLimits minQ="-100" maxQ="100"/></iidm:generator>
      <iidm:battery id="BAT" targetP="20" targetQ="0" minP="-100" maxP="100" bus="B" connectableBus="B"><iidm:minMaxReactiveLimits minQ="-25" maxQ="25"/></iidm:battery>
      <iidm:staticVarCompensator id="SVC" bMin="-0.02" bMax="0.03" voltageSetpoint="225" reactivePowerSetpoint="12" regulationMode="REACTIVE_POWER" regulating="true" bus="B" connectableBus="B" p="1" q="2"/>
      <iidm:shuntCompensator id="SH" sectionCount="2" voltageRegulatorOn="false" bus="B" connectableBus="B"><iidm:shuntLinearModel gPerSection="0" bPerSection="0.001" maximumSectionCount="4"/></iidm:shuntCompensator>
    </iidm:voltageLevel>
  </iidm:substation>
  <iidm:extension id="GEN"><apc:activePowerControl participate="false" droop="3.0" participationFactor="1.0" maxTargetP="180.0"/></iidm:extension>
  <iidm:extension id="BAT"><apc:activePowerControl participate="true" droop="4.0" participationFactor="1.2" minTargetP="10.0"/></iidm:extension>
</iidm:network>
""",
        encoding="utf-8",
    )

    network = powerio.parse(source).value
    assert network.n_static_var_compensators == 1
    svc = network.static_var_compensators[0]
    assert svc["uid"] == "SVC"
    assert svc["bus"] == network.buses[0]["id"]
    assert svc["regulation_mode"] == "reactive_power"
    assert svc["reactive_power_setpoint_mvar"] == 12.0
    assert network.shunts[0]["section_count"] == 2

    generator_control = network.generators[0]["active_power_control"]
    assert network.generators[0]["energy_source"] == "other"
    assert generator_control == {
        "participate": False,
        "droop_percent": 3.0,
        "participation_factor": 1.0,
        "minimum_target_active_power_mw": None,
        "maximum_target_active_power_mw": 180.0,
    }
    assert network.generators[0]["voltage_regulation_on"] is False
    assert network.generators[0]["regulated_bus"] is None
    assert network.generators[0]["regulating_terminal"] == {
        "equipment": {"component_type": "storage", "local_id": "BAT"},
        "terminal": 1,
    }
    storage_control = network.storage[0]["active_power_control"]
    assert storage_control == {
        "participate": True,
        "droop_percent": 4.0,
        "participation_factor": 1.2,
        "minimum_target_active_power_mw": 10.0,
    }


def test_xiidm_subnetworks_boundary_lines_and_tie_lines_are_available():
    source = """<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="Merged" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="root" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:network id="A" caseDate="2026-01-01T01:00:00Z" forecastDistance="1" sourceFormat="part-a" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
    <iidm:substation id="SA"><iidm:voltageLevel id="VA" nominalV="100" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="BA"/></iidm:busBreakerTopology><iidm:boundaryLine id="DLA" p0="5" q0="6" r="1" x="2" generationVoltageRegulationOn="true" generationMinP="0" generationMaxP="20" generationTargetP="10" generationTargetV="100" bus="BA" connectableBus="BA"><iidm:reactiveCapabilityCurve><iidm:property name="owner" value="RTE"/><iidm:point p="0" minQ="-10" maxQ="10"/><iidm:point p="10" minQ="0" maxQ="20"/></iidm:reactiveCapabilityCurve></iidm:boundaryLine></iidm:voltageLevel></iidm:substation>
  </iidm:network>
  <iidm:network id="B" caseDate="2026-01-01T02:00:00Z" forecastDistance="2" sourceFormat="part-b" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
    <iidm:substation id="SB"><iidm:voltageLevel id="VB" nominalV="100" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="BB"/></iidm:busBreakerTopology><iidm:boundaryLine id="DLB" p0="-5" q0="-6" r="3" x="4" bus="BB" connectableBus="BB"/></iidm:voltageLevel></iidm:substation>
  </iidm:network>
  <iidm:tieLine id="TL" boundaryLineId1="DLA" boundaryLineId2="DLB"/>
</iidm:network>
"""

    network = powerio.parse(
        io.StringIO(source), name="merged.xiidm", format="xiidm"
    ).value
    details = network.detailed_connectivity
    assert details is not None

    subnetworks = details["subnetworks"]
    assert len(subnetworks) == 2
    assert subnetworks[0]["component"] == {
        "component_type": "subnetwork",
        "local_id": "A",
    }
    assert subnetworks[0]["parent"]["local_id"] == "Merged"
    assert subnetworks[0]["case_metadata"]["forecast_distance"] == 1
    assert subnetworks[0]["case_metadata"]["source_model_format"] == "part-a"
    assert len(subnetworks[0]["components"]) >= 4

    boundary = details["boundary_lines"][0]
    assert boundary["component"]["local_id"] == "DLA"
    assert boundary["voltage_level"]["local_id"] == "VA"
    assert boundary["active_power_setpoint_mw"] == 5.0
    assert boundary["reactive_power_setpoint_mvar"] == 6.0
    assert boundary["resistance_ohm"] == 1.0
    assert boundary["reactance_ohm"] == 2.0
    assert boundary["generation"]["voltage_regulation_on"] is True
    assert boundary["generation"]["target_active_power_mw"] == 10.0
    reactive_limits = boundary["generation"]["reactive_limits"]
    assert reactive_limits["kind"] == "capability_curve"
    assert reactive_limits["limits"]["properties"] == {"owner": "RTE"}
    assert reactive_limits["limits"]["points"][1]["maximum_reactive_power_mvar"] == 20.0

    tie = details["tie_lines"][0]
    assert tie["component"]["local_id"] == "TL"
    assert tie["boundary_line1"]["local_id"] == "DLA"
    assert tie["boundary_line2"]["local_id"] == "DLB"
    assert tie["calculation_branch"]["component_type"] == "branch"


def test_xiidm_unassigned_tap_position_remains_absent(case9, tmp_path):
    assert case9.detailed_connectivity is None
    source = """<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/equipment/1_12" id="tap" caseDate="2021-01-03T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="EQUIPMENT">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="225" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="B1"/><iidm:bus id="B2"/></iidm:busBreakerTopology></iidm:voltageLevel>
    <iidm:twoWindingsTransformer id="T" r="1" x="10" g="0" b="0" ratedU1="225" ratedU2="225" voltageLevelId1="VL" bus1="B1" connectableBus1="B1" voltageLevelId2="VL" bus2="B2" connectableBus2="B2">
      <iidm:ratioTapChanger lowTapPosition="0" loadTapChangingCapabilities="false"><iidm:step rho="1"/></iidm:ratioTapChanger>
    </iidm:twoWindingsTransformer>
  </iidm:substation>
</iidm:network>
"""

    module = powerio.parse(io.StringIO(source), name="tap.xiidm", format="xiidm")
    network = module.value
    tap_changer = network.detailed_connectivity["tap_changers"][0]
    assert "component" not in network.detailed_connectivity["terminals"][0]
    assert "component" not in tap_changer
    assert "tap_position" not in tap_changer
    assert tap_changer["low_tap_position"] == 0
    assert tap_changer["steps"][0]["rho"] == 1.0

    destination = tmp_path / "tap-cgmes"
    powerio.emit(module, "cgmes", destination)
    cgmes_details = powerio.parse(
        destination, format="cgmes"
    ).value.detailed_connectivity
    assert cgmes_details["terminals"][0]["component"]["component_type"] == "terminal"
    assert (
        cgmes_details["tap_changers"][0]["component"]["component_type"] == "tap_changer"
    )


def test_xiidm_connectivity_node_numbers_are_available():
    source = """<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="nodes" caseDate="2025-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="110" topologyKind="NODE_BREAKER">
      <iidm:nodeBreakerTopology>
        <iidm:bus v="110" angle="0" nodes="0,1,2"/>
        <iidm:busbarSection id="BBS" node="2"/>
        <iidm:switch id="BR" kind="BREAKER" open="false" node1="1" node2="2"/>
        <iidm:internalConnection node1="0" node2="1"/>
      </iidm:nodeBreakerTopology>
      <iidm:generator id="G" energySource="OTHER" minP="0" maxP="10" voltageRegulatorOn="true" targetP="5" node="0"><iidm:minMaxReactiveLimits minQ="-2" maxQ="2"/></iidm:generator>
    </iidm:voltageLevel>
  </iidm:substation>
</iidm:network>
"""

    details = powerio.parse(
        io.StringIO(source), name="nodes.xiidm", format="xiidm"
    ).value.detailed_connectivity
    nodes = details["connectivity_nodes"]
    assert sorted(node["node_number"] for node in nodes) == [0, 1, 2]
    assert all(node["calculated_bus"] == 1 for node in nodes)
    assert details["equipment_reactive_limits"][0]["equipment"]["local_id"] == "G"


def test_xiidm_equipment_omissions_and_reactive_curve_are_available():
    source = """<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/equipment/1_12" id="equipment" caseDate="2021-01-03T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="EQUIPMENT">
  <iidm:substation id="S"><iidm:voltageLevel id="VL" nominalV="225" topologyKind="NODE_BREAKER">
    <iidm:nodeBreakerTopology><iidm:busbarSection id="BBS" node="0"/></iidm:nodeBreakerTopology>
    <iidm:generator id="G" energySource="SOLAR" minP="0" maxP="100" voltageRegulatorOn="true" node="0">
      <iidm:reactiveCapabilityCurve><iidm:property name="curve" value="retained"/><iidm:point p="0" minQ="-20" maxQ="20"><iidm:property name="point" value="first"/></iidm:point><iidm:point p="100" minQ="-10" maxQ="10"/></iidm:reactiveCapabilityCurve>
    </iidm:generator>
  </iidm:voltageLevel></iidm:substation>
</iidm:network>
"""

    network = powerio.parse(
        io.StringIO(source), name="equipment.xiidm", format="xiidm"
    ).value
    assert network.generators[0]["energy_source"] == "solar"
    details = network.detailed_connectivity
    assert [field["field"] for field in details["omitted_fields"]] == [
        "active_power",
        "reactive_power",
        "voltage_setpoint",
        "rated_apparent_power",
    ]
    limits = details["equipment_reactive_limits"][0]
    assert limits["equipment"]["local_id"] == "G"
    assert limits["limits"]["kind"] == "capability_curve"
    curve = limits["limits"]["limits"]
    assert curve["properties"] == {"curve": "retained"}
    assert curve["points"][0]["properties"] == {"point": "first"}


def test_public_type_is_balanced_network(case9):
    assert isinstance(case9, powerio.BalancedNetwork)
    assert "BalancedNetwork" in powerio.__all__
    assert "EmitResult" in powerio.__all__
    assert not hasattr(powerio, "Conversion")
    assert not hasattr(powerio, "Case")
    assert not hasattr(powerio, "Network")
    assert "Network" not in powerio.__all__
    representation = repr(case9)
    assert representation.startswith("BalancedNetwork(")
    assert "n_generators=3" in representation
    assert "n_gens" not in representation


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


def test_resolve_format_reports_canonical_artifact_metadata():
    psse = powerio.resolve_format("raw34")
    assert psse == powerio.FormatInfo("psse34", "raw", False, True)
    assert powerio.resolve_format("AUX").token == "powerworld"

    pypsa = powerio.resolve_format("pypsa")
    assert pypsa == powerio.FormatInfo("pypsa-csv", None, True, True)

    pwb = powerio.resolve_format("pwb")
    assert pwb.can_emit is False
    assert powerio.resolve_format("pio-json") is None
    assert powerio.resolve_format("json") is None
    assert powerio.resolve_format("not-a-format") is None
    assert "FormatInfo" in powerio.__all__
    assert "resolve_format" in powerio.__all__


def test_private_extension_stub_contains_only_final_boundary_helpers():
    native = importlib.import_module("powerio._powerio")
    assert not hasattr(native, "parse_display_bytes")

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
    assert hidden_functions.isdisjoint(dir(native))

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
    assert {"from_file", "from_str", "from_bytes", "kind", "to_format", "write_file"}.isdisjoint(
        stubbed_methods
    )

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

    assert hasattr(powerio._powerio._PioModule, "_parse_path")
    assert not hasattr(powerio, "convert_file")


def test_parse_infers_format_from_extension():
    case = powerio.parse(DATA / "case9.m").value
    assert case.n_buses == 9
    assert case.source_format == "matpower"


def test_parse_is_the_only_public_grid_exchange_input_operation():
    path = DATA / "case9.m"
    sources = [
        path,
        path.read_bytes(),
        memoryview(path.read_bytes()),
        io.StringIO(path.read_text()),
        io.BytesIO(path.read_bytes()),
    ]
    for source in sources:
        module = powerio.parse(source, format="matpower", name=None if source is path else "case9.m")
        assert isinstance(module.value, powerio.BalancedNetwork)
        assert module.value.n_buses == 9
    assert {"parse_file", "parse_text", "parse_bytes"}.isdisjoint(dir(powerio))
    assert {"kind", "emit", "export_state", "list_states"}.isdisjoint(
        dir(powerio.PioModule)
    )


def test_opfdata_parses_to_its_solved_calculation():
    path = DATA / "opfdataset" / "example_0.json"
    module = powerio.parse(path)
    value = module.value
    assert isinstance(value, powerio.AcOpfSolution)
    assert value.module is module

    emitted = powerio.emit(module, "matpower")
    assert "mpc.version" in emitted.text
    assert "EMIT.SOLUTION.DATA_OMITTED" in {
        diagnostic.code for diagnostic in emitted.diagnostics
    }
    assert (
        json.loads(powerio.serialize(module).text)["value"]["type"]
        == "powerio.AcOpfSolution"
    )


def test_value_uses_actual_python_types():
    module = powerio.parse(DATA / "case9.m")
    assert isinstance(module, powerio.PioModule)
    assert isinstance(module.value, powerio.BalancedNetwork)
    assert isinstance(
        powerio.parse(DATA / "dist" / "micro" / "xfmr_single_phase.dss").value,
        powerio.dist.MulticonductorNetwork,
    )


def test_balanced_network_module_builds_typed_calculation_instances():
    source = powerio.parse(DATA / "case9.m")
    builders = [
        (source.to_dc_pf_instance, powerio.DcPfInstance, "powerio.DcPfInstance"),
        (source.to_ac_pf_instance, powerio.AcPfInstance, "powerio.AcPfInstance"),
        (source.to_dc_opf_instance, powerio.DcOpfInstance, "powerio.DcOpfInstance"),
        (source.to_ac_opf_instance, powerio.AcOpfInstance, "powerio.AcOpfInstance"),
    ]
    for build, value_type, structural_type in builders:
        module = build()
        assert isinstance(module.value, value_type)
        document = json.loads(powerio.serialize(module).text)
        assert document["schema"] == "powerio.module"
        assert document["version"] == 1
        assert document["value"]["type"] == structural_type
        assert document["history"][-1]["kind"] == "transform"


def test_calculation_instance_builder_requires_a_balanced_network_module():
    solution = powerio.parse(DATA / "opfdataset" / "example_0.json")
    with pytest.raises(powerio.PowerIOError):
        solution.to_dc_opf_instance()
    assert isinstance(
        powerio.parse(DATA / "opfdataset" / "example_0.json").value,
        powerio.AcOpfSolution,
    )


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
        "OperatingPoint",
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
        "SocwrOpfSolution",
        "McAcPfSolution",
        "McAcOpfSolution",
        "AcScucSolution",
    ]:
        assert name in powerio.__all__, name
        assert hasattr(powerio, name), name


def test_scuc_record_classes_are_exported():
    for name in [
        "Residuals",
        "ScucActiveReserveZone",
        "ScucBranchSwitchingCost",
        "ScucContingency",
        "ScucDevice",
        "ScucDeviceOutputs",
        "ScucDevicePeriod",
        "ScucEnergyCostBlock",
        "ScucEnergyRequirement",
        "ScucInitialCommitment",
        "ScucInputs",
        "ScucNetworkOutputs",
        "ScucRampLimits",
        "ScucReactiveCapability",
        "ScucReactiveReserveZone",
        "ScucReserveCosts",
        "ScucReserveLimits",
        "ScucShunt",
        "ScucStartupCostAdjustment",
        "ScucStartupLimit",
        "ScucTransformerControl",
        "ScucViolationCosts",
    ]:
        assert name in powerio.__all__, name
        assert hasattr(powerio, name), name


def test_goc3_instance_exposes_typed_scuc_inputs():
    instance = powerio.parse(DATA / "goc3" / "goc3_small.json").value
    assert isinstance(instance, powerio.AcScucInstance)
    assert isinstance(instance.network, powerio.BalancedNetwork)

    inputs = instance.inputs
    assert isinstance(inputs, powerio.ScucInputs)
    assert inputs.interval_durations == (1.0, 1.0)
    assert isinstance(inputs.devices, tuple)
    assert len(inputs.devices) == 2

    producer, consumer = inputs.devices
    assert isinstance(producer, powerio.ScucDevice)
    assert producer.id == powerio.ComponentId("generator", "sd_00")
    assert producer.kind == "producer"
    assert producer.startup_cost == 2.0
    assert producer.startup_limits[0].maximum_startups == 1
    assert producer.energy_upper_bounds[0].energy == 9.0
    assert producer.energy_lower_bounds[0].energy == 1.0
    assert producer.initial_on_status is True
    assert producer.initial_commitment.accumulated_up_time == 4.0
    assert producer.ramp_limits.startup == 1.0
    assert producer.reserve_limits.synchronized == 0.0
    assert producer.reactive_capability.kind == "none"
    assert len(producer.periods) == 2
    assert isinstance(producer.periods[0], powerio.ScucDevicePeriod)
    assert producer.periods[0].energy_cost_blocks[0].marginal_cost == 10.0
    assert producer.periods[0].energy_cost_blocks[0].block_size == 5.0
    assert isinstance(producer.periods[0].reserve_costs, powerio.ScucReserveCosts)

    assert consumer.id == powerio.ComponentId("load", "sd_01")
    assert consumer.kind == "consumer"
    assert isinstance(inputs.shunts[0], powerio.ScucShunt)
    assert inputs.shunts[0].conductance_per_step == 0.0
    assert inputs.shunts[0].susceptance_per_step == 3.0
    assert inputs.shunts[0].initial_step == 1
    assert isinstance(
        inputs.branch_switching_costs[0], powerio.ScucBranchSwitchingCost
    )
    assert isinstance(inputs.transformer_controls[0], powerio.ScucTransformerControl)

    active_zone = inputs.active_reserve_zones[0]
    assert isinstance(active_zone, powerio.ScucActiveReserveZone)
    assert active_zone.buses == (
        powerio.ComponentId("bus", "bus_00"),
        powerio.ComponentId("bus", "bus_01"),
    )
    assert active_zone.ramping_up_requirement == (0.0, 0.0)
    assert isinstance(inputs.reactive_reserve_zones[0], powerio.ScucReactiveReserveZone)
    assert len(inputs.contingencies) == 3
    assert inputs.contingencies[0].id == powerio.ComponentId("contingency", "ctg_00")
    assert inputs.contingencies[0].components == (
        powerio.ComponentId("branch", "acl_00"),
    )
    assert inputs.violation_costs.active_power_balance == 1.0

    with pytest.raises(AttributeError):
        inputs.interval_durations = (2.0,)  # type: ignore[misc]


def _goc3_output():
    return {
        "time_series_output": {
            "bus": [
                {"uid": "bus_00", "vm": [1.0, 1.01], "va": [0.0, 0.0]},
                {"uid": "bus_01", "vm": [0.99, 0.98], "va": [-0.1, -0.2]},
            ],
            "shunt": [{"uid": "sh_00", "step": [1, 2]}],
            "ac_line": [
                {"uid": "acl_00", "on_status": [1, 1]},
                {"uid": "acl_01", "on_status": [1, 0]},
            ],
            "two_winding_transformer": [
                {
                    "uid": "xf_00",
                    "tm": [1.0, 1.01],
                    "ta": [0.0, 0.01],
                    "on_status": [1, 1],
                }
            ],
            "dc_line": [
                {
                    "uid": "dc_00",
                    "pdc_fr": [0.0, 0.1],
                    "qdc_fr": [0.0, 0.0],
                    "qdc_to": [0.0, 0.0],
                }
            ],
            "simple_dispatchable_device": [
                {
                    "uid": "sd_00",
                    "on_status": [1, 1],
                    "p_on": [0.1, 0.2],
                    "q": [0.0, 0.0],
                    "p_reg_res_up": [0.0, 0.0],
                    "p_reg_res_down": [0.0, 0.0],
                    "p_syn_res": [0.0, 0.0],
                    "p_nsyn_res": [0.0, 0.0],
                    "p_ramp_res_up_online": [0.0, 0.0],
                    "p_ramp_res_down_online": [0.0, 0.0],
                    "p_ramp_res_up_offline": [0.0, 0.0],
                    "p_ramp_res_down_offline": [0.0, 0.0],
                    "q_res_up": [0.0, 0.0],
                    "q_res_down": [0.0, 0.0],
                },
                {
                    "uid": "sd_01",
                    "on_status": [1, 0],
                    "p_on": [0.04, 0.0],
                    "q": [0.0, 0.0],
                    "p_reg_res_up": [0.0, 0.0],
                    "p_reg_res_down": [0.0, 0.0],
                    "p_syn_res": [0.0, 0.0],
                    "p_nsyn_res": [0.0, 0.0],
                    "p_ramp_res_up_online": [0.0, 0.0],
                    "p_ramp_res_down_online": [0.0, 0.0],
                    "p_ramp_res_up_offline": [0.0, 0.0],
                    "p_ramp_res_down_offline": [0.0, 0.0],
                    "q_res_up": [0.0, 0.0],
                    "q_res_down": [0.0, 0.0],
                },
            ],
        }
    }


def test_parse_goc3_problem_and_solution_with_one_parse(tmp_path):
    (tmp_path / "problem.json").write_bytes(
        (DATA / "goc3" / "goc3_small.json").read_bytes()
    )
    (tmp_path / "solution.json").write_text(json.dumps(_goc3_output()))

    solution = powerio.parse(tmp_path)
    value = solution.value
    assert isinstance(value, powerio.AcScucSolution)
    assert isinstance(value.instance, powerio.AcScucInstance)
    assert value.instance.inputs.interval_durations == (1.0, 1.0)
    assert value.termination == "not_reported"
    assert isinstance(value.residuals, powerio.Residuals)
    assert value.residuals.max_active_power_mismatch is None
    assert value.residuals.max_reactive_power_mismatch is None
    assert value.producer is None
    assert isinstance(value.network_outputs, powerio.ScucNetworkOutputs)
    assert value.network_outputs.bus_vm == ((1.0, 0.99), (1.01, 0.98))
    assert isinstance(value.device_outputs, powerio.ScucDeviceOutputs)
    assert value.network_outputs.shunt_step == ((1,), (2,))
    assert value.device_outputs.on_status == ((True, True), (True, False))
    assert value.device_outputs.shutdown_status == ((False, False), (False, True))
    assert value.objective is None
    with pytest.raises(AttributeError):
        value.network_outputs.bus_vm = ()  # type: ignore[misc]
    assert solution._inner._type_name == "powerio.AcScucSolution"
    document = json.loads(powerio.serialize(solution).text)
    assert document["schema"] == "powerio.module"
    assert document["version"] == 1
    assert document["value"]["type"] == "powerio.AcScucSolution"

    emitted = json.loads(powerio.emit(solution, "goc3-json").text)
    assert set(emitted) == {"time_series_output"}
    assert "startup_status" not in emitted["time_series_output"][
        "simple_dispatchable_device"
    ][0]


def test_goc3_solution_requires_the_matching_problem():
    with pytest.raises(powerio.PowerIOError, match="matching problem file"):
        powerio.parse(
            io.StringIO(json.dumps(_goc3_output())),
            name="solution.json",
        )


def test_parse_powerworld_display_file():
    path = DATA / "powerworld" / "ACTIVSg200.pwd"
    parsed = powerio.parse_display(path)

    assert not hasattr(powerio, "parse_display_file")
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
    assert gen["voltage_regulation_on"] is True
    assert gen["regulated_bus"] is None
    assert gen["regulating_terminal"] is None


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
    assert isinstance(named.value, powerio.BalancedNetwork)
    sources = json.loads(_emit_module(named))["sources"]
    assert [s["name"] for s in sources] == ["mycase.m"]

    with pytest.raises(OSError):
        powerio.parse(data.decode(), format="matpower")


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
    module = powerio.parse(path)
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


def test_ir_roundtrip_and_grid_exchange_emission():
    c = powerio.parse(DATA / "case9.m").value
    module = powerio.PioModule.from_value(c)
    back = powerio.deserialize(io.StringIO(powerio.serialize(module).text)).value
    assert isinstance(back, powerio.BalancedNetwork)
    assert back.n_buses == c.n_buses
    assert back.base_mva == c.base_mva

    conv = powerio.emit(module, "powermodels-json")
    assert json.loads(conv.text)["name"] == "case9"
    assert conv.diagnostics == ()
    assert not hasattr(conv, "warnings")
    assert powerio.emit(module, "matpower").text


def test_pio_module_from_value_keeps_records_and_wraps_generated_networks():
    source = DATA / "api_conformance.m"
    parsed = powerio.parse(source)
    wrapped = powerio.PioModule.from_value(parsed.value)
    assert isinstance(wrapped.value, powerio.BalancedNetwork)
    assert wrapped.diagnostics == parsed.diagnostics
    assert powerio.emit(wrapped, "matpower").text.encode() == source.read_bytes()

    generated = powerio.deserialize(io.StringIO(powerio.serialize(parsed).text)).value
    generated_module = powerio.PioModule.from_value(generated)
    assert generated_module.value.n_buses == generated.n_buses
    assert "mpc.baseMVA" in powerio.emit(generated_module, "matpower").text

    with pytest.raises(TypeError, match="typed PowerIO value"):
        powerio.PioModule.from_value(object())


def test_pio_module_emit_uses_dynamic_writer(tmp_path):
    source = DATA / "api_conformance.m"
    module = powerio.parse(source)

    conversion = powerio.emit(module, "matpower")
    assert conversion.text.encode() == source.read_bytes()
    assert conversion.diagnostics == ()
    assert conversion.fidelity == "exact_same_format"
    assert len(conversion.artifacts) == 1

    echoed = tmp_path / "echo.m"
    result = powerio.emit(module, "matpower", echoed)
    assert result.text is None
    assert result.diagnostics == ()
    assert result.artifacts[0].path == str(echoed)
    assert echoed.read_bytes() == source.read_bytes()

    stored_text = powerio.serialize(module).text
    stored_doc = json.loads(stored_text)
    assert stored_doc["source_map"]

    stored = tmp_path / "case.pio.json"
    result = powerio.serialize(module, stored)
    assert result.text is None
    assert result.diagnostics == ()
    assert json.loads(stored.read_text())["value"]["type"] == "powerio.BalancedNetwork"

    # A nonnetwork calculation has the same stored writer through PioModule.
    solved = powerio.parse(DATA / "opfdataset" / "example_0.json")
    solved_text = powerio.serialize(solved).text
    assert json.loads(solved_text)["value"]["type"] == "powerio.AcOpfSolution"


def test_deserialize_rejects_non_v1_powerio_ir():
    document = json.loads(powerio.serialize(powerio.parse(DATA / "case9.m")).text)
    document["version"] = 2

    with pytest.raises(powerio.PowerIOError) as failure:
        powerio.deserialize(io.StringIO(json.dumps(document)))

    assert failure.value.code == "READ.MODULE.UNSUPPORTED"


def test_pio_module_emit_covers_memory_and_file_destinations(tmp_path):
    source = DATA / "api_conformance.m"
    module = powerio.parse(source)

    conversion = powerio.emit(module, "matpower")
    assert conversion.text.encode() == source.read_bytes()
    assert conversion.diagnostics == ()

    destination = tmp_path / "case.raw"
    result = powerio.emit(module, "psse", destination)
    assert result.text is None
    assert all(
        isinstance(diagnostic, powerio.Diagnostic) for diagnostic in result.diagnostics
    )
    assert destination.read_text().startswith("0, 100,")


def test_module_has_fields_without_kind_or_selection_operations():
    module = powerio.parse(DATA / "api_conformance.m")
    assert isinstance(module.value, powerio.BalancedNetwork)
    assert isinstance(module.diagnostics, list)
    for removed in ("kind", "inspect", "list_states", "inspect_state", "export_state"):
        assert not hasattr(module, removed)


def test_typed_collection_protocols_use_contained_values(time_series_powerio_ir):
    series_module = _parse_module(time_series_powerio_ir)
    series = series_module.value
    assert isinstance(series, powerio.TimeSeries)
    assert len(series) == 2
    assert all(isinstance(item, powerio.OperatingPoint) for item in series)
    assert len(series.time_points) == 2
    with pytest.raises(IndexError):
        _ = series[2]

    parsed = json.loads(_emit_module(powerio.parse(DATA / "api_conformance.m")))
    network = parsed["value"]["data"]
    scenario_doc = {
        "schema": "powerio.module",
        "version": parsed["version"],
        "producer": parsed["producer"],
        "value": {
            "type": "powerio.ScenarioSet<powerio.BalancedNetwork>",
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
    assert isinstance(scenarios["peak"], powerio.BalancedNetwork)
    with pytest.raises(KeyError):
        _ = scenarios["winter"]

    peak = scenarios["peak"]
    original_base_rating = scenarios["base"].branches[0]["rate_a"]
    report = powerio.apply_updates(
        peak,
        [
            powerio.NetworkUpdate.set_branch_thermal_rating(
                powerio.ComponentId("branch", "1-2"),
                powerio.ApparentPower.megavolt_amperes(321.0),
            )
        ],
    )
    assert report.changes[0].field == "branch_thermal_rating"
    assert peak.branches[0]["rate_a"] == 321.0
    assert scenarios["peak"].branches[0]["rate_a"] == 321.0
    assert scenarios["base"].branches[0]["rate_a"] == original_base_rating


def test_typed_collections_construct_without_powerio_ir():
    network = powerio.parse(DATA / "case9.m").value
    points = [powerio.TimePoint("h0"), powerio.TimePoint("h1", 3600.0)]
    series = powerio.TimeSeries([network, network], time_points=points)

    assert len(series) == 2
    assert series.time_points == tuple(points)
    assert all(isinstance(value, powerio.BalancedNetwork) for value in series)
    series_module = powerio.PioModule.from_value(series)
    assert json.loads(powerio.serialize(series_module).text)["value"]["type"] == (
        "powerio.TimeSeries<powerio.BalancedNetwork>"
    )

    scenarios = powerio.ScenarioSet(
        {"base": network, "peak": network},
        probabilities={"base": 0.6, "peak": 0.4},
    )
    assert scenarios.keys() == ("base", "peak")
    assert scenarios.scenarios == (
        powerio.Scenario("base", 0.6),
        powerio.Scenario("peak", 0.4),
    )
    assert isinstance(powerio.PioModule.from_value(scenarios).value, powerio.ScenarioSet)

    days = powerio.ScenarioSet(
        {"weekday": series, "weekend": series},
        probabilities={"weekday": 0.7, "weekend": 0.3},
    )
    restored = powerio.deserialize(
        io.StringIO(powerio.serialize(powerio.PioModule.from_value(days)).text)
    ).value
    assert isinstance(restored["weekday"], powerio.TimeSeries)
    assert isinstance(restored["weekday"][0], powerio.BalancedNetwork)


def test_typed_collection_constructors_validate_shape_type_and_probability():
    balanced = powerio.parse(DATA / "case9.m").value
    multiconductor = powerio.parse(
        DATA / "dist" / "micro" / "xfmr_single_phase.dss"
    ).value

    with pytest.raises(powerio.PowerIODataError) as empty:
        powerio.TimeSeries([], time_points=[])
    assert empty.value.code == "VALIDATE.COLLECTION.EMPTY"

    with pytest.raises(powerio.PowerIODataError) as mixed:
        powerio.TimeSeries(
            [balanced, multiconductor],
            time_points=[powerio.TimePoint("a"), powerio.TimePoint("b")],
        )
    assert mixed.value.code == "VALIDATE.COLLECTION.ELEMENT_TYPE"

    with pytest.raises(powerio.PowerIODataError) as shape:
        powerio.TimeSeries([balanced], time_points=[])
    assert shape.value.code == "VALIDATE.TIME_SERIES.SHAPE"

    with pytest.raises(ValueError, match="finite and nonnegative"):
        powerio.TimeSeries(
            [balanced], time_points=[powerio.TimePoint("bad", float("nan"))]
        )

    with pytest.raises(ValueError, match="every scenario ID exactly once"):
        powerio.ScenarioSet(
            {"base": balanced, "peak": balanced},
            probabilities={"base": 1.0},
        )

    with pytest.raises(powerio.PowerIODataError) as probability:
        powerio.ScenarioSet(
            {"base": balanced, "peak": balanced},
            probabilities={"base": 0.8, "peak": 0.8},
        )
    assert probability.value.code == "VALIDATE.SCENARIO.PROBABILITY_SUM"


def test_pio_module_from_value_accepts_instances_and_selected_entries():
    instance = powerio.parse(DATA / "case9.m").to_dc_pf_instance().value
    wrapped = powerio.PioModule.from_value(instance)
    assert isinstance(wrapped.value, powerio.DcPfInstance)

    network = powerio.parse(DATA / "case9.m").value
    scenarios = powerio.ScenarioSet({"base": network})
    selected = powerio.PioModule.from_value(scenarios["base"])
    assert isinstance(selected.value, powerio.BalancedNetwork)


def test_indexed_time_series_entry_updates_its_parent(time_series_powerio_ir):
    module = _parse_module(time_series_powerio_ir)
    operating_point = module.value[1]
    report = powerio.apply_updates(
        operating_point,
        [
            powerio.OperatingPointUpdate.set_load_active_power(
                powerio.ComponentId("load", "bus-5"),
                powerio.ActivePower.megawatts(91.5),
            )
        ],
    )

    assert report.changes[0].field == "load_active_power"
    document = json.loads(powerio.serialize(module).text)
    quantity = document["value"]["data"]["values"][1]["quantities"]
    assert quantity["load_active_power"]["values"][0] == 91.5
    assert document["history"][-1]["kind"] == "edit"


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
    # Categorized errors share one PowerIO base class and are also value
    # errors, so callers can choose the amount of detail they need.
    assert issubclass(powerio.PowerIOParseError, powerio.PowerIOError)
    assert issubclass(powerio.PowerIODataError, powerio.PowerIOError)
    assert issubclass(powerio.PowerIOError, ValueError)


def test_malformed_case_raises_parse_error():
    # A malformed/unparseable case file is a parse-category error.
    with pytest.raises(powerio.PowerIOParseError):
        _parse(
            ("this is not a matpower case").encode(),
            "matpower",
            value_type=powerio.BalancedNetwork,
        )


def test_reference_bus_count_is_data_error():
    two_ref = TINY.replace("\t3\t2\t0", "\t3\t3\t0")  # bus 3: PV -> ref
    with pytest.raises(powerio.PowerIODataError):
        _parse(
            (two_ref).encode(), "matpower", value_type=powerio.BalancedNetwork
        ).value.reference_bus_index()


def test_emit_paths_are_clean_unicode(tmp_path):
    destination = tmp_path / "réseau.raw"
    result = powerio.emit(powerio.parse(DATA / "case9.m"), "psse", destination)
    assert result.artifacts[0].path == str(destination)
    assert "�" not in result.artifacts[0].path
    assert destination.is_file()


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
        "emit_gridfm",
        "emit_dcopf_bundle",
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
        f"m = powerio.parse(r'{DATA / 'case9.m'}')\n"
        "assert powerio.emit(m, 'matpower').text\n"
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
    assert "gencost" in case9.to_ppc()


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
    "rejected",
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
def test_noncanonical_branch_susceptance_formulas_are_refused(case9, rejected):
    with pytest.raises(ValueError, match="branch susceptance formula"):
        case9.calc_ptdf(rejected)


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
    # Only the three documented solver choices are accepted.
    with pytest.raises(ValueError, match="expected 'auto', 'dense', or 'sparse'"):
        case9.calc_lodf(solver="CG")


def test_bad_enum_strings_raise(case9, tmp_path):
    with pytest.raises(ValueError):
        case9.calc_bprime_matrix(scheme="nonsense")
    with pytest.raises(ValueError):
        case9.calc_ptdf(formula="nope")
    with pytest.raises(ValueError):
        case9.calc_ptdf(solver="bogus")


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


# --- convert -----------------------------------------------------------


def test_emit_matpower_echo_is_byte_exact():
    src = (DATA / "case14.m").read_text()
    conv = _emit_file(DATA / "case14.m", "matpower")
    assert conv.text == src
    assert conv.diagnostics == ()


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
        assert isinstance(r.diagnostics, tuple)
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


def test_psse_transformer_name_and_control_view(tmp_path):
    source = tmp_path / "controlled.rawx"
    source.write_text(
        json.dumps(
            {
                "network": {
                    "caseid": {
                        "fields": ["rev", "sbase", "basfrq", "title1"],
                        "data": [35, 100, 60, "controlled"],
                    },
                    "bus": {
                        "fields": ["ibus", "name", "baskv", "ide", "vm", "va"],
                        "data": [[1, "B1", 230, 3, 1, 0], [2, "B2", 18, 1, 1, 0]],
                    },
                    "transformer": {
                        "fields": [
                            "ibus",
                            "jbus",
                            "kbus",
                            "ckt",
                            "cw",
                            "cz",
                            "cm",
                            "name",
                            "stat",
                            "r1_2",
                            "x1_2",
                            "sbase1_2",
                            "windv1",
                            "nomv1",
                            "wdg1rate1",
                            "cod1",
                            "cont1",
                            "node1",
                            "rma1",
                            "rmi1",
                            "vma1",
                            "vmi1",
                            "ntp1",
                            "cnxa1",
                            "windv2",
                            "nomv2",
                        ],
                        "data": [
                            [
                                1,
                                2,
                                0,
                                "T1",
                                1,
                                1,
                                1,
                                "CONTROLLED TRANSFORMER",
                                1,
                                0.01,
                                0.10,
                                100,
                                1.05,
                                230,
                                100,
                                -1,
                                -2,
                                2,
                                1.08,
                                0.92,
                                1.05,
                                0.98,
                                17,
                                0,
                                1,
                                18,
                            ],
                            [
                                1,
                                2,
                                0,
                                "T2",
                                1,
                                1,
                                1,
                                "DC LINE CONTROL",
                                1,
                                0.02,
                                0.20,
                                100,
                                1,
                                230,
                                90,
                                4,
                                0,
                                0,
                                1.08,
                                0.92,
                                0,
                                0,
                                17,
                                0,
                                1,
                                18,
                            ],
                            [
                                1,
                                2,
                                0,
                                "T3",
                                1,
                                1,
                                1,
                                "ASYMMETRIC CONTROL",
                                1,
                                0.03,
                                0.30,
                                100,
                                1,
                                230,
                                80,
                                5,
                                0,
                                0,
                                15,
                                -15,
                                100,
                                -100,
                                21,
                                12.5,
                                1,
                                18,
                            ],
                        ],
                    },
                    "sub": {
                        "fields": ["isub", "name", "lati", "long", "srg"],
                        "data": [[1, "SUB", 0, 0, 0]],
                    },
                    "subnode": {
                        "fields": ["isub", "inode", "name", "ibus", "stat", "vm", "va"],
                        "data": [[1, 1, "T1-H", 1, 1, 1, 0], [1, 2, "T1-L", 2, 1, 1, 0]],
                    },
                    "subterm": {
                        "fields": ["isub", "inode", "type", "eqid", "ibus", "jbus", "kbus"],
                        "data": [[1, 1, "2", "T1", 1, 2, 0], [1, 2, "2", "T1", 2, 1, 0]],
                    },
                }
            }
        ),
        encoding="utf-8",
    )

    branch = powerio.parse(source).value.branches[0]
    assert branch["name"] == "CONTROLLED TRANSFORMER"
    assert branch["control"]["mode"] == "voltage"
    assert branch["control"]["enabled"] is False
    assert branch["control"]["controlled_bus"] == 2
    assert branch["control"]["controlled_bus_on_winding_side"] is True
    terminal = branch["control"]["regulating_terminal"]
    assert terminal["equipment"]["component_type"] == "transformer"
    assert terminal["terminal"] == 2
    assert branch["control"]["tap_position_count"] == 17

    dc_control = next(
        branch
        for branch in powerio.parse(source).value.branches
        if branch["name"] == "DC LINE CONTROL"
    )["control"]
    assert dc_control["mode"] == "dc_line_quantity"
    assert dc_control["enabled"] is True

    asymmetric_control = next(
        branch
        for branch in powerio.parse(source).value.branches
        if branch["name"] == "ASYMMETRIC CONTROL"
    )["control"]
    assert asymmetric_control["mode"] == "asymmetric_active_flow"
    assert asymmetric_control["winding_connection_angle"] == 12.5


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
    assert result.diagnostics == ()
    assert out.read_bytes() == src.read_bytes()


def test_emit_file_destination_writes_the_text_byte_exact(tmp_path):
    src = DATA / "psse" / "case14.raw"
    out = tmp_path / "echo.raw"
    result = _emit_file(src, "psse", destination=out)
    assert result.text is None
    assert result.diagnostics == ()
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
    assert conv.diagnostics == ()


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


def test_gridfm_public_surface_uses_parse_and_emit(case9, tmp_path):
    for removed in ("emit_gridfm_batch", "write_gridfm_batch", "read_gridfm"):
        assert removed not in powerio.__all__
        assert not hasattr(powerio, removed)
    assert not hasattr(case9, "emit_gridfm")

    # `resolve_format` describes the format, not the build; the feature probe
    # says whether this extension compiled the GridFM parser and emitter.
    if not powerio.features()["gridfm"]:
        pytest.skip("this extension was built without the gridfm feature")
    info = powerio.resolve_format("gridfm")
    assert info is not None and info.can_emit
    result = powerio.emit(
        powerio.PioModule.from_value(case9), "gridfm", tmp_path / "dataset"
    )
    assert result.layout == "directory"
    assert result.artifacts


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
        "xiidm",
        "cgmes",
        "dgs",
    ]
    root = Path(__file__).resolve().parents[1] / "powerio"
    for stub in ("__init__.pyi", "_powerio.pyi"):
        text = (root / stub).read_text()
        for v in variants:
            assert f'"{v}"' in text, f"{stub} missing source_format {v!r}"


def test_direct_dc_operations_follow_powermodels_orientation_and_sign():
    """Named DC calculations match the PowerModels orientation and sign."""
    net = powerio.parse(DATA / "api_conformance.m").value
    A = net.calc_incidence_matrix()
    Bf = net.calc_branch_flow_matrix()
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
    branch_shift_injection = net.calc_branch_phase_shift_injection()
    shift_injection = net.calc_bus_phase_shift_injection()
    bus_injection = net.calc_bus_injection_dc(va)
    np.testing.assert_allclose(
        branch_flow,
        branch_shift_injection,
    )
    np.testing.assert_allclose(branch_shift_injection, susceptance * shift)
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
        net.calc_branch_flow_matrix("tap_adjusted_reactance").toarray(),
        net.calc_branch_flow_matrix("reactance_only").toarray(),
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
    net = powerio.parse(DATA / "t_case9_oos.m").value
    assert any(not branch["in_service"] for branch in net.branches)

    n_active = sum(branch["in_service"] for branch in net.branches)
    assert net.calc_incidence_matrix().shape == (n_active, net.n_buses)
    assert net.calc_branch_flow_matrix().shape == (n_active, net.n_buses)
    assert net.calc_branch_flow_dc(np.zeros(net.n_buses)).shape == (n_active,)

    # Bus axis calculations remain complete; inactive branches contribute no
    # operator row.
    assert net.calc_bus_susceptance_matrix().shape == (net.n_buses, net.n_buses)
    assert net.calc_branch_susceptances().shape == (n_active,)
    assert net.calc_branch_phase_shift_injection().shape == (n_active,)
    assert net.calc_bus_phase_shift_injection().shape == (net.n_buses,)
    assert net.calc_bus_injection_dc(np.zeros(net.n_buses)).shape == (net.n_buses,)


def test_typed_updates_use_stable_component_ids_and_explicit_units():
    for removed in (
        "load_active_power",
        "load_reactive_power",
        "generator_active_power",
        "generator_reactive_power",
        "generator_voltage_magnitude",
        "generator_in_service",
        "branch_in_service",
        "transformer_tap_ratio",
        "transformer_phase_shift",
        "switch_closed",
    ):
        assert not hasattr(powerio.OperatingPointUpdate, removed)
    assert not hasattr(powerio.NetworkUpdate, "branch_thermal_rating")

    module = powerio.parse(DATA / "case9.m")
    component_id = powerio.ComponentId("load", "bus-5")
    assert component_id == powerio.ComponentId("load", "bus-5")
    assert hash(component_id) == hash(powerio.ComponentId("load", "bus-5"))
    assert component_id != powerio.ComponentId("generator", "bus-5")
    update = powerio.OperatingPointUpdate.set_load_active_power(
        component_id,
        powerio.ActivePower.watts(91_500_000.0),
    )

    report = powerio.apply_updates(module, [update])

    assert module.value.loads[0]["p"] == 91.5
    assert report.connectivity_changed is False
    assert len(report) == 1
    assert report.changes[0].component_id.component_type == "load"
    assert report.changes[0].component_id.local_id == "bus-5"
    assert report.changes[0].field == "load_active_power"
    assert report.changes[0].terminal is None


def test_bus_load_allocation_is_owned_by_powerio():
    module = powerio.parse(DATA / "case9.m").to_dc_opf_instance()
    report = powerio.apply_bus_load_active_power(
        module,
        5,
        powerio.ActivePower.megawatts(125.0),
    )
    document = json.loads(powerio.serialize(module).text)
    loads = document["value"]["data"]["network"]["loads"]
    load = next(row for row in loads if row["bus"] == 5)

    assert load["p"] == 125.0
    assert [change.component_id.local_id for change in report.changes] == ["bus-5"]
    assert [change.field for change in report.changes] == ["load_active_power"]

    with pytest.raises(ValueError, match="allocation"):
        powerio.apply_bus_load_active_power(
            module,
            5,
            powerio.ActivePower.megawatts(130.0),
            allocation="first_load",
        )

    powerio.apply_bus_load_active_power(
        module,
        5,
        powerio.ActivePower.megawatts(0.0),
    )
    restored = powerio.apply_bus_load_active_power(
        module,
        5,
        powerio.ActivePower.megawatts(90.0),
        allocation="equal",
    )
    assert module.value.network.loads[0]["p"] == 90.0
    assert [change.component_id.local_id for change in restored.changes] == ["bus-5"]


def test_network_update_uses_mva_and_reports_the_exact_field():
    module = powerio.parse(DATA / "case9.m")
    update = powerio.NetworkUpdate.set_branch_thermal_rating(
        powerio.ComponentId("branch", "1-4"),
        powerio.ApparentPower.megavolt_amperes(333.0),
    )

    report = powerio.apply_updates(module, [update])

    assert module.value.branches[0]["rate_a"] == 333.0
    assert [change.field for change in report.changes] == ["branch_thermal_rating"]
    assert report.connectivity_changed is False


def test_update_batch_is_atomic_and_rejects_untyped_dictionaries():
    module = powerio.parse(DATA / "case9.m")
    before = module.value.loads[0]["p"]
    valid = powerio.OperatingPointUpdate.set_load_active_power(
        powerio.ComponentId("load", "bus-5"),
        powerio.ActivePower.megawatts(91.5),
    )
    invalid = powerio.OperatingPointUpdate.set_load_active_power(
        powerio.ComponentId("load", "missing"),
        powerio.ActivePower.megawatts(1.0),
    )

    with pytest.raises(powerio.PowerIOError):
        powerio.apply_updates(module, [valid, invalid])
    assert module.value.loads[0]["p"] == before

    with pytest.raises(TypeError, match="typed updates"):
        powerio.apply_updates(module, [{"load": "bus-5", "p": 91.5}])
    assert module.value.loads[0]["p"] == before


def test_service_update_reports_connectivity_change():
    module = powerio.parse(DATA / "case9.m")
    update = powerio.OperatingPointUpdate.set_branch_in_service(
        powerio.ComponentId("branch", "1-4"), False
    )

    report = powerio.apply_updates(module, [update])

    assert module.value.branches[0]["in_service"] is False
    assert report.connectivity_changed is True
    assert report.changes[0].field == "branch_in_service"


def test_calculation_updates_are_constructed_from_typed_updates():
    operating = powerio.OperatingPointUpdate.set_generator_in_service(
        powerio.ComponentId("generator", "bus-1"), True
    )
    network = powerio.NetworkUpdate.set_branch_thermal_rating(
        powerio.ComponentId("branch", "1-4"),
        powerio.ApparentPower.volt_amperes(250_000_000.0),
    )

    assert powerio.CalculationUpdate(operating).data_role == "operating_point"
    assert powerio.CalculationUpdate(network).data_role == "network"
    assert not hasattr(powerio.CalculationUpdate, "operating_point")
    assert not hasattr(powerio.CalculationUpdate, "network")
