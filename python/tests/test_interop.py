"""Optional TransitionZero contract tests; no network calls or vendored models."""
from pathlib import Path

import pytest

pytest.importorskip("interop")
pytest.importorskip("pypsa")
import polars as pl
from interop.adapters.outbound.local_filesystem import LocalFilesystem
from interop.core.reporting import EventLog, ScopedRecorder
from powerio.interop import (
    PowerIOSink,
    PowerIOSinkParams,
    PowerIOSource,
    PowerIOSourceParams,
)

import powerio as pio


def test_source_stages_transformers_and_shunts(tmp_path):
    log = EventLog()
    source = PowerIOSource(LocalFilesystem(), ScopedRecorder(log))
    case = Path(__file__).resolve().parents[2] / "tests/data/case14.m"
    state = source.load_into_state(PowerIOSourceParams(input_path=case), tmp_path)
    counts = {key: frame.select(pl.len()).collect().item() for key, frame in state.source_topology.items()}
    assert counts == {"buses": 14, "generators": 5, "lines": 17, "loads": 11, "shunt_impedances": 1, "transformers": 3}


def test_source_and_sink_preserve_electrical_tables(tmp_path):
    recorder = ScopedRecorder(EventLog())
    case = Path(__file__).resolve().parents[2] / "tests/data/case14.m"
    state = PowerIOSource(LocalFilesystem(), recorder).load_into_state(
        PowerIOSourceParams(input_path=case), tmp_path / "stage")
    state.destination_tables = {name: frame.collect() for name, frame in state.source_topology.items()}
    path = tmp_path / "roundtrip.pio.json"
    PowerIOSink(LocalFilesystem(), recorder).write(state, PowerIOSinkParams(output_path=path))
    result = pio.deserialize(path.read_bytes())
    assert len(result.value.buses) == 14
    assert len(result.value.branches) == 20
    assert len(result.value.generators) == 5
    assert len(result.value.shunts) == 1
    expected = pio.parse(case)
    assert sum(g["pg"] for g in result.value.generators) == pytest.approx(sum(g["pg"] for g in expected.value.generators))
    assert sum(load["p"] for load in result.value.loads) == pytest.approx(sum(load["p"] for load in expected.value.loads))


def test_entry_point_contracts():
    from importlib.metadata import entry_points
    for group, name, expected in [("interop.sources", "powerio_source", PowerIOSource), ("interop.sinks", "powerio_sink", PowerIOSink)]:
        entries = [entry for entry in entry_points(group=group) if entry.name == name]
        if not entries:
            pytest.skip("entry points require an installed PowerIO distribution")
        assert len(entries) == 1
        assert entries[0].load() is expected


def test_sienna_staging_runs_upstream_translation(tmp_path):
    from interop.plugins.steps.sienna_to_pypsa.map_components import (
        SiennaToPypsaMapComponents,
    )
    recorder = ScopedRecorder(EventLog())
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    expected = pio.parse(case)
    state = PowerIOSource(LocalFilesystem(), recorder).load_into_state(
        PowerIOSourceParams(input_path=case, tables="sienna"), tmp_path / "stage")
    state = SiennaToPypsaMapComponents(recorder).run(state, None)
    assert state.destination_tables["lines"].height == 9
    assert state.destination_tables["generators"].height == 3
    path = tmp_path / "roundtrip.pio.json"
    PowerIOSink(LocalFilesystem(), recorder).write(state, PowerIOSinkParams(output_path=path))
    actual = pio.deserialize(path.read_bytes()).value
    assert len(actual.branches) == 9
    assert sum(b["rate_a"] for b in actual.branches) == pytest.approx(sum(b["rate_a"] for b in expected.value.branches))
    assert sum(b["x"] for b in actual.branches) / actual.base_mva == pytest.approx(sum(b["x"] for b in expected.value.branches) / expected.value.base_mva)


def test_ir_keeps_pipeline_extensions_and_series(tmp_path):
    from datetime import datetime

    from interop.core.extensions import ExtensionKind, GeneratorExtension
    recorder = ScopedRecorder(EventLog())
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    source = PowerIOSource(LocalFilesystem(), recorder)
    state = source.load_into_state(PowerIOSourceParams(input_path=case), tmp_path / "stage")
    state.destination_tables = {name: frame.collect() for name, frame in state.source_topology.items()}
    name = state.destination_tables["generators"]["name"][0]
    state.destination_extensions[ExtensionKind.GENERATOR] = [GeneratorExtension(name=name, overnight_cost_per_mw=1234.0, marginal_cost_series="generators.parquet")]
    series = pl.DataFrame({"snapshot": [datetime(2030, 1, 1), datetime(2030, 1, 1, 1)], "name": [name, name], "marginal_cost": [12.5, 18.0]})
    state.destination_extension_series[ExtensionKind.GENERATOR] = series.lazy()
    path = tmp_path / "extended.pio.json"
    PowerIOSink(LocalFilesystem(), recorder).write(state, PowerIOSinkParams(output_path=path))
    recovered = source.load_into_state(PowerIOSourceParams(input_path=path, format="pio-ir"), tmp_path / "recovered")
    assert recovered.source_extensions[ExtensionKind.GENERATOR] == state.destination_extensions[ExtensionKind.GENERATOR]
    assert recovered.source_extension_series[ExtensionKind.GENERATOR].collect().equals(series)
