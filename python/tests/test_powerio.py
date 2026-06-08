"""Tests for the powerio Python bindings.

Run with `pytest python/tests` after `maturin develop`. The matrix and graph
tests need the optional extras: `pip install '.[all]'`.
"""

import json
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest
import scipy.io
import scipy.sparse as sp

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
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


def load(name):
    return powerio.parse_matpower(str(DATA / f"{name}.m"))


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
    assert case9.n == 9
    assert case9.n_branches == 9
    assert case9.n_gens == 3
    assert case9.base_mva == 100.0
    assert not case9.is_radial  # case9 is meshed
    assert case9.n_connected_components == 1


def test_parse_infers_format_from_extension():
    # powerio.parse dispatches on the extension; a .m file lands on MATPOWER.
    case = powerio.parse(DATA / "case9.m")
    assert case.n == 9
    assert case.source_format == "Matpower"


def test_case_tables(case9):
    assert len(case9.buses) == 9
    assert len(case9.branches) == 9
    assert len(case9.gens) == 9 - 6  # 3 gens
    bus = case9.buses[0]
    assert bus["id"] == 1 and bus["type"] == "REF"
    gen = case9.gens[0]
    assert gen["cost"]["model"] == 2
    assert gen["cost"]["coeffs"] == [0.11, 5.0, 150.0]


def test_loads_and_shunts_are_first_class():
    case = powerio.parse(DATA / "case30.m")
    # MATPOWER folds demand onto the bus row; powerio splits it back out.
    assert case.n_loads > 0
    assert all({"bus", "p", "q", "in_service"} <= set(l) for l in case.loads)
    # buses carry no pd/qd (that's what loads are for)
    assert "pd" not in case.buses[0]


def test_parse_matpower_string_roundtrip(case9):
    text = (DATA / "case9.m").read_text()
    c = powerio.parse_matpower_string(text, name="from_string")
    assert c.name == "from_string"
    assert c.n == case9.n
    assert np.allclose(c.bprime().toarray(), case9.bprime().toarray())


def test_parse_str_general():
    text = (DATA / "case9.m").read_text()
    c = powerio.parse_str(text, "matpower")
    assert c.n == 9


def test_write_is_byte_exact():
    src = (DATA / "case9.m").read_text()
    case = powerio.parse(DATA / "case9.m")
    assert case.write() == src
    assert powerio.write(case) == src


def test_parse_bad_path_raises():
    # I/O failures map to the standard OSError subclass, not PowerIOError.
    with pytest.raises(FileNotFoundError):
        powerio.parse_matpower(str(DATA / "does_not_exist.m"))


def test_bad_parse_raises_powerio_error():
    with pytest.raises(powerio.PowerIOError):
        powerio.parse_matpower_string("this is not a matpower case")


def test_delegated_surface_resolves(case9):
    # Pin the attributes/methods that reach through __getattr__ to the compiled
    # handle, so a Rust-side getter rename can't silently desync the API.
    for attr in [
        "name",
        "base_mva",
        "source_format",
        "n",
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
        "gens",
        "reference_bus_index",
        "connectivity_report",
        "write",
        "write_dcopf_bundle",
    ]:
        assert hasattr(case9, attr), attr
    with pytest.raises(AttributeError):
        case9.does_not_exist


def test_import_and_parse_pull_in_no_numpy_or_scipy():
    # The zero-dep promise: parse/convert/write need nothing but the
    # interpreter. Run in a fresh process so another test importing scipy can't
    # pollute it, and parse + write a real case so the whole IO path is covered.
    code = (
        "import sys, powerio\n"
        f"c = powerio.parse_matpower(r'{DATA / 'case9.m'}')\n"
        "assert c.write()\n"
        "assert 'numpy' not in sys.modules, 'powerio dragged in numpy'\n"
        "assert 'scipy' not in sys.modules, 'powerio dragged in scipy'\n"
    )
    r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr


# --- matrix structure & values -----------------------------------------


