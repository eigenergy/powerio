"""Tests for the powerio Python bindings.

Run with `pytest python/tests` after `maturin develop`. The matrix and graph
tests need the optional extras: `pip install '.[all]'`.
"""

import json
import math
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest
import scipy.io
import scipy.sparse as sp

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
SCOPF_SMALL = (
    Path(__file__).resolve().parents[2]
    / "powerio-prob"
    / "tests"
    / "data"
    / "goc3_small.json"
)
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
    return powerio.parse_file(DATA / f"{name}.m")


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
    assert case9.n_gens == 3
    assert case9.base_mva == 100.0
    assert not case9.is_radial  # case9 is meshed
    assert case9.n_connected_components == 1


def test_parse_scopf_uses_public_versioned_json_document():
    instance = powerio.parse_scopf(SCOPF_SMALL.read_text())
    assert instance["schema"] == "powerio.scopf"
    assert instance["powerio_version"] == powerio.__version__
    assert instance["index_base"] == 0
    body = instance["instance"]
    static = body["static"]
    assert body["lengths"]["I"] == 2
    assert body["dt"] == [1.0, 1.0]
    assert static["acl_branch"][0]["j_ln"] == 0
    assert [row["u_0"] for row in static["acl_branch"]] == [1, 0]
    assert static["acx_branch"][0]["u_0"] == 1
    assert (static["prod"][0]["j_dev"], static["prod"][0]["j_sdd"]) == (0, 0)
    assert (static["cons"][0]["j_dev"], static["cons"][0]["j_sdd"]) == (0, 1)
    assert static["prod"][0]["u_0"] == 1
    assert static["active_reserve_set_pr"][0]["j_dev"] == 0
    assert static["active_reserve_set_pr"][0]["j_sdd"] == 0

    one_based = powerio.parse_scopf(SCOPF_SMALL.read_text(), index_base=1)
    assert one_based["index_base"] == 1
    assert one_based["instance"]["static"]["acl_branch"][0]["j_ln"] == 1
    assert one_based["instance"]["static"]["bus"][0]["i"] == static["bus"][0]["i"]
    assert one_based["instance"]["static"]["acl_branch"][0]["u_0"] == 1
    assert "parse_scopf" in powerio.__all__


def test_parse_scopf_rejects_bad_input_and_unknown_format():
    with pytest.raises(powerio.PowerIOParseError):
        powerio.parse_scopf("not json")
    with pytest.raises(ValueError, match="unsupported SCOPF source format"):
        powerio.parse_scopf(SCOPF_SMALL.read_text(), from_="matpower")
    with pytest.raises(ValueError, match="index_base must be 0 or 1"):
        powerio.parse_scopf(SCOPF_SMALL.read_text(), index_base=2)


def test_public_type_is_balanced_network(case9):
    assert isinstance(case9, powerio.BalancedNetwork)
    assert "BalancedNetwork" in powerio.__all__
    assert not hasattr(powerio, "Case")
    # Network is the 0.8 bridge alias: same object, DeprecationWarning, gone
    # at 1.0.0; it stays out of __all__ so nothing advertises it.
    import warnings as _warnings

    with _warnings.catch_warnings(record=True) as caught:
        _warnings.simplefilter("always")
        assert powerio.Network is powerio.BalancedNetwork
    assert any(issubclass(w.category, DeprecationWarning) for w in caught)
    assert "Network" not in powerio.__all__
    assert repr(case9).startswith("BalancedNetwork(")


def test_parse_infers_format_from_extension():
    # parse_file dispatches on the extension; a .m file lands on MATPOWER.
    case = powerio.parse_file(DATA / "case9.m")
    assert case.n_buses == 9
    assert case.source_format == "matpower"


def test_opfdata_uses_shared_python_parse_and_conversion_surface():
    path = DATA / "opfdataset" / "example_0.json"
    case = powerio.parse_file(path)

    assert case.source_format == "opfdata-json"
    assert case.n_buses == 14
    assert case.n_branches == 20
    assert case.n_gens == 5
    assert any("solver initial values" in warning for warning in case.read_warnings)

    converted = powerio.convert_file(path, "matpower")
    assert "mpc.bus" in converted.text


