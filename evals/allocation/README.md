# Parser and matrix allocation baseline

A "before" measurement for the 1.0 allocation work: PowerIO #293 (reader peak memory), #294 (dense sensitivity buffers), #421 (parser allocation and peak memory), and the cheap-clone network handles. A performance claim on any of those needs a matching "after" from the same case, revision, build profile, and tool, which is what this harness produces.

Not a workspace member: `evals` is in the root manifest's `exclude`, so it never enters `cargo test`, `cargo publish`, or the release archives. It links the library crates by path.

## Method

A counting global allocator wraps `System` and records, per measured call:

- **allocs** — `alloc` and `realloc` calls. `realloc` counts once, and its size delta is what moves the live total.
- **alloc_bytes** — bytes requested. Growth from a `realloc` counts; a shrink does not.
- **peak_live_bytes** — high water mark of bytes in flight during the call, measured relative to whatever the caller already had live, so one row does not inherit the previous row's residue.
- **wall_micros** — wall time of the call.

Allocation counts, byte totals, and the peak are deterministic: three consecutive runs at the recorded revision produced byte-identical values for those three columns. Wall time is not deterministic and is reported as an observation, never as a gate.

`Instant::now` brackets the call only; the allocator counters are plain relaxed atomics and the harness is single threaded, so the counts are exact rather than sampled.

## Reproducing

```bash
cd evals/allocation
cargo build --release
./target/release/powerio-eval-allocation > results-baseline.tsv
```

`POWERIO_ROOT` overrides the repository root; by default it is resolved from `CARGO_MANIFEST_DIR`, so the run does not depend on the working directory.

Output is TSV: `case`, `op`, `input_bytes`, `allocs`, `alloc_bytes`, `peak_live_bytes`, `wall_micros`.

## Recorded run

| | |
|---|---|
| Repository revision | `7131d72b815dfb82fa103817f5c11fd22ae2a562` (branch `agent/v1-module-diagnostics-source`) |
| Toolchain | `rustc 1.95.0 (59807616e 2026-04-14)`, `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` |
| Build profile | `release` with `debug = true` (this manifest's `[profile.release]`) |
| Host | aarch64-apple-darwin, macOS 25.5.0 |
| Repetitions | 3 full runs; allocs, alloc_bytes, and peak_live_bytes identical across all three |
| Results | `results-baseline.tsv` (the third run) |

## Fixtures

Nothing is generated, so there is no seed. Identity is each file's SHA-256, listed in `fixtures.sha256`.

`tests/data/case{9,118}.m`, `case2869pegase.m`, and the OpenDSS, BMOPF, PSS/E, pandapower, and PyPSA inputs are committed fixtures the ordinary tests already use.

`tests/data/large/` is gitignored and not in the repository. Fetch it with `bash benchmarks/fetch_cases.sh`, which pulls the MATPOWER cases from the MATPOWER repository and `case99k` from `goghino/opf_benchmarks`. The digests in `fixtures.sha256` pin the exact bytes measured; a run without those files simply omits the large rows and says so on stderr.

The dense PTDF and LODF rows are limited to cases at or below 3000 buses, because the path is cubic and `case2869pegase` alone already takes about 20 seconds. That limit is in the harness, not a sampling decision, and it is why the larger cases have no `ptdf` row.