@pytest.mark.parametrize("name", SMALL)
def test_bprime_is_singular_laplacian(name):
    c = load(name)
    b = c.bprime()
    assert sp.issparse(b) and b.format == "csr"
    assert b.shape == (c.n, c.n)
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
    # Exact cross-check across two boundary paths: B' in the XB scheme is the
    # paper-convention weighted Laplacian (b = 1/x). Catches a shared bug in
    # the COO conversion that the symmetric self-check can't.
    assert np.allclose(
        case9.bprime("xb").toarray(),
        case9.weighted_laplacian("paper").toarray(),
    )


def test_bdoubleprime_shunts_and_scheme():
    c = load("case30")  # has bus shunts
    bpp = c.bdoubleprime()
    assert bpp.shape == (c.n, c.n)
    # B'' keeps shunts, so it differs from the shuntless B'.
    assert not np.allclose(bpp.toarray(), c.bprime().toarray())
    # The scheme kwarg is wired: BX zeroes line resistance, XB does not.
    assert not np.allclose(c.bdoubleprime("bx").toarray(), c.bdoubleprime("xb").toarray())


@pytest.mark.parametrize("name", SMALL)
def test_ybus_complex_equals_parts(name):
    c = load(name)
    y = c.ybus()
    assert y.dtype == np.complex128 and y.shape == (c.n, c.n)
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
    assert block.shape == (2 * case9.n, 2 * case9.n)


@pytest.mark.parametrize("name", SMALL)
def test_sensitivities(name):
    c = load(name)
    ptdf, lodf = c.ptdf(), c.lodf()
    m, n = ptdf.shape
    assert n == c.n
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
    assert n == case9.n
    assert len(inc.b) == m and len(inc.p_shift) == n and len(inc.branch_of_col) == m
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
    rebuilt = inc.A @ sp.diags(inc.b) @ inc.A.T
    assert np.allclose(case9.weighted_laplacian().toarray(), rebuilt.toarray())


# --- string-kwarg parsing (aliases + errors) ---------------------------


def test_convention_aliases(case9):
    # Documented aliases all parse; separator/case-insensitive.
    for conv in ["paper", "paper-pure", "PURE", "matpower", "mp"]:
        assert sp.issparse(case9.ptdf(conv))
    for scheme in ["bx", "XB"]:
        assert sp.issparse(case9.bprime(scheme))


def test_bad_enum_strings_raise(case9, tmp_path):
    with pytest.raises(ValueError):
        case9.bprime(scheme="nonsense")
    with pytest.raises(ValueError):
        case9.ptdf(convention="nope")
    with pytest.raises(ValueError):
        case9.write_dcopf_bundle(str(tmp_path), units="bogus")


# --- graph view ---------------------------------------------------------


def test_to_networkx_attrs_and_status_filter():
    c = powerio.parse_matpower_string(TINY)
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
    assert powerio.parse_matpower_string(oos).to_networkx().number_of_edges() == 1


# --- connectivity & reference bus --------------------------------------


def test_connectivity_report(case9):
    rep = case9.connectivity_report()
    assert rep["n_buses"] == 9
    assert rep["n_components"] == 1
    assert rep["isolated_buses"] == []


def test_reference_bus_index(case9):
    assert case9.reference_bus_index() == 0


def test_reference_bus_error_on_two_refs():
    two_ref = TINY.replace("\t3\t2\t0", "\t3\t3\t0")  # bus 3: PV -> ref
    with pytest.raises(powerio.PowerIOError):
        powerio.parse_matpower_string(two_ref).reference_bus_index()


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
    assert a.shape[0] == case9.n
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
    case = powerio.parse_matpower_string(genless)
    assert case.n_gens == 0
    with pytest.raises(powerio.PowerIOError):
        case.write_dcopf_bundle(str(tmp_path))


# --- convert -----------------------------------------------------------