def test_parse_powerworld_display_file_and_bytes():
    path = DATA / "powerworld" / "ACTIVSg200.pwd"
    parsed = powerio.parse_display_file(path)
    from_bytes = powerio.parse_display_bytes(path.read_bytes(), "powerworld-pwd")

    assert parsed == from_bytes
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
    case = powerio.parse_str(
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
    case = powerio.parse_file(DATA / "case30.m")
    # MATPOWER folds demand onto the bus row; powerio splits it back out.
    assert case.n_loads > 0
    assert all({"bus", "p", "q", "in_service"} <= set(load) for load in case.loads)
    # buses carry no pd/qd (that's what loads are for)
    assert "pd" not in case.buses[0]


def test_parse_str_roundtrip(case9):
    text = (DATA / "case9.m").read_text()
    c = powerio.parse_str(text, "matpower")
    assert c.name == "case9"
    assert c.n_buses == case9.n_buses
    assert np.allclose(c.bprime().toarray(), case9.bprime().toarray())


def test_parse_str_general():
    text = (DATA / "case9.m").read_text()
    c = powerio.parse_str(text, "matpower")
    assert c.n_buses == 9


def test_to_dense_reports_the_star_lowered_space():
    # A 3-winding transformer lowers to a star bus plus three branches before
    # the matrix builders run. to_dense says it matches that view, so its
    # tables have to be that view: it used to take buses and branches from the
    # case mirror while reading reference_bus, n_components and is_radial off
    # the lowered one, and hand back a struct describing neither.
    case = json.loads(powerio.parse_file(DATA / "case9.m").to_json())
    winding = lambda bus: {  # noqa: E731
        "bus": bus, "tap": 1.0, "shift": 0.0, "nominal_kv": 0.0,
        "rate_a": 0.0, "rate_b": 0.0, "rate_c": 0.0,
    }
    z = {"r": 0.0, "x": 0.05, "base_mva": 100.0}
    case["transformers_3w"] = [{
        "windings": [winding(4), winding(5), winding(6)], "z": [z, z, z],
        "star_vm": 1.0, "star_va": 0.0, "mag_g": 0.0, "mag_b": 0.0,
        "in_service": True, "name": "t3", "extras": {},
    }]
    net = powerio.from_json(json.dumps(case))

    # The element tables stay the case file's own rows.
    assert net.n_buses == 9
    assert net.n_branches == 9

    dense = net.to_dense()
    assert (dense.n, dense.m) == (10, 12)
    assert net.bprime().shape == (dense.n, dense.n)

    ids = set(dense.bus_ids.tolist())
    endpoints = set(dense.branch.from_id.tolist()) | set(dense.branch.to_id.tolist())
    assert endpoints <= ids, "a branch names a bus the dense view does not report"
    assert endpoints == ids, f"isolated bus: {ids - endpoints}"


def test_parse_bytes_reaches_the_binary_reader():
    # PowerWorld binary has no text form, so parse_str cannot read one; this is
    # the in-memory door for an upload or an archive member.
    pwb = (DATA / "powerworld" / "ACTIVSg200.pwb").read_bytes()
    c = powerio.parse_bytes(pwb, "pwb")
    assert c.n_buses == 200
    assert c.n_branches == 246

    # Text formats agree with the path parse.
    m = (DATA / "case9.m").read_bytes()
    assert powerio.parse_bytes(m, "matpower").n_buses == 9

    # Bytes a text format cannot decode raise, rather than blaming the case.
    with pytest.raises(powerio.PowerIOError, match="UTF-8"):
        powerio.parse_bytes(b"\xff\xfe\x00", "matpower")


def test_read_warnings_surface():
    # The genuine pandapower fixture carries a switch table the reader cannot
    # model, so the parse reports it; the MATPOWER reader is total and reports
    # nothing.
    case = powerio.parse_file(DATA / "pandapower" / "example.json")
    assert case.read_warnings
    assert any("switch" in w for w in case.read_warnings)
    assert powerio.parse_file(DATA / "case9.m").read_warnings == []


def test_json_roundtrip_and_parsed_conversion():
    c = powerio.parse_file(DATA / "case9.m")
    back = powerio.from_json(c.to_json())
    assert back.n_buses == c.n_buses
    assert back.base_mva == c.base_mva

    conv = c.to_format("powermodels-json")
    assert json.loads(conv.text)["name"] == "case9"
    assert conv.warnings == []
    assert powerio.to_matpower(c) == c.to_matpower()


def test_source_format_round_trips_through_to_format(case9):
    # `net.to_format(other.source_format)` must work for every format: since
    # 0.9 the property returns the same token every `from`/`to` accepts (the
    # #75 camel-case spellings are retired).
    pm = powerio.parse_str(case9.to_format("powermodels-json").text, "powermodels-json")
    assert pm.source_format == "powermodels-json"
    eg = powerio.parse_str(case9.to_format("egret-json").text, "egret-json")
    assert eg.source_format == "egret-json"
    for other in (case9, pm, eg):
        # The raw source_format string feeds straight back into to_format.
        assert case9.to_format(other.source_format).text


def test_to_dense(case9):
    dense = case9.to_dense()
    assert dense.n == case9.n_buses
    assert dense.m == case9.n_branches
    assert dense.ng == case9.n_gens
    assert list(dense.bus_ids) == [bus["id"] for bus in case9.buses]
    assert dense.branch.from_id.shape == (case9.n_branches,)
    assert dense.gen.pg.shape == (case9.n_gens,)
    assert dense.demand.pd.shape == (case9.n_buses,)
    assert dense.reference_bus == case9.reference_bus_index()


def test_write_is_byte_exact():
    src = (DATA / "case9.m").read_text()
    case = powerio.parse_file(DATA / "case9.m")
    assert case.to_matpower() == src


def test_to_normalized_is_per_unit_and_in_memory(case9):
    n = case9.to_normalized()
    # case9 is fully in service with one reference bus, so nothing is dropped.
    assert n.n_buses == case9.n_buses
    assert n.n_gens == case9.n_gens
    # A derived product with no retained source: it serializes from the model.
    assert n.source_format == "normalized"
    # Powers are per unit (divided by baseMVA).
    g, rg = n.generators[0], case9.generators[0]
    assert abs(g["pmax"] - rg["pmax"] / case9.base_mva) < 1e-9
    # The result is a full BalancedNetwork, so the matrix builders work on it.
    assert n.bprime().shape == (n.n_buses, n.n_buses)


def test_to_normalized_filters_out_of_service():
    case = powerio.parse_file(str(DATA / "t_case9_oos.m"))
    n = case.to_normalized()
    # The fixture marks one generator and one branch out of service; no isolated
    # buses, so every bus survives.
    assert n.n_gens == case.n_gens - 1
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
    n = powerio.parse_str(src, "matpower").to_normalized()
    assert [bus["id"] for bus in n.buses] == [1, 2, 3, 4, 10]
    assert n.loads[0]["bus"] == 10
    assert n.branches[-1]["from_id"] == 4
    assert n.branches[-1]["to_id"] == 10


def test_to_normalized_with_options_clamps_angle_bounds():
    case = powerio.parse_file(DATA / "angle_bounds_clamp.m")

    plain = case.to_normalized()
    assert plain.branches[0]["angmin"] == pytest.approx(-2.0 * math.pi)
    assert plain.branches[0]["angmax"] == pytest.approx(2.0 * math.pi)
    assert plain.branches[1]["angmin"] == pytest.approx(0.0)
    assert plain.branches[1]["angmax"] == pytest.approx(0.0)
    assert plain.branches[3]["angmin"] == pytest.approx(-120.0 * math.pi / 180.0)
    assert plain.branches[3]["angmax"] == pytest.approx(-100.0 * math.pi / 180.0)
    assert plain.branches[4]["angmin"] == pytest.approx(100.0 * math.pi / 180.0)
    assert plain.branches[4]["angmax"] == pytest.approx(120.0 * math.pi / 180.0)

    repaired = case.to_normalized_with_options(clamp_angle_bounds=True)
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
        for warning in repaired.read_warnings
    )
    assert any(
        "branch 1 angle difference bounds clamped" in warning
        for warning in repaired.read_warnings
    )
    assert any(
        "branch 3 angle difference bounds clamped" in warning
        for warning in repaired.read_warnings
    )
    assert any(
        "branch 4 angle difference bounds clamped" in warning
        for warning in repaired.read_warnings
    )

    with pytest.raises(powerio.PowerIODataError):
        case.to_normalized_with_options(
            clamp_angle_bounds=True, angle_bound_pad=math.pi / 2.0
        )


