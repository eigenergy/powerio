# Parser benchmark

Measured head-to-head: `caseio` (Rust) vs the two Julia parsers it competes
with. Median parse time, same machine (Apple M-series, release build), no
warmup counted. caseio via `cargo run --release -p caseio --example timeparse`;
the Julia numbers via `julia --project=benchmarks benchmarks/bench_julia.jl`
(`BenchmarkTools.@benchmark`, 40/10 samples). All three return identical
bus/branch counts — fast *and* correct.

| case | buses / branches | **caseio** (Rust) | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case118 | 118 / 186 | 0.20 ms | 0.19 ms | 5.4 ms |
| case2869pegase | 2869 / 4582 | **2.20 ms** | 2.81 ms | 133 ms |
| case_ACTIVSg2000 | 2000 / 3206 | 2.70 ms | 2.19 ms | 129 ms |
| case9241pegase | 9241 / 16049 | **6.74 ms** | 9.10 ms | 554 ms |
| case13659pegase | 13659 / 20467 | **10.2 ms** | 13.4 ms | 778 ms |

## Read

- **vs PowerModels.jl**: 25–80× faster across the board. PowerModels' parser
  carries the modeling/standardization machinery; if you only want the data,
  it's the slow path.
- **vs ExaPowerIO.jl** (the focused Julia reader, ~30–40× faster than
  PowerModels): caseio is **faster on the large pegase scaling cases** (2869,
  9241, 13659) by ~1.3×, a wash on small case118, and ~1.2× slower on
  ACTIVSg2000. So caseio is competitive with, and on the headline large cases
  faster than, the fastest existing parser — honestly mixed, not a blanket win.
- **The durable edge is not raw speed.** caseio is the only one of the three
  that is **lossless** and **round-trips byte-for-byte** (`parse → write →
  parse`, proven in `caseio/tests/roundtrip.rs`) — ExaPowerIO has no writer and
  PowerModels' export is lossy — and the only one callable from Rust, the CLI,
  and Python with no runtime.

The ACTIVSg case (more generators, `gentype`/`genfuel`/`bus_name` cell arrays)
points at the remaining lever: caseio's per-section comment-strip and the
document's owned copies are allocation-bound. A zero-copy pass (parse from byte
ranges into the typed structs) is the path to winning ExaPowerIO outright on
every case; the numbers above are the pre-optimization baseline.

## Reproduce

```
# caseio (Rust)
cargo run --release -p caseio --example timeparse -- tests/data/case2869pegase.m
# fetch the large corpus first (gitignored)
bash benchmarks/fetch_cases.sh
# Julia (needs a Julia install)
julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'
julia --project=benchmarks benchmarks/bench_julia.jl
```
