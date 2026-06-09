# Parser Benchmark and Validation

Two benchmark suites live in the repo. `powerio/benches/parse.rs`
(`cargo bench --bench parse`) times the Rust parser and writers by themselves.
This directory contains comparison and validation harnesses that call each tool
through its own runtime.

Two things are measured here, both on the vendored and large MATPOWER cases:

1. **Speed**: parser wall time for powerio, ExaPowerIO.jl, PowerModels.jl, and
   pandapower's reader, from small cases up to a 192768 bus, 54 MB file.
2. **Correctness**: powerio parse output, conversions, and Y_bus checked against
   PowerModels.jl, ExaPowerIO.jl, egret, and pandapower.

Numbers below are median wall time from one session on an Apple M-series laptop,
release build. Re-run the scripts below before using the numbers in a paper,
release note, or package page.

## Speed

All three parsers run in one Julia process under the same
`BenchmarkTools.@benchmark` harness (`benchmarks/bench_julia.jl`). powerio is
called through its C ABI (`pio_parse_file`, built with `cargo build --release -p
powerio-capi`), so it reads the file from disk and builds its case the way
ExaPowerIO and PowerModels do. The powerio handle is freed in an untimed
teardown, matching the other two, whose returned data is collected after the
sample rather than inside it.

<!-- BENCH:speed-julia START -->
| case | buses / branches | powerio | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | 1.83 ms | 2.93 ms | 133.0 ms |
| case_ACTIVSg2000 | 2000 / 3206 | 2.21 ms | 2.31 ms | 134.4 ms |
| case9241pegase | 9241 / 16049 | 5.97 ms | 9.43 ms | 586.8 ms |
| case13659pegase | 13659 / 20467 | 8.98 ms | 13.84 ms | 847.7 ms |
| case_ACTIVSg10k | 10000 / 12706 | 9.57 ms | 9.79 ms | n/a |
| case_ACTIVSg25k | 25000 / 32230 | 23.52 ms | 23.6 ms | n/a |
| case_ACTIVSg70k | 70000 / 88207 | 63.82 ms | 62.65 ms | n/a |
| case_SyntheticUSA | 82000 / 104121 | 76.81 ms | 81.92 ms | n/a |
| case99k | 99396 / 117860 | 87.66 ms | 96.5 ms | n/a |
| case193k | 192768 / 228574 | 169.35 ms | 180.4 ms | n/a |
<!-- BENCH:speed-julia END -->

PowerModels is skipped past case13659 because those runs take minutes on this
machine. The comparison is a benchmark record, not a feature gate. Validation
below is the correctness gate.

## vs pandapower

pandapower reads MATPOWER `.m` through `matpowercaseframes` (a pandas reader) and
then `from_mpc` builds its `net`. `benchmarks/bench_parse.py`, same machine,
median wall time:

<!-- BENCH:speed-pandapower START -->
| case | powerio parse | matpowercaseframes (pandapower's `.m` reader) |
| --- | --- | --- |
| case2869pegase | 1.8 ms | n/a |
| case9241pegase | 5.9 ms | n/a |
| case13659pegase | 8.9 ms | 132.6 ms |
| case193k | 168.2 ms | 2387.7 ms |
<!-- BENCH:speed-pandapower END -->

`from_mpc` raises `IndexError` on case118 and the pegase cases in pandapower
3.2.2, so the table reports `matpowercaseframes` as the reader path where that
reader works. With current `matpowercaseframes` 1.1.6, case2869pegase and
case9241pegase raise `OverflowError` on `Inf` limits, so those baselines are
recorded as n/a. The `powerio: parse` row uses the base Python package and reads
from disk. The scipy matrix path `powerio[matrix]: parse + Y_bus + B'` measured
9.0 / 26.0 / 36.1 / 565.1 ms on the same four cases.

## Correctness: validated against four tools

`bash benchmarks/run_validation.sh` runs every validator over every fixture and
prints a pass/fail matrix. Latest run, all checks pass:

| fixture | PMjson | PMread | PSS/E | ExaPowerIO | pandapower (+ Y_bus) |
| --- | --- | --- | --- | --- | --- |
| case9 / 14 / 30 / 57 / 118 | ✓ | ✓ | ✓ | ✓ | ✓ |
| t_case9_dcline | ✓ | ✓ | ✓ | ✓ | ✓ |
| t_case9_oos | ✓ | ✓ | ✓ | ✓ | ✓ |
| pglib case5_pjm / case14_ieee | ✓ | ✓ | ✓ | ✓ | ✓ |
| case2869pegase | ✓ | ✓ | ✓ | ✓ | n/a† |
| psse/case5, psse/case14 (read side) | n/a | n/a | ✓ | n/a | n/a |
| egret/case9, case14, case30 (read side) | ✓ | n/a | n/a | n/a | n/a |

† pandapower's reader (matpowercaseframes) does `int(float(tok))` and raises on the
`Inf` limits MATPOWER uses for "unlimited", which case2869pegase carries; the pp
validator reports n/a there (powerio, PowerModels, and ExaPowerIO all read the case).

What each column checks:

- **PMjson** (`validate_powermodels.jl`): powerio's PowerModels JSON *writer* vs
  PowerModels.jl's own parse of the `.m`, field by field over bus / branch / gen /
  load / shunt. powerio emits idiomatic `per_unit=true` JSON (the form PowerModels
  itself writes), so this runs on PowerModels' default `validate=true` with no
  workarounds, including the dcline case, whose per-end bounds powerio derives the
  way PowerModels does.
- **PMread** (`pm_export.jl` + `validate_powermodels.jl`): powerio's PowerModels
  JSON *reader*: PowerModels exports the `.m` to JSON, powerio reads that and
  re-emits, and the two are compared. The PMjson check above covers only the writer.
- **PSS/E** (`validate_psse.jl`): powerio's PSS/E `.raw` vs PowerModels.jl on the
  write side (`.m` → `.raw`), and powerio's PowerModels JSON of a real `.raw` on the
  read side; counts and demand/generation/shunt totals. A switched shunt is read as
  a fixed shunt at `BINIT`, matching PowerModels, so case14's switched shunt is
  carried, not dropped.
- **egret read side** (`validate_core.jl`): powerio reads a real egret `.json`
  (egret's own serializer output) and re-emits PowerModels JSON, checked against the
  matching MATPOWER case. The egret writer is checked separately by the egret
  oracle in the matrix below.
- **ExaPowerIO** (`validate_exapowerio.jl`): powerio (through the C ABI) vs
  ExaPowerIO's default `filtered=true` parse, value for value over bus / branch /
  gen. powerio's in-service rows are filtered to match ExaPowerIO's dropped
  out-of-service elements (see `t_case9_oos`). Encodings reconciled: per unit
  (×baseMVA), `b_fr + b_to` = total `b`, radians vs degrees, tap 0→1; bus types
  aren't compared (ExaPowerIO rewrites them from generator placement).
- **pandapower** (`validate_pandapower.py`): powerio's parse and Y_bus vs
  pandapower's `_m2ppc` + `makeYbus` (PYPOWER's admittance kernel, the same one
  MATPOWER uses). Compares counts, per-branch r/x/b/tap/shift, per-bus demand and
  shunt, and the full Y_bus element for element (re-indexed to powerio's bus order;
  endpoints renumbered to dense positions so makeYbus handles the gappy pegase bus
  ids). powerio's first-class loads and shunts are summed back onto their bus for
  the per-bus comparison. `_m2ppc` is used instead of `from_mpc` because it runs
  before the `from_ppc` step that raises on dclines and parallel branches.

### Full reader × writer matrix

`benchmarks/validate_matrix.py` converts each source to every target and checks the
output's electrical core against the source's own core, read by an independent
oracle (PowerModels.jl for MATPOWER / PowerModels JSON / PSS/E, and PowerWorld via
a powerio `.aux` → JSON bridge; the `egret` package for egret). The diagonal
returns the original source text. Sources are the real native files where they exist (PSS/E `.raw`, egret
`.json`) and representative MATPOWER cases otherwise. All 65 cells pass (13 source
cases × 5 targets):

```
source        ->MAT  ->PM  ->PSS/E  ->PWLD  ->egret
MATPOWER       ok    ok    ok      ok      ok     (case9/14/30/118, t_case9_dcline,
PSS/E (.raw)   ok    ok    ok      ok      ok      pglib_case5_pjm, case2869pegase)
egret (.json)  ok    ok    ok      ok      ok     (case9/14/30, dcline3)
```

PowerWorld and egret have validation coverage here: PowerWorld through the
read-back bridge, egret against the `egret` package. The Rust suite also runs the
all-pairs tests in `powerio/tests/roundtrip_formats.rs`. See
[docs/format-fidelity.md](../docs/format-fidelity.md) for the conventions and limits.

## Reproduce

```
bash benchmarks/fetch_cases.sh                 # large cases into gitignored tests/data/large
cargo build --release -p powerio-capi           # the C ABI the Julia harness calls
maturin develop --release                       # the powerio wheel into the venv
pip install -r benchmarks/requirements.txt      # the pandapower + egret oracles
julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'

julia --project=benchmarks benchmarks/bench_julia.jl       # parser speed table
python benchmarks/bench_parse.py tests/data/case2869pegase.m   # Python / pandapower speed
bash benchmarks/run_validation.sh                          # correctness matrix
```

The oracle tools are benchmark scoped: PowerModels.jl and ExaPowerIO.jl in
`benchmarks/Project.toml`, pandapower and egret in `benchmarks/requirements.txt`.
None is a dependency of the powerio package or wheel.

Versions for the run above: Julia 1.12.6 with PowerModels 0.21.6, ExaPowerIO
0.3.0, BenchmarkTools 1.8.0 (`benchmarks/Project.toml` / `Manifest.toml`); Python
with pandapower 3.2.2, gridx-egret 0.6.2, scipy 1.13.1, numpy 2.0.2.