def test_parse_bad_path_raises():
    # I/O failures map to the standard OSError subclass, not PowerIOError.
    with pytest.raises(FileNotFoundError):
        powerio.parse_file(DATA / "does_not_exist.m")


def test_bad_parse_raises_powerio_error():
    with pytest.raises(powerio.PowerIOError):
        powerio.parse_str("this is not a matpower case", "matpower")


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
        powerio.parse_str("this is not a matpower case", "matpower")


def test_unmet_precondition_raises_data_error(tmp_path):
    # A well-formed case that can't satisfy an operation (here: DC-OPF with no
    # generators) is a data-category error, not a parse error.
    genless = TINY[: TINY.index("mpc.gen = [")]
    case = powerio.parse_str(genless, "matpower")
    with pytest.raises(powerio.PowerIODataError):
        case.write_dcopf_bundle(str(tmp_path))


def test_reference_bus_count_is_data_error():
    two_ref = TINY.replace("\t3\t2\t0", "\t3\t3\t0")  # bus 3: PV -> ref
    with pytest.raises(powerio.PowerIODataError):
        powerio.parse_str(two_ref, "matpower").reference_bus_index()


def test_dcopf_bundle_paths_are_clean_unicode(case9, tmp_path):
    # The returned dir/files must be exact strings that re-open the written
    # files, never lossily mangled (no U+FFFD).
    out = case9.write_dcopf_bundle(str(tmp_path))
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
        "n_gens",
        "n_loads",
        "n_shunts",
        "is_radial",
        "n_connected_components",
        "buses",
        "loads",
        "shunts",
        "branches",
        "generators",
        "reference_bus_index",
        "reference_bus_indices",
        "connectivity_report",
        "to_matpower",
        "write_dcopf_bundle",
    ]:
        assert hasattr(case9, attr), attr
    with pytest.raises(AttributeError):
        case9.does_not_exist  # noqa: B018 -- the attribute access is the assertion


