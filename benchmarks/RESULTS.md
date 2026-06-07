# Parser benchmark and cross-tool validation

Two things measured here, both on the vendored and large MATPOWER cases:

1. **Speed** — caseio against the parsers it competes with (ExaPowerIO.jl,
   PowerModels.jl, pandapower's reader), from small cases up to a 193k-bus, 56 MB
   file.
2. **Correctness** — caseio's parse, conversions, and Y_bus checked value for
   value against PowerModels.jl, ExaPowerIO.jl, and pandapower.

Numbers below are median time, one session, Apple M-series, release build. They
vary a few percent run to run; the relative picture is stable.

## Speed: parsers, head to head

All three parsers are timed in one Julia process under the same
`BenchmarkTools.@benchmark` harness (`benchmarks/bench_julia.jl`). caseio is called
through its C ABI (`cio_parse`, built with `cargo build --release -p caseio-capi`),
so it reads the file from disk and builds its case exactly as ExaPowerIO and
PowerModels do. The caseio case is freed in an untimed teardown, matching the other
two, whose returned data is collected after the sample rather than inside it.

| case | buses / branches | **caseio** | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | **1.78 ms** | 2.72 ms | 121 ms |
| case_ACTIVSg2000 | 2000 / 3206 | **2.07 ms** | 2.07 ms | 122 ms |
| case9241pegase | 9241 / 16049 | **5.67 ms** | 8.94 ms | 558 ms |
| case13659pegase | 13659 / 20467 | **8.57 ms** | 13.1 ms | 781 ms |
| case_ACTIVSg10k | 10000 / 12706 | **8.93 ms** | 9.09 ms | — |
| case_ACTIVSg25k | 25000 / 32230 | **22.0 ms** | 22.3 ms | — |
| case_ACTIVSg70k | 70000 / 88207 | **59.5 ms** | 64.5 ms | — |
| case_SyntheticUSA | 82000 / 104121 | **71.3 ms** | 76.6 ms | — |
| case99k | 99396 / 117860 | **80.7 ms** | 90.2 ms | — |
| case193k | 192768 / 228574 | **158 ms** | 169 ms | — |

(PowerModels skipped past case13659 — it takes minutes there and the gap is settled.)

### Read

- **vs PowerModels.jl**: 50–70× faster wherever PowerModels is practical to run
  (68× on case2869pegase, 98× on case9241pegase).
- **vs ExaPowerIO.jl**: caseio is faster or tied on every case — ~35% on the pegase
  cases (European, number-dense decimals, no cell arrays) and ~2–12% on the ACTIVSg
  / SyntheticUSA / US cases, where it does *more* work: those carry large `gentype`
  / `genfuel` / `bus_name` cell arrays that ExaPowerIO skips (it logs "Unrecognized
  assignment" and drops them), while caseio parses `bus_name` into the model and
  retains the full source for a byte-exact round-trip. The earlier read of these
  cases had caseio a few percent behind, which looked like the cost of losslessness.
  It wasn't: profiling found the gap was overhead unrelated to what caseio keeps — a
  `BTreeSet` reference-validation pass, a `split_ascii_whitespace` row tokenizer, a
  per-generator string-keyed map, and a materialized line index. Replacing those
  (HashSet, a byte tokenizer, a typed `[Option<f64>; 11]` for the gen capability
  columns, a streamed locate) cut parse time ~25–35% and put caseio ahead while it
  keeps strictly more of the file than ExaPowerIO does.
- **The pure parse is a touch faster than the table.** `cio_parse` reads the file
  from disk on every sample (as ExaPowerIO / PowerModels do); the Rust `timeparse`
  example parses an already-in-memory string and so excludes the per-sample read.
  The single source-retaining parse path is what makes the byte-exact round-trip
  free — an earlier design ran a second pass to record byte ranges (~38% of parse
  at case118, 51% at case2869); the current path drops it and the file reader moves
  its buffer straight into the retained source, so a parse never copies the whole
  file twice.
- **And the edge isn't only speed.** caseio is the only one of the three that is
  lossless and round-trips byte for byte — verified at 56 MB / 193k buses — and the
  only one callable from Rust, the CLI, Python, and C/Julia (the C ABI) with no
  runtime. ExaPowerIO has no writer; PowerModels' export is lossy.

### vs pandapower

pandapower reads MATPOWER `.m` through `matpowercaseframes` (a pandas reader) and
then `from_mpc` builds its `net` model. `benchmarks/bench_parse.py`, same machine:

| case | **caseio** parse | matpowercaseframes (pandapower's `.m` reader) |
| --- | --- | --- |
| case2869pegase | **2.5 ms** | 26.1 ms |
| case9241pegase | **8.6 ms** | 87.5 ms |
| case13659pegase | **13.1 ms** | 142.8 ms |
| case193k | **218 ms** | 2302 ms |

caseio's parse is ~10× faster than pandapower's reader, and that's before
`from_mpc` builds the `net` (case30: `from_mpc` ≈ 65 ms vs caseio < 1 ms;
`from_mpc` also raises `IndexError` on case118 and the pegase cases in pandapower
3.2.2, so it isn't a general MATPOWER path). The `caseio: parse` row uses the
zero-dependency `caseio` package; `casemat: parse + Y_bus + B'` (the scipy path)
runs about 2× the parse alone.

## Correctness: validated against all three

`bash benchmarks/run_validation.sh` runs every validator over every fixture and
prints a pass/fail matrix. Latest run — all checks pass:

| fixture | PMjson | PMread | PSS/E | ExaPowerIO | pandapower (+ Y_bus) |
| --- | --- | --- | --- | --- | --- |
| case9 / 14 / 30 / 57 / 118 | ✓ | ✓ | ✓ | ✓ | ✓ |
| t_case9_dcline | ✓ | ✓ | ✓ | ✓ | ✓ |
| t_case9_oos | ✓ | ✓ | ✓ | ✓ | ✓ |
| pglib case5_pjm / case14_ieee | ✓ | ✓ | ✓ | ✓ | ✓ |
| case2869pegase | ✓ | ✓ | ✓ | ✓ | ✓ |
| psse/case5, psse/case14 (read side) | — | — | ✓ | — | — |

What each column checks:

- **PMjson** (`validate_powermodels.jl`) — caseio's PowerModels JSON *writer* vs
  PowerModels.jl's own parse of the `.m`, field by field over bus / branch / gen /
  load / shunt. caseio emits idiomatic `per_unit=true` JSON (the form PowerModels
  itself writes), so this runs on PowerModels' default `validate=true` with no
  workarounds — including the dcline case, whose per-end bounds caseio now derives
  the way PowerModels does.
- **PMread** (`pm_export.jl` + `validate_powermodels.jl`) — caseio's PowerModels
  JSON *reader*: PowerModels exports the `.m` to JSON, caseio reads that and
  re-emits, and the two are compared. Exercises caseio reading real PowerModels
  output, not just its own.
- **PSS/E** (`validate_psse.jl`) — caseio's PSS/E `.raw` vs PowerModels.jl on the
  write side (`.m` → `.raw`), and caseio's PowerModels JSON of a real `.raw` on the
  read side; counts and demand/generation totals, switched shunts noted (not
  modeled).
- **ExaPowerIO** (`validate_exapowerio.jl`) — caseio (through the C ABI) vs
  ExaPowerIO's default `filtered=true` parse, value for value over bus / branch /
  gen. caseio's in-service rows are filtered to match ExaPowerIO's dropped
  out-of-service elements (see `t_case9_oos`). Encodings reconciled: per unit
  (×baseMVA), `b_fr + b_to` = total `b`, radians vs degrees, tap 0→1; bus types
  aren't compared (ExaPowerIO rewrites them from generator placement).
- **pandapower** (`validate_pandapower.py`) — caseio's parse and Y_bus vs
  pandapower's `_m2ppc` + `makeYbus` (PYPOWER's admittance kernel, the same one
  MATPOWER uses). Compares counts, per-branch r/x/b/tap/shift, per-bus demand and
  shunt, and the full Y_bus element for element (re-indexed to caseio's bus order;
  endpoints renumbered to dense positions so makeYbus handles the gappy pegase bus
  ids). `_m2ppc` is used instead of `from_mpc` because it runs before the
  `from_ppc` step that raises on dclines and parallel branches.

**Coverage gaps.** PowerWorld `.aux` and EGRET JSON have no external validator here:
there is no independent `.aux` reader to check against, EGRET has no caseio reader
and the `egret` package isn't installed. Both are covered by the in-tree all-pairs
round-trip tests (`caseio/tests/roundtrip_formats.rs`: core preservation,
reader∘writer idempotence, byte-exact same-format echo), not against a third-party
tool.

## Reproduce

```
bash benchmarks/fetch_cases.sh                 # large cases into gitignored tests/data/large
cargo build --release -p caseio-capi           # the C ABI the Julia harness calls
maturin develop --release                      # casemat into the active venv
maturin develop --release -m caseio-ext/Cargo.toml   # caseio into the active venv
julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'

julia --project=benchmarks benchmarks/bench_julia.jl       # parser speed table
python benchmarks/bench_parse.py tests/data/case2869pegase.m   # Python / pandapower speed
bash benchmarks/run_validation.sh                          # correctness matrix
```

Julia pins (`benchmarks/Project.toml` / `Manifest.toml`): PowerModels 0.21.6,
ExaPowerIO 0.3.0, BenchmarkTools 1.8.0. Python: pandapower 3.2.2,
matpowercaseframes 2.1.0.
