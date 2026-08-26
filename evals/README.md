# The evaluation workspace

Everything here evaluates powerio against external tools and against its own
performance record. Nothing in this tree is published; the crates workspace
excludes it (`exclude = ["fuzz", "evals"]` in the root manifest), and minimal
fixtures for ordinary tests stay beside those tests under `tests/data`.

- `validation/` is the cross tool correctness matrix: powerio's parse and
  write against PowerModels.jl, ExaPowerIO.jl, Egret, pandapower, PyPSA,
  OpenDSS, the BMOPF schema, and the matrix oracles. `run_validation.sh` is
  the same gate CI runs; `run_rich_validation.sh` walks external corpora.
- `performance/` is the timing record: `bench_parse.py` and `bench_julia.jl`
  measure parse and matrix construction, `asv/` tracks the airspeed velocity
  history, and `RESULTS.md` is the rendered table.
- `allocation/` is the allocation gate with its pinned fixture digests and
  baseline table.

## What every case records

A validation case is reproducible or it is noise. Each harness records, in
its script or beside its fixtures:

- the source software and revision the oracle values came from (each
  `validate_*` script pins its tool version through
  `validation/requirements.txt` or the Julia manifest);
- the calculation settings the comparison runs under (per unit bases, angle
  units, tolerance constants declared at the top of each script);
- the identity mapping between powerio element order and the oracle's;
- the numeric tolerances, asserted per quantity;
- the diagnostics the case is expected to produce, asserted as codes.

A new case that cannot state those five things does not enter the matrix.
