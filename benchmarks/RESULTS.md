# Parser benchmark

Measured head-to-head: `caseio` (Rust) vs the two Julia parsers it competes
with, from small cases up to a **193k-bus, 54 MB** file. Median parse time, same
machine (Apple M-series, release), measured in one session; all three return
identical bus/branch counts — fast *and* correct. caseio via
`cargo run --release -p caseio --example timeparse`; Julia via
`benchmarks/bench_julia.jl` (`BenchmarkTools.@benchmark`). Per-machine numbers
vary a few percent run to run; the relative picture is stable.

`parse_matpower` is the single parse path: it builds the typed `MpcCase` and
retains the original source text so the writer can echo it for a byte-exact
round-trip. The round-trip costs no extra parse pass — just keeping the source
string — so this is both the apples-to-apples number against
ExaPowerIO/PowerModels (which also return data) and the lossless number.

| case | buses / branches | **caseio** | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | **1.90 ms** | 3.86 ms | 133 ms |
| case_ACTIVSg2000 | 2000 / 3206 | **2.08 ms** | 3.06 ms | 148 ms |
| case9241pegase | 9241 / 16049 | **5.62 ms** | 9.85 ms | 620 ms |
| case13659pegase | 13659 / 20467 | **8.34 ms** | 15.1 ms | 893 ms |
| case_ACTIVSg10k | 10000 / 12706 | **9.03 ms** | 9.62 ms | — |
| case_ACTIVSg25k | 25000 / 32230 | **22.7 ms** | 23.8 ms | — |
| case_ACTIVSg70k | 70000 / 88207 | **61.2 ms** | 67.0 ms | — |
| case_SyntheticUSA | 82000 / 104121 | **72.0 ms** | 82.3 ms | — |
| case99k | 99396 / 117860 | **83.5 ms** | 94.0 ms | — |
| case193k | 192768 / 228574 | **169 ms** | 194 ms | — |

(PowerModels skipped past case13659 — it takes minutes there; the gap is already settled.)

## Read

- **vs PowerModels.jl**: 25–70× faster everywhere. On case_ACTIVSg2000 it's
  148 ms vs caseio's 2.1 ms — 70×.
- **vs ExaPowerIO.jl** (the focused Julia reader): caseio wins every case —
  **~1.5–2× on pegase** (European, number-dense) and **7–15% on the synthetic US
  cases** (ACTIVSg / SyntheticUSA, with `gentype`/`genfuel`/`bus_name` cell
  arrays) — and it wins while *also* giving a lossless round-trip the others
  don't.
- **Round-trip is free here.** An earlier design built a second pass that located
  and stored every assignment's byte range so the file could round-trip; that
  pass was ~half of parse time (38% at case118, 51% at case2869, rising with
  size). The current path drops it: it retains the raw source text (one cheap
  copy) and the writer echoes it, so the byte-exact round-trip costs no extra
  parse pass.
- **The durable edge isn't raw speed.** caseio is the only one of the three that
  is **lossless** and **round-trips byte-for-byte** — verified at **54 MB / 193k
  buses** — and the only one callable from Rust, the CLI, and Python with no
  runtime. ExaPowerIO has no writer; PowerModels' export is lossy.

## Reproduce

```
bash benchmarks/fetch_cases.sh          # large cases into gitignored tests/data/large
cargo run --release -p caseio --example timeparse -- tests/data/large/case193k.m
julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'
julia --project=benchmarks benchmarks/bench_julia.jl
```
