# Benchmark and Validation

Criterion benchmark suites live in `powerio/benches/parse.rs` and
`powerio-matrix/benches/matrix.rs`. The first times parser and writer paths; the
second times derived matrix construction from already parsed and indexed cases.
This directory contains extractors, comparison harnesses, and validation
harnesses that call each tool through its own runtime.

The top-level questions are:

1. **Speed**: parser wall time for powerio, ExaPowerIO.jl, PowerModels.jl, and
   pandapower's reader, PowerWorld aux/pwb reader timing, and matrix builder
   timing from small cases up to a 192768 bus, 54 MB file.
2. **Correctness**: powerio parse output, conversions, and Y_bus checked against
   PowerModels.jl, ExaPowerIO.jl, egret, and pandapower.

Numbers below come from one local snapshot, release build. Parser comparison
tables use median wall time; Criterion tables use mean estimates. Re-run the
scripts below before using the numbers in a paper, release note, or package
page.

Snapshot environment: MacBook Pro `Mac17,8`, Apple M5 Pro, 18 cores, 64 GB RAM,
macOS 26.4.1 (`25E253`), arm64. Rust `rustc 1.95.0`, `cargo 1.95.0`; Apple
clang 21.0.0; Julia 1.12.6; Python 3.12.13 in `.venv`. The repository was a
local working tree at `23dd4f94` with the benchmark and documentation changes
shown here.

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
| case2869pegase | 2869 / 4582 | 1.82 ms | 2.91 ms | 126.7 ms |
| case_ACTIVSg2000 | 2000 / 3206 | 2.36 ms | 2.26 ms | 126.9 ms |
| case9241pegase | 9241 / 16049 | 6.0 ms | 9.39 ms | 572.0 ms |
| case13659pegase | 13659 / 20467 | 9.3 ms | 13.83 ms | 811.1 ms |
| case_ACTIVSg10k | 10000 / 12706 | 9.91 ms | 9.58 ms | n/a |
| case_ACTIVSg25k | 25000 / 32230 | 24.55 ms | 23.48 ms | n/a |
| case_ACTIVSg70k | 70000 / 88207 | 66.56 ms | 62.51 ms | n/a |
| case_SyntheticUSA | 82000 / 104121 | 80.39 ms | 80.87 ms | n/a |
| case99k | 99396 / 117860 | 92.63 ms | 97.94 ms | n/a |
| case193k | 192768 / 228574 | 177.16 ms | 267.54 ms | n/a |
<!-- BENCH:speed-julia END -->

PowerModels is skipped past case13659 because those runs take minutes on this
machine. The comparison is a benchmark record, not a feature gate. Validation
below is the correctness gate.

## vs pandapower

pandapower reads MATPOWER `.m` through `matpowercaseframes` (a pandas reader) and
then `from_mpc` builds its `net`. `benchmarks/bench_parse.py`, same machine,
median wall time:

<!-- BENCH:speed-pandapower START -->
| case | powerio parse | powerio parse + Y_bus + Bp | matpowercaseframes (pandapower's `.m` reader) |
| --- | --- | --- | --- |
| case2869pegase | 1.9 ms | 7.2 ms | n/a |
| case9241pegase | 6.2 ms | 25.1 ms | n/a |
| case13659pegase | 9.7 ms | 34.2 ms | 114.4 ms |
| case193k | 176.5 ms | 526.6 ms | 1789.3 ms |
<!-- BENCH:speed-pandapower END -->

`from_mpc` raises `IndexError` on case118 and the pegase cases in pandapower
3.2.2, so the table reports `matpowercaseframes` as the reader path where that
reader works. With current `matpowercaseframes` 1.1.6, case2869pegase and
case9241pegase raise `OverflowError` on `Inf` limits, so those baselines are
recorded as n/a. The `powerio: parse` row uses the base Python package and reads
from disk. The matrix column includes parsing plus building the SciPy Y_bus and
Bp matrices.

## PowerWorld aux and pwb

`cargo bench --bench parse -- "parse_aux_|parse_pwb_"` times both PowerWorld
readers on the same cases: the vendored 200 bus pair, the fetched 2000 bus
pair and RTS-GMLC (`benchmarks/fetch_powerworld.sh`), and any file passed
through `POWERIO_BENCH_AUX`/`POWERIO_BENCH_PWB` for cases that cannot be
fetched. Criterion mean wall time, same machine as above.

<!-- BENCH:powerworld START -->
| case | buses / branches | aux | pwb |
| --- | --- | --- | --- |
| ACTIVSg200 | 200 / 246 | 2.84 ms | 0.43 ms |
| ACTIVSg2000 June 2016 | 2007 / 3043 | 32.95 ms | 3.48 ms |
| RTS-GMLC | 73 / 120 | n/a | 0.44 ms |
| Texas7k (local TAMU copy) | 6717 / 9140 | 77.64 ms | 17.12 ms |
<!-- BENCH:powerworld END -->

The `.pwb` reader locates each table by a depth first search over count
word candidates and validates every record behind every candidate (the
binary carries no field dictionary). The search machinery keeps that
affordable: probe rejections are static strings instead of formatted
allocations, bus membership is a bitmap instead of a hash set, and record
runs are cached by first record offset so candidates that point at the
same records share one walk. With those three changes (#99) the binary
reader beats the aux text reader on every sibling pair; before them the
same search took 43 ms / 424 ms / 907 ms on the first three files. RTS-GMLC
stays the dearest decode per bus because its bus numbers (101 through 325)
are small integers that appear constantly in binary data, forging candidate
device records the search walks and rejects once each.

The Texas7k decode (the 2021 era record layouts) initially repriced the
search by 10 to 45 percent on the 425 era files, and bisection showed the
widened branch flag vocabulary was almost none of it (about 4 us on the
200 bus case): the cost was an inlining loss (the whole record probes
became opaque fn pointer calls, so the early rejections stopped inlining
into the resync scans) and the second generator layout's candidate scan
running unconditionally whenever a forged load candidate failed the
chain. Both are structural fixes now: the probes are generic and
monomorphize, and the header constant keys which generator layouts the
search admits (425 files never carry the regulated bus shape, 483
through 551 files never carry the older one, 508 saves exist with both
and try the regulated shape first), which also keeps a layout the file
cannot carry from outbidding the right one. With those, plus probe
orderings that run the most selective checks first (the bus flag mask
before the name text scan, the generator block ranges as the values
read), the 425 era files parse below the pre widening numbers and the
2021 Texas7k row is the local large case published in the table. A
branch flag mask keyed to the detected generator layout was also tried and rejected: it turns real records
invisible to the table end check on the newer files, and a forged short
table can win (see known_branch_flags in the reader). Every structural
validation is unchanged; the reader stays correctness first (a wrong
network is worse than a slow loud parse).

## Matrix builders

`cargo bench -p powerio-matrix --bench matrix` times sparse matrix, DC OPF
component, and dense sensitivity builders. Each fixture is parsed once and
wrapped in `IndexedNetwork` before the timed loop, so the rows below do not
include parser or indexing time. The pipeline row additionally includes writing
the requested MTX files, the shunt sidecar, and metadata. Criterion mean wall
time, same machine as above.

<!-- BENCH:matrix START -->
| operation | case | buses / branches | mean |
| --- | --- | --- | --- |
| Bp sparse | case118 | 118 / 186 | 0.02 ms |
| Bpp sparse | case118 | 118 / 186 | 0.021 ms |
| Y_bus sparse | case118 | 118 / 186 | 0.035 ms |
| LACPF block | case118 | 118 / 186 | 0.065 ms |
| adjacency | case118 | 118 / 186 | 0.018 ms |
| Bp sparse | case2869pegase | 2869 / 4582 | 0.57 ms |
| Bpp sparse | case2869pegase | 2869 / 4582 | 0.601 ms |
| Y_bus sparse | case2869pegase | 2869 / 4582 | 1.057 ms |
| LACPF block | case2869pegase | 2869 / 4582 | 2.007 ms |
| adjacency | case2869pegase | 2869 / 4582 | 0.48 ms |
| DC OPF incidence | case118 | 118 / 186 | 0.014 ms |
| DC OPF weighted Laplacian | case118 | 118 / 186 | 0.012 ms |
| DC OPF grounded Laplacian | case118 | 118 / 186 | 0.037 ms |
| DC OPF flow map | case118 | 118 / 186 | 0.009 ms |
| DC OPF instance | case118 | 118 / 186 | 0.003 ms |
| PTDF + LODF | case118 | 118 / 186 | 2.927 ms |
| pipeline Y_bus pair | case2869pegase | 2869 / 4582 | 3.01 ms |
<!-- BENCH:matrix END -->

Refresh those rows with:

```
cargo bench -p powerio-matrix --bench matrix
python3 benchmarks/extract_matrix_bench.py
python3 benchmarks/render_tables.py
```

## Correctness: validation suite

`bash benchmarks/run_validation.sh` runs every validator over every fixture and
prints a pass/fail matrix. The latest local run passed:

| fixture | PMjson | PMread | PSS/E | ExaPowerIO | pandapower Y_bus | pandapower JSON | PyPSA CSV |
| --- | --- | --- | --- | --- | --- | --- | --- |
| case9 / 14 / 30 / 57 / 118 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| t_case9_dcline | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| t_case9_oos | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| pglib case5_pjm / case14_ieee | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| case2869pegase | ✓ | ✓ | ✓ | ✓ | n/a† | ✓ | ✓ |
| psse/case5, psse/case14 (read side) | n/a | n/a | ✓ | n/a | n/a | n/a | n/a |
| egret/case9, case14, case30 (read side) | ✓ | n/a | n/a | n/a | n/a | n/a | n/a |

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
  ids). powerio's first class loads and shunts are summed back onto their bus for
  the per-bus comparison. `_m2ppc` is used instead of `from_mpc` because it runs
  before the `from_ppc` step that raises on dclines and parallel branches.
- **pp-json** (`validate_pandapower_converter.py`): powerio's pandapower
  `pandapowerNet` JSON writer: pandapower imports the output, and counts plus
  the full Y_bus are compared against powerio's matrix builder.
- **pypsa** (`validate_pypsa.py`): powerio's PyPSA CSV folder writer: PyPSA
  imports the output, and counts, load/generation totals, and line and
  transformer parameters (converted back to powerio's per unit basis) are
  compared; a line/transformer split mismatch fails the case.

### Full reader × writer matrix

`benchmarks/validate_matrix.py` converts each source to every legacy text target and checks the
output's electrical core against the source's own core, read by an independent
oracle (PowerModels.jl for MATPOWER / PowerModels JSON / PSS/E, and PowerWorld via
a powerio `.aux` → JSON bridge; the `egret` package for egret). The diagonal
returns the original source text. Sources are the real native files where they exist (PSS/E `.raw`, egret
`.json`) and representative MATPOWER cases otherwise. All 65 legacy cells pass (13 source
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
[the format fidelity guide](https://eigenergy.github.io/powerio/guide/format-fidelity.html)
for the conventions and limits.

## Reproduce

```
bash benchmarks/fetch_cases.sh                 # large cases into gitignored tests/data/large
cargo build --release -p powerio-capi           # the C ABI the Julia harness calls
python3.12 -m venv .venv                        # Python oracle venv
.venv/bin/python -m pip install --upgrade pip maturin -r benchmarks/requirements.txt
env VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'

julia --project=benchmarks benchmarks/bench_julia.jl --json # parser speed table
.venv/bin/python benchmarks/bench_parse.py --json \
  tests/data/case2869pegase.m \
  tests/data/large/case9241pegase.m \
  tests/data/large/case13659pegase.m \
  tests/data/large/case193k.m
POWERIO_BENCH_AUX=<Texas7k_20210804.AUX> \
POWERIO_BENCH_PWB=<Texas7k_20210804.PWB> \
  cargo bench -p powerio --bench parse                # parser, writer, PowerWorld, PWD
python3 benchmarks/extract_powerworld_bench.py
cargo bench -p powerio-matrix --bench matrix           # sparse matrix and DC OPF builders
python3 benchmarks/extract_matrix_bench.py
python3 benchmarks/render_tables.py
python3 benchmarks/render_tables.py --check
bash benchmarks/run_validation.sh                          # correctness matrix
```

The oracle tools are benchmark scoped: PowerModels.jl and ExaPowerIO.jl in
`benchmarks/Project.toml`, pandapower, PyPSA, and egret in `benchmarks/requirements.txt`.
None is a dependency of the powerio package or wheel.

Versions for the run above: Julia 1.12.6 with PowerModels 0.21.6, ExaPowerIO
0.3.0, BenchmarkTools 1.8.0 (`benchmarks/Project.toml` / `Manifest.toml`);
Python package stack `powerio 0.3.3`, pandapower 3.2.2, matpowercaseframes
1.1.6, gridx-egret 0.6.2, PyPSA 1.2.4, scipy 1.18.0, numpy 2.5.0, pandas
2.3.3, networkx 3.6.1.

## Rich data model validation

`bash benchmarks/run_rich_validation.sh` is the validation tier for fields that
do not fit the MATPOWER row shape: branch terminal admittance, switches, branch
current ratings and flow solution values, storage current ratings, HVDC costs,
and load voltage models.

The strict part runs committed fixtures:

```
cargo test -p powerio rich
cargo test -p powerio-dist rich
cargo test -p powerio-matrix ybus_uses_asymmetric_terminal_admittance
```

It also runs a PowerModels JSON oracle leg when Julia is available:

```
julia --project=benchmarks benchmarks/validate_oracles.jl rich <tmp> <rich-json>...
```

That leg asks PowerModels.jl to parse rich PowerModels JSON and compares the
fields PowerModels exposes in its internal data dict: multiple loads on a bus,
`g_fr`/`b_fr`/`g_to`/`b_to`, `c_rating_*`, branch `pf/qf/pt/qt`, switch state and
ratings, storage `current_rating`, and dcline cost.

The broad corpus part is opt in and report only. It never commits local paths or
external data. Point it at any local case corpus with repeated `--root` flags:

```
bash benchmarks/run_rich_validation.sh --root <local-corpus> --root <another-local-corpus>
```

The scanner also accepts `POWERIO_RICH_ROOTS` as a path list separated by the
platform path separator. It treats every root the same way; package test data,
local archives, and generated cases are just corpus roots. Reports are written to
`benchmarks/results/rich_corpus.tsv`, `benchmarks/results/rich_corpus.json`,
`benchmarks/results/rich_oracle.tsv`, and `benchmarks/results/rich_dist_local.tsv`;
that directory is gitignored.

Distribution local DSS corpus checks stay opt in through
`POWERIO_DIST_LOCAL_DSS_CORPUS`. Failures from broad local corpora are triage
data; the committed rich tests and the curated PowerModels rich oracle are the
release gate.
