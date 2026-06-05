"""Tests for the netmat Python bindings.

Run with `pytest python/tests` after `maturin develop`.
"""

from pathlib import Path

import numpy as np
import pytest
import scipy.sparse as sp

import netmat as nm

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
SMALL = ["case9", "case30"]


def load(name):
    return nm.parse_matpower(str(DATA / f"{name}.m"))


def is_symmetric(m, tol=1e-9):
    return (abs(m - m.T) > tol).nnz == 0


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
    assert case9.n_connected_components == 1


def test_case_tables(case9):
    assert len(case9.buses) == 9
    assert len(case9.branches) == 9
    assert len(case9.gens) == 3
    bus = case9.buses[0]
    assert bus["id"] == 1 and bus["type"] == "REF"
    gen = case9.gens[0]
    assert gen["cost"]["model"] == 2
    assert gen["cost"]["coeffs"] == [0.11, 5.0, 150.0]


def test_parse_string_roundtrip(case9):
    text = (DATA / "case9.m").read_text()
    c = nm.parse_matpower_string(text, name="from_string")
    assert c.name == "from_string"
    assert c.n == case9.n
    assert np.allclose(c.bprime().toarray(), case9.bprime().toarray())


def test_parse_bad_path_raises():
    with pytest.raises(nm.NetmatError):
        nm.parse_matpower(str(DATA / "does_not_exist.m"))


# --- matrix structure ---------------------------------------------------


@pytest.mark.parametrize("name", SMALL)
def test_bprime_is_singular_laplacian(name):
    c = load(name)
    b = c.bprime()
    assert sp.issparse(b) and b.format == "csr"
    assert b.shape == (c.n, c.n)
    assert is_symmetric(b)
    # Shuntless Laplacian: rows sum to zero, positive diagonal, M-matrix sign.
    row_sums = np.asarray(b.sum(axis=1)).ravel()
    assert np.allclose(row_sums, 0.0, atol=1e-8)
    diag = b.diagonal()
    assert np.all(diag > 0)
    off = b - sp.diags(diag)
    assert off.max() <= 1e-12


@pytest.mark.parametrize("name", SMALL)
def test_ybus_complex_equals_parts(name):
    c = load(name)
    y = c.ybus()
    assert y.dtype == np.complex128 and y.shape == (c.n, c.n)
    g, b = c.ybus_parts()
    assert np.allclose(y.toarray(), (g + 1j * b).toarray())


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
def test_sensitivities_shapes(name):
    c = load(name)
    ptdf, lodf = c.ptdf(), c.lodf()
    m, n = ptdf.shape
    assert n == c.n
    assert lodf.shape == (m, m)
    # LODF diagonal is -1 on the monitored = outaged branch.
    assert np.allclose(lodf.diagonal(), -1.0)


def test_incidence_and_weighted_laplacian(case9):
    inc = case9.incidence()
    n, m = inc.A.shape
    assert n == case9.n
    assert len(inc.b) == m
    assert len(inc.p_shift) == n
    assert len(inc.branch_of_col) == m
    rebuilt = inc.A @ sp.diags(inc.b) @ inc.A.T
    assert np.allclose(case9.weighted_laplacian().toarray(), rebuilt.toarray())


def test_convention_kwarg(case9):
    paper = case9.ptdf("paper")
    matpower = case9.ptdf(convention="matpower")
    assert paper.shape == matpower.shape


def test_bad_scheme_raises(case9):
    with pytest.raises(ValueError):
        case9.bprime(scheme="nonsense")


# --- graph view ---------------------------------------------------------


def test_to_networkx(case9):
    g = case9.to_networkx()
    assert g.number_of_nodes() == 9
    assert g.number_of_edges() == 9
    # Edges carry the branch index and series reactance.
    _, _, data = next(iter(g.edges(data=True)))
    assert "branch" in data and "x" in data


# --- connectivity & reference bus --------------------------------------


def test_connectivity_report(case9):
    rep = case9.connectivity_report()
    assert rep["n_buses"] == 9
    assert rep["n_components"] == 1
    assert rep["isolated_buses"] == []


def test_reference_bus_index(case9):
    assert case9.reference_bus_index() == 0


# --- DC-OPF bundle ------------------------------------------------------


def test_write_dcopf_bundle(case9, tmp_path):
    out = case9.write_dcopf_bundle(str(tmp_path))
    files = out["files"]
    assert files, "bundle wrote no files"
    assert Path(out["dir"]).is_dir()
    for f in files:
        assert Path(f).is_file()
    names = {Path(f).name for f in files}
    assert {"A.mtx", "L.mtx", "q.mtx", "pd.mtx"} <= names


# --- large case integration --------------------------------------------


def test_large_case_pegase():
    path = DATA / "case2869pegase.m"
    if not path.is_file():
        pytest.skip("case2869pegase.m not vendored")
    c = nm.parse_matpower(str(path))
    assert c.n == 2869
    b = c.bprime()
    assert b.shape == (2869, 2869)
    assert is_symmetric(b)
