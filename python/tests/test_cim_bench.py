"""Public capabilities used by external CGMES benchmark integrations."""
from pathlib import Path

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"


def test_component_counts_exclude_transformers():
    network = powerio.parse(DATA / "case14.m").value
    counts = network.component_counts()
    assert counts == {"lines": 17, "generators": 5, "loads": 11, "substations": 0}
    assert network.n_branches == 20


def test_sever_source_forces_fresh_emission_and_keeps_original(tmp_path):
    original = powerio.parse(DATA / "case14.m")
    fresh = original.sever_source()
    replay = powerio.emit(original, "matpower")
    regenerated = powerio.emit(fresh, "matpower")
    assert replay.fidelity == "exact_same_format"
    assert regenerated.fidelity != "exact_same_format"
    assert replay.artifacts[0].data == (DATA / "case14.m").read_bytes()
    assert fresh.value.component_counts() == original.value.component_counts()
    assert fresh.type_name == original.type_name
    assert len(fresh.diagnostics) == len(original.diagnostics)
    # A CGMES directory source must also become actual writer output.
    powerio.emit(original, "cgmes", tmp_path / "cgmes")
    cgmes = powerio.parse(tmp_path / "cgmes", format="cgmes")
    assert powerio.emit(cgmes, "cgmes").fidelity == "exact_same_format"
    result = powerio.emit(cgmes.sever_source(), "cgmes")
    assert result.fidelity != "exact_same_format"
    assert len(result.artifacts) == 4
