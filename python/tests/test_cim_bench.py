"""Public capabilities used by external CGMES benchmark integrations."""
from pathlib import Path

import pytest

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"


def test_component_counts_exclude_transformers():
    network = powerio.parse(DATA / "case14.m").value
    counts = network.component_counts()
    assert counts == {"lines": 17, "generators": 5, "loads": 11, "substations": 0}
    assert network.n_branches == 20
    assert network.n_lines == 17
    assert network.n_substations == 0


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
    counts = cgmes.value.component_counts()
    assert cgmes.value.n_lines == counts["lines"]
    assert cgmes.value.n_substations == counts["substations"] > 0
    assert powerio.emit(cgmes, "cgmes").fidelity == "exact_same_format"
    result = powerio.emit(cgmes.sever_source(), "cgmes")
    assert result.fidelity != "exact_same_format"
    assert len(result.artifacts) == 4


def test_archive_expansion_uses_the_shared_acquisition_limit(monkeypatch, tmp_path):
    import zipfile

    module = powerio.parse(DATA / "case14.m")
    emitted = powerio.emit(module, "cgmes")
    expanded_bytes = sum(len(artifact.data) for artifact in emitted.artifacts)
    path = tmp_path / "profiles.zip"
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for artifact in emitted.artifacts:
            archive.writestr(artifact.name, artifact.data)
    assert path.stat().st_size < expanded_bytes
    monkeypatch.setenv("POWERIO_MAX_REFERENCED_BYTES", str(expanded_bytes))
    assert powerio.parse(path, format="cgmes").value.n_buses == 14
    monkeypatch.setenv("POWERIO_MAX_REFERENCED_BYTES", str(expanded_bytes - 1))
    with pytest.raises(powerio.PowerIOError, match="input limit"):
        powerio.parse(path, format="cgmes")


def test_component_counts_reject_normalized_equipment_classification():
    network = powerio.parse(DATA / "case14.m").value.to_normalized()
    with pytest.raises(ValueError, match="unnormalized network"):
        network.component_counts()


def test_native_line_count_rejects_normalized_equipment_classification():
    network = powerio.parse(DATA / "case14.m").value.to_normalized()
    with pytest.raises(ValueError, match="unnormalized network"):
        _ = network.n_lines
