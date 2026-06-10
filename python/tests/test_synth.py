"""Tests for the synthetic case generators (``powerio.generate_case``)."""

import pytest

import powerio


def test_tree_defaults():
    case = powerio.generate_case()
    assert case.n_buses == 64
    assert case.n_branches == 63  # spanning tree
    assert case.is_radial
    assert case.n_connected_components == 1
    assert case.buses[0]["id"] == 1 and case.buses[0]["kind"] == "REF"
    # buses and branches only
    assert case.n_gens == 0 and case.n_loads == 0 and case.n_shunts == 0


def test_lattice_rounds_up_to_perfect_square():
    case = powerio.generate_case("lattice", n=10)
    assert case.n_buses == 16
    assert case.n_connected_components == 1


def test_pegase_like_is_meshed():
    case = powerio.generate_case("pegase-like", n=100)
    assert case.n_buses == 100
    assert case.n_branches > 99
    assert not case.is_radial


def test_same_seed_same_case():
    a = powerio.generate_case("tree", n=32, seed=7)
    b = powerio.generate_case("tree", n=32, seed=7)
    assert a.to_json() == b.to_json()
    c = powerio.generate_case("tree", n=32, seed=8)
    assert c.to_json() != a.to_json()


def test_generated_case_round_trips_through_json():
    case = powerio.generate_case("lattice", n=16)
    back = powerio.from_json(case.to_json())
    assert back.n_buses == case.n_buses
    assert back.n_branches == case.n_branches


def test_matrix_builders_work_on_generated_case():
    case = powerio.generate_case("pegase-like", n=64)
    assert case.bprime().shape == (64, 64)
    assert case.adjacency().nnz > 0


def test_n_is_clamped_to_minimum():
    assert powerio.generate_case("tree", n=0).n_buses == 2
    assert powerio.generate_case("pegase-like", n=1).n_buses == 2
    assert powerio.generate_case("lattice", n=0).n_buses == 4  # 2x2 grid


def test_unknown_topology_raises():
    with pytest.raises(ValueError):
        powerio.generate_case("torus")
