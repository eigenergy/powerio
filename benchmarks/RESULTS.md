# Parser benchmark

casemat parses MATPOWER `.m` and can write the case back out **byte-for-byte**
(lossless round-trip). The competitive point isn't only speed — it's being fast
*and* lossless *and* round-trippable *and* callable from Rust / CLI / Python,
which no single competitor offers.

## casemat (measured)

`cargo bench --bench parse` on case2869pegase (2869 buses, 4582 branches),
release build, Apple M-series. Medians:

| op | time |
| --- | --- |
| parse | 2.69 ms |
| write (replay the source document) | 9.1 µs |
| round-trip (parse + write) | 2.68 ms |

Writing is document replay, so lossless round-trip adds ~0 over parse.

## Cross-tool comparison

The wall-clock comparison against the Julia tools needs a Julia install, so it
isn't hardcoded here — run `julia --project=benchmarks benchmarks/bench_julia.jl`
to time `PowerModels.parse_file` and `ExaPowerIO` on the same files and fill the
`ms` / `alloc` columns. The two columns that matter for positioning are filled
from this repo's test suite, not from timings:

| tool | lang | formats | lossless round-trip | parse case2869pegase |
| --- | --- | --- | --- | --- |
| **casemat** | Rust (+ CLI, Python) | MATPOWER | **yes** (byte-exact, `tests/roundtrip.rs`) | **2.69 ms** |
| ExaPowerIO.jl | Julia | MATPOWER | no writer | run harness |
| PowerModels.jl | Julia (+ JuMP stack) | MATPOWER, PSS/E v33 | no (lossy `export_matpower`) | run harness |
| matpowercaseframes | Python | MATPOWER | no writer | ~25 ms (parse only) |

"Lossless round-trip" means `parse → write → parse` reproduces the source
modulo the trailing newline, preserving every `mpc.*` field (including ones the
typed model doesn't interpret), in-matrix column-header comments, and exact
numeric tokens like `7e-05`. ExaPowerIO is fast but write-only-absent;
PowerModels carries the optimization stack and its MATPOWER export is lossy.
casemat is the only one proven byte-exact (and the writer is ~9 µs).