def test_import_and_parse_pull_in_no_optional_deps():
    # The zero-dep promise: parse/convert/write need nothing but the
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
        f"c = powerio.parse_file(r'{DATA / 'case9.m'}')\n"
        "assert c.to_matpower()\n"
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
        case9.to_dense()
    with pytest.raises(ImportError, match=r"powerio\[matrix\]"):
        case9.bprime()


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
    b = c.bprime()
    assert sp.issparse(b) and b.format == "csr"
    assert b.shape == (c.n_buses, c.n_buses)
    assert b.indices.dtype == np.int32  # COO indices emitted as i32
    assert is_symmetric(b)
    # Shuntless Laplacian: rows sum to zero and follow the M-matrix sign pattern.
    row_sums = np.asarray(b.sum(axis=1)).ravel()
    assert np.allclose(row_sums, 0.0, atol=1e-8)
    diag = b.diagonal()
    assert np.all(diag > 0)
    off = b - sp.diags(diag)
    assert off.max() <= 1e-12


def test_bprime_xb_equals_weighted_laplacian(case9):
    # Exact cross-check across two boundary paths: Bp in the XB scheme is the
    # reactance-only weighted Laplacian (b = 1/x). Catches a shared bug in
    # the COO conversion that the symmetric self-check can't.
    assert np.allclose(
        case9.bprime("xb").toarray(),
        case9.weighted_laplacian("reactance-only").toarray(),
    )


def test_bdoubleprime_shunts_and_scheme():
    c = load("case30")  # has bus shunts
    bpp = c.bdoubleprime()
    assert bpp.shape == (c.n_buses, c.n_buses)
    # Bpp keeps shunts, so it differs from Bp on this case.
    assert not np.allclose(bpp.toarray(), c.bprime().toarray())
    # The scheme kwarg is wired: BX zeroes line resistance, XB does not.
    assert not np.allclose(c.bdoubleprime("bx").toarray(), c.bdoubleprime("xb").toarray())


