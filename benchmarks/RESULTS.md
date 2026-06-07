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
`BenchmarkTools.@benchmark` harness (`benchmarks/bench_julia.jl`). caseio is
called through its C ABI (`cio_parse`, built with
`cargo build --release -p caseio-capi`), so it reads the file from disk and builds
its case exactly as ExaPowerIO and PowerModels do — a like-for-like number rather
than the old hand-pasted one from a separate Rust binary.

| case | buses / branches | **caseio** | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | **2.51 ms** | 3.31 ms | 161 ms |
| case_ACTIVSg2000 | 2000 / 3206 | 2.71 ms | **2.26 ms** | 144 ms |
| case9241pegase | 9241 / 16049 | **9.21 ms** | 9.74 ms | 640 ms |
| case13659pegase | 13659 / 20467 | **14.2 ms** | 15.8 ms | 997 ms |
| case_ACTIVSg10k | 10000 / 12706 | 12.6 ms | **9.75 ms** | — |
| case_ACTIVSg25k | 25000 / 32230 | 29.9 ms | **24.6 ms** | — |
| case_ACTIVSg70k | 70000 / 88207 | 82.1 ms | **70.6 ms** | — |
| case_SyntheticUSA | 82000 / 104121 | 98.9 ms | **86.5 ms** | — |
| case99k | 99396 / 117860 | 115 ms | **96.5 ms** | — |
| case193k | 192768 / 228574 | **224 ms** | 320 ms | — |

(PowerModels skipped past case13659 — it takes minutes there and the gap is settled.)

### Read

- **vs PowerModels.jl**: 50–70× faster wherever PowerModels is practical to run
  (64× on case2869pegase, 70× on case13659pegase).
- **vs ExaPowerIO.jl**: a wash, and which way it falls tracks what's in the file.
  caseio leads on the pegase cases (European, number-dense) and on the 193k file
  (224 ms vs 320 ms). ExaPowerIO leads ~15–30% on the ACTIVSg / SyntheticUSA
  synthetic cases — those carry large `gentype` / `genfuel` / `bus_name` cell
  arrays, which ExaPowerIO skips (it logs "Unrecognized assignment" and drops
  them) while caseio retains the full source text for a byte-exact round-trip.
  caseio does strictly more work on those files and stays within a small constant
  of a reader that throws the extra sections away.
- **The pure parse is faster than the table shows.** `cio_parse` reads the file
  from disk on every sample (matching ExaPowerIO / PowerModels). The Rust
  `timeparse` example parses an already-in-memory string and so excludes the
  per-sample read: 205 ms on case193k vs the C ABI's 224 ms. Either way the
  source-retaining single parse path is what gives the round-trip for free — an
  earlier design ran a second pass to record byte ranges (~38% of parse at
  case118, 51% at case2869); the current path drops it.
- **The durable edge isn't raw speed.** caseio is the only one of the three that
  is lossless and round-trips byte for byte — verified at 56 MB / 193k buses — and
  the only one callable from Rust, the CLI, Python, and C/Julia (the C ABI) with
  no runtime. ExaPowerIO has no writer; PowerModels' export is lossy.

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

| fixture | PowerModels JSON | PSS/E | ExaPowerIO | pandapower (+ Y_bus) |
| --- | --- | --- | --- | --- |
| case9 / 14 / 30 / 57 / 118 | ✓ | ✓ | ✓ | ✓ |
| t_case9_dcline | skip¹ | ✓ | ✓ | ✓ |
| pglib case5_pjm / case14_ieee | ✓ | ✓ | ✓ | ✓ |
| case2869pegase | ✓ | ✓ | ✓ | ✓ |
| psse/case5, psse/case14 (read side) | — | ✓ | — | — |

What each column checks:

- **PowerModels JSON** (`validate_powermodels.jl`) — caseio's PowerModels JSON vs
  PowerModels.jl's own parse of the `.m`, field by field over bus / branch / gen /
  load / shunt after `make_per_unit!`. Parsed with `validate=false` so PowerModels'
  correction pass doesn't divide the JSON `null` that an unbounded `Inf` bound
  becomes; the nulls are restored before per-unit, and PowerModels' derived
  `transformer` flag isn't compared (caseio labels a few unity-tap pegase branches
  the other way — their tap/shift/r/x/b match, and the Y_bus check below confirms
  the electrical result).
- **PSS/E** (`validate_psse.jl`) — caseio's PSS/E `.raw` vs PowerModels.jl on the
  write side (`.m` → `.raw`), and caseio's PowerModels JSON of a real `.raw` on the
  read side; counts and demand/generation totals, with switched shunts noted (not
  modeled).
- **ExaPowerIO** (`validate_exapowerio.jl`) — caseio (through the C ABI) vs
  `ExaPowerIO.parse_matpower(; filtered=false)`, value for value over bus / branch
  / gen. Reconciles the encodings: ExaPowerIO returns per unit (×baseMVA), splits
  line charging into `b_fr + b_to` (= total `b`), stores `shift` / angle limits in
  radians (caseio degrees), and normalizes `tap` 0→1. Bus types aren't compared
  (ExaPowerIO rewrites them from generator placement).
- **pandapower** (`validate_pandapower.py`) — caseio's parse and Y_bus vs
  pandapower's `_m2ppc` + `makeYbus` (PYPOWER's admittance kernel, the same one
  MATPOWER uses). Compares counts, per-branch r/x/b/tap/shift, per-bus demand and
  shunt, and the full Y_bus element for element (re-indexed to caseio's bus order;
  endpoints renumbered to dense positions so makeYbus handles the gappy pegase bus
  ids). `_m2ppc` is used instead of `from_mpc` because it runs before the
  `from_ppc` step that raises on dclines and parallel branches.

¹ caseio writes dcline limits under MATPOWER names (`mp_pmax`) rather than
PowerModels' `pmaxf`, and its mixed-model gencost export for `t_case9_dcline`
doesn't round-trip through PowerModels yet, so the PowerModels JSON check is
skipped for dcline cases. The other three validators still cover them.

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
