"""Tests for the `caseio` Python bindings — the dependency-light package.

Run with `pytest python/tests` after building the caseio wheel, e.g.
`maturin develop -m caseio-ext/pyproject.toml`.
"""

import subprocess
import sys
from pathlib import Path

import pytest

import caseio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"


def test_parse_basic():
    case = caseio.parse(DATA / "case9.m")
    assert case.n == 9
    assert case.n_branches == 9
    assert case.base_mva == 100.0
    assert len(case.buses) == 9
    assert case.n_gens == 3
    assert not case.is_radial  # case9 is meshed
    assert case.n_connected_components == 1


def test_loads_and_shunts_are_first_class():
    case = caseio.parse(DATA / "case30.m")
    # MATPOWER folds demand onto the bus row; caseio splits it back out.
    assert case.n_loads > 0
    assert all({"bus", "p", "q", "in_service"} <= set(l) for l in case.loads)
    # buses carry no pd/qd (that's what loads are for)
    assert "pd" not in case.buses[0]


def test_parse_string():
    src = (DATA / "case9.m").read_text()
    case = caseio.parse_string(src, name="c9")
    assert case.name == "c9"
    assert case.n == 9


def test_roundtrip_byte_exact():
    src = (DATA / "case9.m").read_text()
    case = caseio.parse(DATA / "case9.m")
    assert case.write() == src


def test_convert_matpower_echo_is_byte_exact():
    src = (DATA / "case14.m").read_text()
    conv = caseio.convert(DATA / "case14.m", "matpower")
    assert conv.text == src
    assert conv.warnings == []


def test_convert_to_psse_produces_output():
    conv = caseio.convert(DATA / "case30.m", "psse")
    assert conv.text.strip()
    assert isinstance(conv.warnings, list)


def test_connectivity_report():
    case = caseio.parse(DATA / "case14.m")
    rep = case.connectivity_report()
    assert rep["n_buses"] == 14
    assert rep["n_components"] == 1


def test_import_pulls_in_no_numpy_or_scipy():
    # The whole point of the caseio package: zero scientific-stack deps. Run in a
    # fresh interpreter so another test having imported casemat can't pollute it.
    code = (
        "import sys, caseio\n"
        "assert 'numpy' not in sys.modules, 'caseio dragged in numpy'\n"
        "assert 'scipy' not in sys.modules, 'caseio dragged in scipy'\n"
    )
    r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr


def test_bad_format_name_raises_value_error():
    with pytest.raises(ValueError):
        caseio.convert(DATA / "case9.m", "bogus")


def test_bad_parse_raises_caseio_error():
    with pytest.raises(caseio.CaseioError):
        caseio.parse_string("this is not a matpower case")


def test_missing_matpower_file_raises_oserror():
    with pytest.raises(OSError):
        caseio.parse(DATA / "definitely_missing_case.m")


def test_missing_json_file_raises_oserror():
    # The non-MATPOWER read path must raise OSError too: a missing file is a
    # missing file, not a ValueError, regardless of the inferred format.
    with pytest.raises(OSError):
        caseio.convert(DATA / "definitely_missing.json", "matpower")


def test_gens_carry_cost_dict():
    case = caseio.parse(DATA / "case30.m")
    costed = [g for g in case.gens if g.get("cost")]
    assert costed, "case30 generators carry cost curves"
    cost = costed[0]["cost"]
    assert isinstance(cost["coeffs"], list) and cost["coeffs"]
    assert "model" in cost