@pytest.mark.parametrize("name", SMALL)
def test_ybus_complex_equals_parts(name):
    c = load(name)
    y = c.ybus()
    assert y.dtype == np.complex128 and y.shape == (c.n_buses, c.n_buses)
    g, b = c.ybus_parts()
    assert np.allclose(y.toarray(), (g + 1j * b).toarray())


def test_kwargs_change_output():
    # case14 carries nonzero taps, so taps/scheme are observable here.
    c = load("case14")
    assert not np.allclose(c.bprime("xb").toarray(), c.bprime("bx").toarray())
    assert not np.allclose(
        c.ybus(include_taps=True).toarray(),
        c.ybus(include_taps=False).toarray(),
    )


def test_adjacency_is_binary_symmetric(case9):
    a = case9.adjacency()
    assert a.shape == (9, 9)
    assert is_symmetric(a)
    assert set(np.unique(a.data)).issubset({0.0, 1.0})
    assert a.diagonal().sum() == 0  # no self loops


def test_lacpf_block_shape(case9):
    block = case9.lacpf()
    assert block.shape == (2 * case9.n_buses, 2 * case9.n_buses)


@pytest.mark.parametrize("name", SMALL)
def test_sensitivities(name):
    c = load(name)
    ptdf, lodf = c.ptdf(), c.lodf()
    m, n = ptdf.shape
    assert n == c.n_buses
    assert lodf.shape == (m, m)
    # LODF diagonal is -1 on the monitored = outaged branch.
    assert np.allclose(lodf.diagonal(), -1.0)
    # PTDF references injections to the slack, so the slack column is zero.
    assert np.allclose(ptdf.toarray()[:, c.reference_bus_index()], 0.0, atol=1e-9)


def test_incidence_column_structure(case9):
    # Catches a row/col transpose in the COO conversion that symmetric matrices
    # cannot: each incidence column has +1 at the from bus, -1 at the to bus.
    inc = case9.incidence()
    n, m = inc.A.shape
    assert n == case9.n_buses
    assert (
        len(inc.branch_weight) == m
        and len(inc.p_shift) == n
        and len(inc.branch_of_col) == m
    )
    assert list(inc.branch_of_col) == list(range(m))  # all in service, in order
    assert inc.branch_of_col.dtype == np.int64
    a = inc.A.tocsc()
    idmap = id_to_dense(case9)
    for k in range(m):
        br = case9.branches[inc.branch_of_col[k]]
        col = a[:, k].toarray().ravel()
        assert np.count_nonzero(col) == 2
        assert col[idmap[br["from_id"]]] == 1.0
        assert col[idmap[br["to_id"]]] == -1.0


def test_weighted_laplacian_matches_incidence(case9):
    inc = case9.incidence()
    rebuilt = inc.A @ sp.diags(inc.branch_weight) @ inc.A.T
    assert np.allclose(case9.weighted_laplacian().toarray(), rebuilt.toarray())


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
    g, b = case.ybus_parts()
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
    inc = case.incidence()
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
                case.branches[int(idx)]["from_id"] for idx in inc.branch_of_col
            ],
            "index": list(range(len(inc.branch_of_col))),
            "row_axis": "matrix_branch",
            "powerio_version": powerio.__version__,
            "source_row": [int(idx) for idx in inc.branch_of_col],
            "table": "matrix_branch",
            "to_bus_id": [
                case.branches[int(idx)]["to_id"] for idx in inc.branch_of_col
            ],
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
                case.bdoubleprime(), "bdoubleprime"
            ),
            "bprime": _real_matrix_arrow_payload(case.bprime(), "bprime"),
            "incidence": _real_matrix_arrow_payload(case.incidence().A, "incidence"),
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
    assert back.n_gens == case9.n_gens
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
    net = powerio.parse_str(src, "matpower")
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

    src = case9.to_matpower().split("mpc.gen = [")
    rows = src[1].split("];")[0].strip().split("\n")
    narrow = "\n".join("\t" + "\t".join(r.strip().rstrip(";").split()[:10]) + ";" for r in rows)
    net = powerio.parse_str(f"{src[0]}mpc.gen = [\n{narrow}\n];{src[1].split('];', 1)[1]}", "matpower")
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


