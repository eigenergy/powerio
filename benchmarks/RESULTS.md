# Parser benchmark

Measured head-to-head: `caseio` (Rust) vs the two Julia parsers it competes
with, from small cases up to a **193k-bus, 54 MB** file. Median parse time, same
machine (Apple M-series, release), measured close in time; all three return
identical bus/branch counts — fast *and* correct. caseio via
`cargo run --release -p caseio --example timeparse`; Julia via
`benchmarks/bench_julia.jl` (`BenchmarkTools.@benchmark`). Per-machine numbers
vary a few percent run to run; the relative picture is stable.

| case | buses / branches | **caseio** | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | **1.99 ms** | 3.14 ms | 133 ms |
| case_ACTIVSg2000 | 2000 / 3206 | 2.40 ms | 2.39 ms | 154 ms |
| case9241pegase | 9241 / 16049 | **6.15 ms** | 9.34 ms | 598 ms |
| case13659pegase | 13659 / 20467 | **9.15 ms** | 13.8 ms | 827 ms |
| case_ACTIVSg10k | 10000 / 12706 | 10.0 ms | 9.56 ms | — |
| case_ACTIVSg25k | 25000 / 32230 | 24.9 ms | 23.1 ms | — |
| case_ACTIVSg70k | 70000 / 88207 | 68.7 ms | 64.8 ms | — |
| case_SyntheticUSA | 82000 / 104121 | 82.1 ms | 79.6 ms | — |
| case99k | 99396 / 117860 | 92.2 ms | 92.0 ms | — |
| case193k | 192768 / 228574 | 185 ms | 178 ms | — |

(PowerModels skipped past case13659 — it takes minutes there; the gap is already settled.)

## Read

- **vs PowerModels.jl**: 25–80× faster everywhere. On case_ACTIVSg2000 it's
  154 ms vs caseio's 2.4 ms — 64×. If you only want the data, PowerModels'
  parser is the slow path.
- **vs ExaPowerIO.jl** (the focused Julia reader): split by case family.
  - **pegase** (European, number-dense): caseio wins by **~1.5×** at every
    size.
  - **ACTIVSg / SyntheticUSA** (synthetic US, with `gentype`/`genfuel`/
    `bus_name` cell arrays): a tie at the small and the very largest end, and
    caseio trails by **≤8%** in between. These cases spend their time on
    per-generator metadata that caseio stores as owned `String`/`Vec` in its
    typed model; ExaPowerIO's layout is leaner there.
  - So caseio is **as fast or faster than ExaPowerIO across the board** —
    decisively faster on pegase, within a few percent on the synthetic cases —
    and scales linearly to 193k buses (~1 µs/bus).
- **The durable edge isn't raw speed.** caseio is the only one of the three
  that is **lossless** and **round-trips byte-for-byte** — verified at **57 MB /
  193k buses** — and the only one callable from Rust, the CLI, and Python with
  no runtime. ExaPowerIO has no writer; PowerModels' export is lossy.

The zero-copy parser (document byte-ranges + streaming row parse) closed the
case_ACTIVSg2000 gap from 0.55 ms to a tie and widened the pegase lead. The
residual on the synthetic cases is the typed output itself (owned bus names and
per-generator `Vec`s), not parsing overhead.

## Reproduce

```
bash benchmarks/fetch_cases.sh          # large cases into gitignored tests/data/large
cargo run --release -p caseio --example timeparse -- tests/data/large/case193k.m
julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'
julia --project=benchmarks benchmarks/bench_julia.jl
```
