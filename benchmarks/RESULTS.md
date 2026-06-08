# Parser benchmark and cross-tool validation

Two benchmark suites live in the repo and don't overlap. `powerio/benches/parse.rs`
(`cargo bench --bench parse`) is the in-process microbenchmark: it times the Rust
parser and writers against themselves, no other tool in the loop. This directory
is the cross-tool comparison: it times powerio against the other parsers and
checks its output value for value against theirs, calling each through its own
runtime (Julia, Python). Use the microbenchmark to catch a regression in our own
code; use this suite to compare against the field.

Two things are measured here, both on the vendored and large MATPOWER cases:

1. **Speed**: powerio against ExaPowerIO.jl, PowerModels.jl, and pandapower's
   reader, from small cases up to a 192768-bus, 54 MB file.
2. **Correctness**: powerio's parse, conversions, and Y_bus checked value for
   value against PowerModels.jl, ExaPowerIO.jl, and pandapower.

Numbers below are median wall time from one session on an Apple M-series laptop,
release build. They vary a few percent run to run; the relative picture is stable.

## Speed: parsers head to head

All three parsers run in one Julia process under the same
`BenchmarkTools.@benchmark` harness (`benchmarks/bench_julia.jl`). powerio is
called through its C ABI (`pio_parse`, built with `cargo build --release -p
powerio-capi`), so it reads the file from disk and builds its case the way
ExaPowerIO and PowerModels do. The powerio handle is freed in an untimed
teardown, matching the other two, whose returned data is collected after the
sample rather than inside it.

<!-- BENCH:speed-julia START -->
| case | buses / branches | powerio | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | 1.73 ms | 2.86 ms | 122.2 ms |
| case_ACTIVSg2000 | 2000 / 3206 | 2.07 ms | 2.11 ms | 127.8 ms |
| case9241pegase | 9241 / 16049 | 5.81 ms | 9.15 ms | 553.2 ms |
| case13659pegase | 13659 / 20467 | 8.6 ms | 13.76 ms | 822.2 ms |
| case_ACTIVSg10k | 10000 / 12706 | 9.22 ms | 9.35 ms | n/a |
| case_ACTIVSg25k | 25000 / 32230 | 22.58 ms | 22.75 ms | n/a |
| case_ACTIVSg70k | 70000 / 88207 | 60.95 ms | 62.75 ms | n/a |
| case_SyntheticUSA | 82000 / 104121 | 73.1 ms | 82.65 ms | n/a |
| case99k | 99396 / 117860 | 84.27 ms | 94.88 ms | n/a |
| case193k | 192768 / 228574 | 161.9 ms | 174.98 ms | n/a |
<!-- BENCH:speed-julia END -->

(PowerModels is skipped past case13659, where it takes minutes and the gap is
already settled.)

- **vs PowerModels.jl**: 62–96× faster on the cases PowerModels was run on (71×
  on case2869pegase, 96× on case13659pegase).
- **vs ExaPowerIO.jl**: faster on every case in this run, by ~36–40% on the
  pegase cases (European, number-dense decimals, no cell arrays), narrowing to
  near parity on the smaller ACTIVSg cases and up to ~12% on case_SyntheticUSA
  and case99k. On the latter group powerio does more work and is still ahead:
  those cases carry large `gentype` / `genfuel` / `bus_name` cell arrays that
  ExaPowerIO drops (it logs "Unrecognized assignment"), while powerio parses
  `bus_name` into the model and retains the full source for a byte-exact
  round trip.
- **Lossless and polyglot.** powerio is the only one of the three that
  round trips byte for byte (verified at 54 MB / 192768 buses) and the only one
  callable from Rust, the CLI, Python, and C/Julia with no runtime. ExaPowerIO
  has no writer; PowerModels' export is lossy.

## vs pandapower

pandapower reads MATPOWER `.m` through `matpowercaseframes` (a pandas reader) and
then `from_mpc` builds its `net`. `benchmarks/bench_parse.py`, same machine,
median wall time:

<!-- BENCH:speed-pandapower START -->
| case | powerio parse | matpowercaseframes (pandapower's `.m` reader) |
| --- | --- | --- |
| case2869pegase | 1.8 ms | 25.6 ms |
| case9241pegase | 5.7 ms | 85.9 ms |
| case13659pegase | 8.9 ms | 139.9 ms |
| case193k | 165.4 ms | 2214.3 ms |
<!-- BENCH:speed-pandapower END -->

powerio's parse is ~14× faster than pandapower's `.m` reader, and that is before
`from_mpc` builds the `net` (case30: `from_mpc` ≈ 59 ms vs powerio under 1 ms).
`from_mpc` raises `IndexError` on case118 and the pegase cases in pandapower
3.2.2, so it isn't a general MATPOWER path. The `powerio: parse` row uses the
zero-dependency `powerio` package and reads from disk, so it matches the C ABI
column in the Julia table above to within run-to-run noise. The scipy matrix path
`powerio[matrix]: parse + Y_bus + B'` measured 9.2 / 27.2 / 34.6 / 533 ms on the
same four cases, roughly 3–5× the bare parse.

## Correctness: validated against four tools

`bash benchmarks/run_validation.sh` runs every validator over every fixture and
prints a pass/fail matrix. Latest run, all checks pass:

| fixture | PMjson | PMread | PSS/E | ExaPowerIO | pandapower (+ Y_bus) |
| --- | --- | --- | --- | --- | --- |
| case9 / 14 / 30 / 57 / 118 | ✓ | ✓ | ✓ | ✓ | ✓ |
| t_case9_dcline | ✓ | ✓ | ✓ | ✓ | ✓ |
| t_case9_oos | ✓ | ✓ | ✓ | ✓ | ✓ |
| pglib case5_pjm / case14_ieee | ✓ | ✓ | ✓ | ✓ | ✓ |
| case2869pegase | ✓ | ✓ | ✓ | ✓ | ✓ |
| psse/case5, psse/case14 (read side) | n/a | n/a | ✓ | n/a | n/a |
| egret/case9, case14, case30 (read side) | ✓ | n/a | n/a | n/a | n/a |

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
- **EGRET read side** (`validate_core.jl`): powerio reads a real EGRET `.json`
  (egret's own serializer output) and re-emits PowerModels JSON, checked against the
  matching MATPOWER case. The EGRET *writer* is checked separately by the egret
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
a powerio `.aux` → JSON bridge; the `egret` package for EGRET). The diagonal is
byte-exact. Sources are the real native files where they exist (PSS/E `.raw`, EGRET
`.json`) and representative MATPOWER cases otherwise. All 65 cells pass (13 source
cases × 5 targets):

```
source        ->MAT  ->PM  ->PSS/E  ->PWLD  ->EGRET
MATPOWER       ok    ok    ok      ok      ok     (case9/14/30/118, t_case9_dcline,
PSS/E (.raw)   ok    ok    ok      ok      ok      pglib_case5_pjm, case2869pegase)
EGRET (.json)  ok    ok    ok      ok      ok     (case9/14/30, dcline3)
```

This closes the previous gap: PowerWorld and EGRET now have validation coverage
(PowerWorld via the read-back bridge, EGRET against the `egret` package), on top of
the in-tree all-pairs round trip tests in `powerio/tests/roundtrip_formats.rs`
(core preservation, reader∘writer idempotence, byte-exact same-format echo). See
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

The oracle tools are benchmark-scoped: PowerModels.jl and ExaPowerIO.jl in
`benchmarks/Project.toml`, pandapower and egret in `benchmarks/requirements.txt`.
None is a dependency of the powerio package or wheel.

Versions for the run above: Julia 1.12.6 with PowerModels 0.21.6, ExaPowerIO
0.3.0, BenchmarkTools 1.8.0 (`benchmarks/Project.toml` / `Manifest.toml`); Python
with pandapower 3.2.2, gridx-egret 0.6.2, scipy 1.13.1, numpy 2.0.2.