def test_convention_aliases(case9):
    # Documented spellings all parse; separator/case-insensitive.
    for conv in ["reactance-only", "REACTANCE_ONLY", "series", "matpower", "mp"]:
        assert sp.issparse(case9.ptdf(conv))
    # 0.8's default spelling. The nearest-looking survivor, "series", is a
    # different formula, so the error has to name the successor or a caller
    # who guesses gets numbers instead of a failure.
    for old in ["paper-pure", "paper", "PAPER_PURE"]:
        with pytest.raises(ValueError, match="reactance-only"):
            case9.ptdf(old)
    for scheme in ["bx", "XB"]:
        assert sp.issparse(case9.bprime(scheme))


def test_sensitivity_solver_kwarg(case9):
    # On a small case the auto policy picks the dense path, so the explicit
    # spellings must agree with the default.
    base = case9.ptdf().toarray()
    assert np.allclose(case9.ptdf(solver="dense").toarray(), base, atol=1e-9)
    assert np.allclose(case9.ptdf(solver="sparse").toarray(), base, atol=1e-10)
    lodf = case9.lodf().toarray()
    assert np.allclose(case9.lodf(solver="dense").toarray(), lodf, atol=1e-9)
    assert np.allclose(case9.lodf(solver="SPARSE").toarray(), lodf, atol=1e-10)
    for old in ["iterative", "cg", "CG"]:
        with pytest.raises(ValueError, match="use 'sparse'"):
            case9.ptdf(solver=old)


def test_bad_enum_strings_raise(case9, tmp_path):
    with pytest.raises(ValueError):
        case9.bprime(scheme="nonsense")
    with pytest.raises(ValueError):
        case9.ptdf(convention="nope")
    with pytest.raises(ValueError):
        case9.ptdf(solver="bogus")
    with pytest.raises(ValueError):
        case9.write_dcopf_bundle(str(tmp_path), units="bogus")


# --- graph view ---------------------------------------------------------


def test_to_networkx_attrs_and_status_filter():
    c = powerio.parse_str(TINY, "matpower")
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
    assert powerio.parse_str(oos, "matpower").to_networkx().number_of_edges() == 1


# --- connectivity & reference bus --------------------------------------


def test_connectivity_report(case9):
    rep = case9.connectivity_report()
    assert rep["n_buses"] == 9
    assert rep["n_components"] == 1
    assert rep["isolated_buses"] == []


def test_reference_bus_index(case9):
    assert case9.reference_bus_index() == 0
    assert case9.reference_bus_indices() == [0]


def test_reference_bus_error_on_two_refs():
    two_ref = TINY.replace("\t3\t2\t0", "\t3\t3\t0")  # bus 3: PV -> ref
    case = powerio.parse_str(two_ref, "matpower")
    # The single-ref query raises; the reference-set query returns both, so a
    # multi-slack case stays legible from Python.
    with pytest.raises(powerio.PowerIOError):
        case.reference_bus_index()
    assert len(case.reference_bus_indices()) == 2


# --- DC-OPF bundle ------------------------------------------------------


def test_write_dcopf_bundle_content(case9, tmp_path):
    out = case9.write_dcopf_bundle(str(tmp_path))
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
    out = case.write_dcopf_bundle(str(out_dir), **kw)
    return next(f for f in out["files"] if Path(f).name == name)


def test_dcopf_requires_generators(tmp_path):
    genless = TINY[: TINY.index("mpc.gen = [")]
    case = powerio.parse_str(genless, "matpower")
    assert case.n_gens == 0
    with pytest.raises(powerio.PowerIOError):
        case.write_dcopf_bundle(str(tmp_path))


# --- convert -----------------------------------------------------------


def test_convert_matpower_echo_is_byte_exact():
    src = (DATA / "case14.m").read_text()
    conv = powerio.convert_file(DATA / "case14.m", "matpower")
    assert conv.text == src
    assert conv.warnings == []


def test_convert_matpower_to_each_format():
    for fmt in ["powermodels-json", "egret-json", "psse", "powerworld", "pandapower-json"]:
        r = powerio.convert_file(str(DATA / "case30.m"), fmt)
        assert isinstance(r.text, str) and len(r.text) > 0
        assert isinstance(r.warnings, list)
    # PowerModels JSON output parses as JSON and keeps the bus count.
    pm = json.loads(powerio.convert_file(str(DATA / "case30.m"), "powermodels-json").text)
    assert len(pm["bus"]) == 30
    pp = json.loads(powerio.convert_file(str(DATA / "case30.m"), "pandapower-json").text)
    assert pp["_class"] == "pandapowerNet"


