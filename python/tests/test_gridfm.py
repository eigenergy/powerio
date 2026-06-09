"""Tests for the gridfm Parquet export (`powerio gridfm`).

These drive the `powerio` CLI binary and read its Parquet with Polars, so they
need the `gridfm` extra (`pip install '.[gridfm]'`) and a built binary. Both are
optional: the module skips cleanly when either is missing.

The binary is found via `$POWERIO_BIN`, else `target/{release,debug}/powerio`
under the repo root. Build it with `cargo build -p powerio-cli` (the CLI always
compiles the `gridfm` feature).
"""

import os
import subprocess
from pathlib import Path

import pytest

pl = pytest.importorskip("polars")

ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "tests" / "data"

# The on-disk schema, byte-for-byte from gridfm-datakit's column_names.py. Same
# lists the Rust round-trip test asserts.
BUS_COLS = [
    "scenario", "load_scenario_idx", "bus", "Pd", "Qd", "Pg", "Qg", "Vm", "Va",
    "PQ", "PV", "REF", "vn_kv", "min_vm_pu", "max_vm_pu", "GS", "BS",
]
GEN_COLS = [
    "scenario", "load_scenario_idx", "idx", "bus", "p_mw", "q_mvar", "min_p_mw",
    "max_p_mw", "min_q_mvar", "max_q_mvar", "cp0_eur", "cp1_eur_per_mw",
    "cp2_eur_per_mw2", "in_service", "is_slack_gen",
]
BRANCH_COLS = [
    "scenario", "load_scenario_idx", "idx", "from_bus", "to_bus", "pf", "qf",
    "pt", "qt", "r", "x", "b", "Yff_r", "Yff_i", "Yft_r", "Yft_i", "Ytf_r",
    "Ytf_i", "Ytt_r", "Ytt_i", "tap", "shift", "ang_min", "ang_max", "rate_a",
    "br_status",
]
YBUS_COLS = ["scenario", "load_scenario_idx", "index1", "index2", "G", "B"]


def powerio_bin():
    env = os.environ.get("POWERIO_BIN")
    candidates = [Path(env)] if env else []
    candidates += [ROOT / "target" / p / "powerio" for p in ("release", "debug")]
    for c in candidates:
        if c.is_file():
            return c
    pytest.skip("powerio binary not found; run `cargo build -p powerio-cli`")


@pytest.fixture(scope="module")
def case14_raw(tmp_path_factory):
    out = tmp_path_factory.mktemp("gridfm")
    subprocess.run(
        [str(powerio_bin()), "gridfm", str(DATA / "case14.m"), "-o", str(out)],
        check=True,
    )
    return out / "case14" / "raw"


def test_writes_the_four_tables_and_manifest(case14_raw):
    for name in ("bus_data", "gen_data", "branch_data", "y_bus_data"):
        assert (case14_raw / f"{name}.parquet").is_file()
    assert (case14_raw / "gridfm_meta.json").is_file()


def test_schema_matches_datakit(case14_raw):
    cases = {
        "bus_data": (BUS_COLS, 14),
        "gen_data": (GEN_COLS, 5),
        "branch_data": (BRANCH_COLS, 20),
        "y_bus_data": (YBUS_COLS, None),
    }
    for name, (cols, rows) in cases.items():
        df = pl.read_parquet(case14_raw / f"{name}.parquet")
        assert list(df.columns) == cols, name
        if rows is not None:
            assert len(df) == rows, name


def test_basic_invariants(case14_raw):
    bus = pl.read_parquet(case14_raw / "bus_data.parquet")
    # PQ/PV/REF one-hot partitions every bus; exactly one reference.
    assert ((bus["PQ"] + bus["PV"] + bus["REF"]) == 1).all()
    assert int(bus["REF"].sum()) == 1
    # Dense, contiguous bus index.
    assert bus["bus"].to_list() == list(range(14))

    gen = pl.read_parquet(case14_raw / "gen_data.parquet")
    assert gen["is_slack_gen"].sum() >= 1


def test_satisfies_graphkit_feature_contract(case14_raw):
    """The columns gridfm-graphkit's HeteroGridDatasetDisk reads, and the
    gen->bus reactive-limit aggregation it does, must work on our output.

    Replicates graphkit's column selection (powergrid_hetero_dataset.py) without
    importing torch, so it proves the contract without the training stack.
    """
    bus = pl.read_parquet(case14_raw / "bus_data.parquet")
    gen = pl.read_parquet(case14_raw / "gen_data.parquet")
    branch = pl.read_parquet(case14_raw / "branch_data.parquet")

    # Columns graphkit reads straight off bus_data (it derives min/max_q_mvar
    # itself, below).
    bus_direct = ["Pd", "Qd", "Qg", "Vm", "Va", "PQ", "PV", "REF",
                  "min_vm_pu", "max_vm_pu", "GS", "BS", "vn_kv"]
    assert set(bus_direct) <= set(bus.columns)

    gen_feats = ["p_mw", "min_p_mw", "max_p_mw", "cp0_eur", "cp1_eur_per_mw",
                 "cp2_eur_per_mw2", "in_service"]
    assert set(gen_feats) <= set(gen.columns)

    edge_cols = ["from_bus", "to_bus", "pf", "qf", "pt", "qt",
                 "Yff_r", "Yff_i", "Yft_r", "Yft_i", "Ytt_r", "Ytt_i",
                 "Ytf_r", "Ytf_i", "tap", "ang_min", "ang_max", "rate_a",
                 "br_status"]
    assert set(edge_cols) <= set(branch.columns)

    # graphkit's bus-level reactive limits: sum gen min/max_q_mvar per (scenario,
    # bus). Must yield a finite value for every bus that has a generator.
    agg = gen.group_by(["scenario", "bus"]).agg(
        pl.col(["min_q_mvar", "max_q_mvar"]).sum()
    )
    assert len(agg) > 0
    total = (gen["min_q_mvar"] + gen["max_q_mvar"]).sum()
    all_zero = ((gen["min_q_mvar"] == 0) & (gen["max_q_mvar"] == 0)).all()
    assert total != 0 or all_zero


def test_graphkit_loads_dataset(case14_raw):
    """Optional end-to-end: if gridfm-graphkit (and torch) are installed, build
    the actual HeteroData graph from our output. Skips otherwise; runs only in an
    environment that has the full training stack."""
    pytest.importorskip("torch")
    mod = pytest.importorskip("gridfm_graphkit.datasets.powergrid_hetero_dataset")
    # The PyG Dataset roots at the network dir, whose `raw/` holds our parquet.
    ds = mod.HeteroGridDatasetDisk(root=str(case14_raw.parent))
    assert len(ds) >= 1
    assert ds[0]["bus"].x.shape[0] == 14
