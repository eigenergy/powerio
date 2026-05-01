# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Purpose

`gridforge` is a Rust **library + CLI/TUI** that parses power network case files and emits canonical linear-algebra and graph representations. It exists to feed the [Scalable Approximate Cholesky][sac] solver and the GridFM ML pipeline (`gridfm-datakit` / `gridfm-graphkit`).

[sac]: https://github.com/UnLochlann/Scalable-Approximate-Cholesky

**Supported inputs (current):**
- MATPOWER 7.x `.m` (transmission, balanced).

**Planned inputs:**
- OpenDSS `.dss` (distribution, unbalanced 3-phase).
- PSS/E `.raw` (transmission planning).
- PowerModels.jl JSON.

**Outputs:**
- B' (FDPF, shuntless) — singular positive Laplacian, rank n-1.
- B'' (FDPF, shunts + taps) — strictly SDDM when bus shunts are present.
- Re(Y_bus) = G — full conductance matrix.
- -Im(Y_bus) — full susceptance Laplacian (positive convention).
- LACPF block — `[[G, -B], [-B, -G]]`, indefinite saddle-point (Talkington's flat-start linearization).
- petgraph `UnGraph<bus_idx, branch_idx>` view + connectivity / radial diagnostics.

**Planned outputs:**
- LinDist3Flow matrices (radial unbalanced 3-phase distribution).
- Apache Parquet rows matching `gridfm-datakit`'s `bus_data` / `branch_data` / `y_bus_data` schemas.

## Common commands

```bash
cargo build --release
cargo test                                  # 22 lib + 8 integration
cargo clippy --all-targets

# Binary
gridforge                                   # launches TUI
gridforge batch -i tests/data -o /tmp/forge --matrices bprime,bdoubleprime
gridforge gen --topology lattice --n 1024 -o /tmp/synth
gridforge verify tests/data/case30.m --kind bdoubleprime
```

## Architecture

```
src/
├── lib.rs                   # public re-exports
├── error.rs                 # thiserror Error enum
├── case.rs                  # MpcCase, Bus, Branch, ConnectivityReport
│                            #   + petgraph view (to_petgraph, is_radial,
│                            #     n_connected_components, connectivity_report)
├── parser/
│   ├── mod.rs               # format dispatcher (currently only matpower)
│   └── matpower/
│       ├── mod.rs           # parse_matpower / parse_matpower_file
│       ├── tokens.rs        # comment stripping (string-aware)
│       ├── matlab.rs        # mpc.<field>=… extractor
│       └── tests.rs
├── matrix/
│   ├── mod.rs               # BuildOptions, Scheme, MatrixStats, sddm_check
│   ├── triplet.rs           # CooBuilder (HashMap-backed; O(nnz) inserts)
│   ├── bprime.rs
│   ├── bdoubleprime.rs
│   ├── ybus.rs              # full Y_bus = G + jB; YbusFlags for B''
│   ├── lacpf.rs
│   └── tests.rs
├── io/
│   ├── mod.rs
│   ├── mtx.rs               # spec-compliant lower-triangle symmetric writer
│   ├── npy.rs               # hand-rolled NumPy v2.0 writer
│   └── meta.rs              # CaseMetadata serde JSON
├── pipeline.rs              # case → outputs orchestrator
├── synth/                   # tree, lattice, pegase-like generators
└── tui/                     # ratatui app (lives in lib for testability)
    ├── mod.rs               # event loop, key dispatch, batch worker
    ├── app.rs               # App state, screen state machine
    ├── screens.rs           # 5 screens (Browse / Inspect / Batch / Synth / Help)
    ├── theme.rs
    ├── log_pane.rs          # tracing → ring buffer pipe
    └── sparsity.rs          # unicode-block sparsity preview

tests/
├── matpower_cases.rs        # integration tests
└── data/                    # vendored case9/14/30/57/118 (BSD-3 from matpower)
```

## Things to know before editing

- **Sign convention**: positive Laplacian (off-diag negative, diag = sum |off-diag|). This matches the Talkington LACPF formulation and is what the Cholesky solver wants.
- **Bus IDs**: MATPOWER 1-based; `MpcCase::bus_index(id)` is the only sanctioned mapping into dense `[0, n)`. Do NOT clamp out-of-range — return `Error::UnknownBus`.
- **`BR_B` is already p.u.**; never divide by `base_mva` again.
- **`tap == 0` ⇒ tap = 1** (MATPOWER convention) — `Branch::effective_tap()` enforces this.
- **B' ignores taps/shifts**, B'' zeros only shifts, Y_bus keeps both.
- **MTX output** is lower-triangle, 1-based, spec-compliant. `sprs::io::write_matrix_market_sym` writes the *upper* triangle (non-spec), so `io::mtx::write_mtx` ships its own writer.
- **`CooBuilder`**: HashMap-backed COO with O(nnz) inserts; replaces the previous O(nnz²) Vec-search.
- **TUI is in the library** (`src/tui/`) so it's testable via `ratatui::backend::TestBackend`. The binary just calls `gridforge::tui::run`.
- **petgraph view**: `MpcCase::to_petgraph()` returns a `UnGraph<usize, usize>` where node weight = dense bus index, edge weight = branch index. Use it for connectivity, radial detection, spanning trees (LinDist3Flow).
- **Adding a new format**: create `src/parser/<format>/mod.rs`, expose `parse_<format>` and `parse_<format>_file`, and re-export from `src/parser/mod.rs`. Keep `MpcCase` as the unifying domain type — every format produces one.

## Test fixtures

`tests/data/case{9,14,30,57,118}.m` are vendored verbatim from `https://github.com/MATPOWER/matpower/tree/master/data` (BSD-3). Add new sizes by curl-ing upstream.

## Relationship to GridFM

`gridforge` is intended to be the **fast Rust data layer** beneath `gridfm-datakit` (Python, scenario generation) and `gridfm-graphkit` (PyTorch Geometric, GNN training). The `Pipeline` Parquet output (planned) will write the same column schemas as `gridfm-datakit` so the two can be drop-in interchangeable for the parsing+matrix-extraction step.