def test_convert_round_trip_through_psse(tmp_path):
    raw = powerio.convert_file(str(DATA / "case30.m"), "psse").text
    p = tmp_path / "case30.raw"
    p.write_text(raw)
    back = powerio.convert_file(str(p), "matpower")  # PSS/E inferred from .raw extension
    case = powerio.parse_str(back.text, "matpower")
    assert case.n_buses == 30


def test_write_file_preserves_the_crlf_echo(tmp_path):
    # A same-format echo of a CRLF source must reach disk byte exact.
    # Writing to_format text through open(path, "w") corrupts it on Windows
    # (text mode turns each \r\n into \r\r\n); write_file bypasses that.
    src = DATA / "psse" / "case14.raw"
    assert b"\r\n" in src.read_bytes()
    case = powerio.parse_file(src)
    out = tmp_path / "echo.raw"
    warnings = case.write_file(out, "psse")
    assert warnings == []
    assert out.read_bytes() == src.read_bytes()


def test_convert_file_out_writes_the_text_byte_exact(tmp_path):
    src = DATA / "psse" / "case14.raw"
    out = tmp_path / "echo.raw"
    conv = powerio.convert_file(src, "psse", out=out)
    assert out.read_bytes() == conv.text.encode()
    assert out.read_bytes() == src.read_bytes()


def test_convert_psse_start_of_markers_to_powermodels(tmp_path):
    p = tmp_path / "start_markers.raw"
    p.write_text(PSSE_START_OF_MARKERS)

    pm = json.loads(powerio.convert_file(p, "powermodels-json", from_="psse").text)

    assert len(pm["bus"]) == 2
    assert len(pm["load"]) == 1
    assert len(pm["gen"]) == 1
    assert len(pm["branch"]) == 1


def test_convert_unknown_format_raises():
    with pytest.raises(ValueError):
        powerio.convert_file(str(DATA / "case30.m"), "nonsense")


def test_convert_str_matches_convert_file():
    text = (DATA / "case30.m").read_text()
    for fmt in ["powermodels-json", "egret-json", "psse", "powerworld", "pandapower-json"]:
        from_str = powerio.convert_str(text, fmt)
        from_file = powerio.convert_file(str(DATA / "case30.m"), fmt)
        assert from_str.text == from_file.text
        assert from_str.warnings == from_file.warnings


def test_convert_str_matpower_echo_is_byte_exact():
    src = (DATA / "case14.m").read_text()
    conv = powerio.convert_str(src, "matpower")
    assert conv.text == src
    assert conv.warnings == []


def test_to_canonical_format_bypasses_matpower_echo():
    src = (DATA / "case14.m").read_text()
    net = powerio.parse_str(src, "matpower")
    assert net.to_format("matpower").text == src
    canonical = net.to_canonical_format("matpower")
    assert canonical.text != src
    assert powerio.parse_str(canonical.text, "matpower").n_buses == net.n_buses


def test_convert_str_named_input_format():
    raw = powerio.convert_file(str(DATA / "case30.m"), "psse").text
    back = powerio.convert_str(raw, "matpower", from_="psse")
    assert powerio.parse_str(back.text, "matpower").n_buses == 30


def test_pypsa_csv_folder_wrapper(tmp_path):
    case = powerio.parse_file(DATA / "case9.m")
    out = tmp_path / "pypsa"
    result = case.write_pypsa_csv_folder(out)
    assert (out / "network.csv").is_file()
    assert (out / "buses.csv").is_file()
    assert result["dir"] == str(out)
    assert "warnings" in result

    back = powerio.read_pypsa_csv_folder(out)
    assert back.n_buses == case.n_buses
    assert back.n_branches == case.n_branches
    assert back.n_gens == case.n_gens


def test_convert_str_errors():
    with pytest.raises(powerio.PowerIOError):
        powerio.convert_str("not a case", "psse")
    with pytest.raises(ValueError):
        powerio.convert_str((DATA / "case14.m").read_text(), "nonsense")