def test_convert_matpower_echo_is_byte_exact():
    src = (DATA / "case14.m").read_text()
    conv = powerio.convert(DATA / "case14.m", "matpower")
    assert conv.text == src
    assert conv.warnings == []


def test_convert_matpower_to_each_format():
    for fmt in ["powermodels-json", "egret-json", "psse", "powerworld"]:
        r = powerio.convert(str(DATA / "case30.m"), fmt)
        assert isinstance(r.text, str) and len(r.text) > 0
        assert isinstance(r.warnings, list)
    # PowerModels JSON output parses as JSON and keeps the bus count.
    pm = json.loads(powerio.convert(str(DATA / "case30.m"), "powermodels-json").text)
    assert len(pm["bus"]) == 30


def test_convert_round_trip_through_psse(tmp_path):
    raw = powerio.convert(str(DATA / "case30.m"), "psse").text
    p = tmp_path / "case30.raw"
    p.write_text(raw)
    back = powerio.convert(str(p), "matpower")  # PSS/E inferred from .raw extension
    case = powerio.parse_matpower_string(back.text)
    assert case.n == 30


def test_convert_unknown_format_raises():
    with pytest.raises(ValueError):
        powerio.convert(str(DATA / "case30.m"), "nonsense")


def test_missing_json_file_raises_oserror():
    # The non-MATPOWER read path must raise OSError too: a missing file is a
    # missing file, not a ValueError, regardless of the inferred format.
    with pytest.raises(OSError):
        powerio.convert(DATA / "definitely_missing.json", "matpower")


# --- large case integration --------------------------------------------


def test_large_case_pegase():
    path = DATA / "case2869pegase.m"
    if not path.is_file():
        pytest.skip("case2869pegase.m not vendored")
    c = powerio.parse_matpower(str(path))
    assert c.n == 2869
    b = c.bprime()
    assert b.shape == (2869, 2869)
    assert is_symmetric(b)


# --- gridfm Parquet surface --------------------------------------------

HAS_GRIDFM = bool(getattr(powerio._powerio, "_has_gridfm", False))
gridfm_only = pytest.mark.skipif(
    not HAS_GRIDFM, reason="extension built without the gridfm feature"
)


def test_gridfm_absent_raises_clean_importerror(case9, tmp_path):
    # On a default (light) build the write path is compiled out, so it must raise
    # a clear ImportError naming the extra — never an AttributeError.
    if HAS_GRIDFM:
        pytest.skip("extension built with gridfm; the absent-path is not exercised")
    with pytest.raises(ImportError, match="gridfm"):
        case9.write_gridfm(str(tmp_path))


@gridfm_only
def test_gridfm_write_single(case9, tmp_path):
    pd = pytest.importorskip("pandas")
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

    bus = pd.read_parquet(raw / "bus_data.parquet")
    assert len(bus) == case9.n
    assert (bus["scenario"] == 0).all()
    assert list(bus["bus"]) == list(range(case9.n))


@gridfm_only
def test_gridfm_batch_stacks_and_keys_by_scenario(tmp_path):
    pd = pytest.importorskip("pandas")
    # Same topology twice → two scenarios stacked in one dataset. (The Python
    # Case is read-only, so the two snapshots share values; the test pins the
    # row-stack and scenario keying, which the Rust tests pair with perturbation.)
    case = load("case9")
    out = powerio.write_gridfm_batch([case, case], str(tmp_path))
    raw = Path(out["dir"])

    bus = pd.read_parquet(raw / "bus_data.parquet")
    assert len(bus) == 2 * case.n
    assert list(bus["scenario"]) == [0] * case.n + [1] * case.n

    meta = json.loads((raw / "gridfm_meta.json").read_text())
    assert meta["n_scenarios"] == 2
    assert meta["scenario"] == 0


@gridfm_only
def test_gridfm_in_all_export():
    # The batch function is part of the package's public surface.
    assert "write_gridfm_batch" in powerio.__all__
    assert hasattr(powerio, "write_gridfm_batch")
