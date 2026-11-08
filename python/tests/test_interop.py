"""Optional TransitionZero contract tests; no network calls or vendored models."""
from pathlib import Path

import pytest

pytest.importorskip("interop")
pytest.importorskip("pypsa")
import polars as pl
from interop.adapters.outbound.local_filesystem import LocalFilesystem
from interop.core.reporting import EventLog, ScopedRecorder
from powerio.interop import PowerIOSource, PowerIOSourceParams


def test_source_stages_transformers_and_shunts(tmp_path):
    log = EventLog()
    source = PowerIOSource(LocalFilesystem(), ScopedRecorder(log))
    case = Path(__file__).resolve().parents[2] / "tests/data/case14.m"
    state = source.load_into_state(PowerIOSourceParams(input_path=case), tmp_path)
    counts = {key: frame.select(pl.len()).collect().item() for key, frame in state.source_topology.items()}
    assert counts == {"buses": 14, "generators": 5, "lines": 17, "loads": 11, "shunt_impedances": 1, "transformers": 3}