def test_missing_json_file_raises_oserror():
    # The non-MATPOWER read path must raise OSError too: a missing file is a
    # missing file, not a ValueError, regardless of the inferred format.
    with pytest.raises(OSError):
        powerio.convert_file(DATA / "definitely_missing.json", "matpower")


# --- large case integration --------------------------------------------


def test_large_case_pegase():
    path = DATA / "case2869pegase.m"
    if not path.is_file():
        pytest.skip("case2869pegase.m not vendored")
    c = powerio.parse_file(str(path))
    assert c.n_buses == 2869
    b = c.bprime()
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
    # Custom native builds can compile the write path out, so the wrapper must
    # still raise ImportError rather than AttributeError.
    if HAS_GRIDFM:
        pytest.skip("extension built with gridfm; the absent-path is not exercised")
    with pytest.raises(ImportError, match="gridfm"):
        case9.write_gridfm(str(tmp_path))
    with pytest.raises(ImportError, match="gridfm"):
        powerio.read_gridfm(str(tmp_path))


@gridfm_only
def test_gridfm_write_single(case9, tmp_path):
    pl = pytest.importorskip("polars")
    out = case9.write_gridfm(str(tmp_path))
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
    out = case9.write_gridfm(str(tmp_path), include_y_bus=False)
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
    out = powerio.write_gridfm_batch([case, case], str(tmp_path))
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
def test_read_gridfm_round_trips(case9, tmp_path):
    # write → read back: the recovered BalancedNetwork mirrors the source's element counts
    # and base_mva, surfaces fidelity warnings, sets source_format Gridfm, and is
    # runnable (serializes to MATPOWER and re-parses).
    out = case9.write_gridfm(str(tmp_path))
    r = powerio.read_gridfm(out["dir"])
    assert isinstance(r.network, powerio.BalancedNetwork)
    assert r.scenario == 0
    assert r.warnings and all(isinstance(w, str) for w in r.warnings)
    net = r.network
    assert (net.n_buses, net.n_branches, net.n_gens) == (
        case9.n_buses,
        case9.n_branches,
        case9.n_gens,
    )
    assert net.base_mva == case9.base_mva
    assert net.source_format == "gridfm"
    text = net.to_matpower()
    assert text.startswith("function mpc")
    assert powerio.parse_str(text, "matpower").n_buses == case9.n_buses


@gridfm_only
def test_read_gridfm_is_unpackable(case9, tmp_path):
    # GridfmRead is a namedtuple: tuple-unpack and attribute access both work.
    out = case9.write_gridfm(str(tmp_path))
    net, scenario, warnings = powerio.read_gridfm(out["dir"])
    assert isinstance(net, powerio.BalancedNetwork)
    assert scenario == 0
    assert isinstance(warnings, list)


@gridfm_only
def test_read_gridfm_scenarios_round_trips_each(tmp_path):
    # The batch write stacks two scenarios; the read side rebuilds one BalancedNetwork per
    # scenario id, ascending.
    case = load("case9")
    out = powerio.write_gridfm_batch([case, case], str(tmp_path))
    reads = powerio.read_gridfm_scenarios(out["dir"])
    assert [r.scenario for r in reads] == [0, 1]
    for r in reads:
        assert isinstance(r.network, powerio.BalancedNetwork)
        assert r.network.n_buses == case.n_buses


@gridfm_only
def test_read_gridfm_selects_scenario(tmp_path):
    case = load("case9")
    out = powerio.write_gridfm_batch([case, case], str(tmp_path))
    assert powerio.read_gridfm(out["dir"], scenario=1).scenario == 1


@gridfm_only
def test_read_gridfm_missing_dir_raises(tmp_path):
    # A nonexistent dataset directory surfaces as a powerio error, not a panic.
    with pytest.raises(powerio.PowerIOError):
        powerio.read_gridfm(tmp_path / "does_not_exist")


@gridfm_only
def test_gridfm_in_all_export():
    # The gridfm read/write surface is part of the package's public API.
    for name in (
        "write_gridfm_batch",
        "read_gridfm",
        "read_gridfm_scenarios",
        "GridfmRead",
    ):
        assert name in powerio.__all__
        assert hasattr(powerio, name)


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
