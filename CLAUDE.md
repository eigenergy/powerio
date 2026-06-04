# CLAUDE.md

Guidance for Claude Code working in this repo.

## Purpose

Rust library + CLI/TUI. Parses power network case files and emits sparse matrices and graph views for any downstream solver. (Planned) feeds the GridFM ML pipeline.

Inputs (today): MATPOWER `.m`. Planned: OpenDSS, PSS/E, PowerModels JSON.

Outputs:
- B' (FDPF, shuntless). Singular positive Laplacian, rank n-1.
- B'' (FDPF, with shunts and taps). SDDM when bus shunts are present.
- `Re(Y_bus)`, `-Im(Y_bus)` (full).
- LACPF block `[[G, -B], [-B, -G]]` (linear AC power flow, flat start, 2n×2n, indefinite).
- Adjacency (`MatrixKind::Adjacency`); PTDF and LODF (DC sensitivities, `sensitivities` subcommand).
- DC-OPF instance bundle (`dcopf` subcommand, `opf_pipeline::write_dcopf_bundle`): signed incidence `A` (n×m), branch susceptance `b`, weighted Laplacian `L = A diag(b) Aᵀ` and its slack-grounded form, flow map `B Aᵀ`, generator cost `Q`/`c`, bounds, thermal limits `f̄`, generator→bus `C_g`, nodal load `p_d`, `e_r`.
- petgraph `UnGraph<bus_idx, branch_idx>` view + connectivity / radial diagnostics.
- Planned: LinDist3Flow, Parquet (gridfm-datakit schema).

## Commands

```
cargo build --release
cargo test
cargo clippy --all-targets

netmat                                                    # TUI
netmat batch -i tests/data -o out --matrices bprime,bdoubleprime
netmat gen --topology lattice --n 1024 -o out
netmat verify tests/data/case30.m --kind bdoubleprime
netmat dcopf tests/data/case30.m -o out
netmat sensitivities tests/data/case30.m -o out
```

## Layout

```
src/
├── lib.rs                   # public re-exports
├── error.rs                 # thiserror Error
├── case.rs                  # MpcCase, Bus, Branch, Generator, GenCost,
│                            #   Storage, ConnectivityReport
│                            #   + petgraph view: to_petgraph,
│                            #     is_radial, n_connected_components,
│                            #     connectivity_report
├── parser/
│   ├── mod.rs               # format dispatcher
│   └── matpower/            # parse_matpower(_file), tokens, matlab
├── matrix/
│   ├── mod.rs               # BuildOptions, Scheme, MatrixStats, sddm_check
│   ├── triplet.rs           # CooBuilder (HashMap, O(nnz); new_rect for rectangular)
│   ├── bprime.rs
│   ├── bdoubleprime.rs
│   ├── ybus.rs              # Y_bus = G + jB; YbusFlags drives B''
│   ├── lacpf.rs
│   ├── incidence.rs         # A, b, B Aᵀ, P_shift; DcConvention
│   ├── laplacian.rs         # L = A diag(w) Aᵀ, ground_at, GroundMap, e_r
│   ├── adjacency.rs         # 0/1 adjacency matrix
│   ├── sensitivity.rs       # PTDF, LODF (DC sensitivities)
│   └── opf.rs               # OpfInstance: Q, c, bounds, f̄, C_g, p_d; Units
├── io/
│   ├── mtx.rs               # spec compliant lower triangle symmetric writer
│   ├── npy.rs               # NumPy v2.0 writer
│   └── meta.rs              # CaseMetadata serde JSON
├── pipeline.rs              # case → outputs (square MatrixKind family)
├── opf_pipeline.rs          # case → DC-OPF bundle directory + manifest
├── synth/                   # tree, lattice, pegase like generators
└── tui/                     # ratatui app (in lib so it's testable)
    ├── mod.rs               # event loop, key dispatch, batch worker
    ├── app.rs               # state machine
    ├── screens.rs           # Browse / Inspect / Batch / Synth / Help
    ├── theme.rs
    ├── log_pane.rs          # tracing → ring buffer
    └── sparsity.rs          # unicode block sparsity preview

tests/
├── matpower_cases.rs        # integration tests
└── data/                    # vendored case9 / 14 / 30 / 57 / 118 (BSD-3 from matpower)
```

## Things to know before editing

- **Sign convention.** Positive Laplacian: off diag negative, diag positive, `diag = sum |off-diag|` for B'. The positive (M-matrix) Laplacian form SDDM solvers expect.
- **Bus IDs.** MATPOWER 1 based; `MpcCase::bus_index(id)` is the only mapping into dense `[0, n)`. Don't clamp out of range — return `Error::UnknownBus`.
- **`BR_B` is already per unit.** Never divide by `base_mva` again.
- **`tap == 0` ⇒ `tap = 1`.** Use `Branch::effective_tap()`.
- **B' ignores taps and shifts. B'' zeros only shifts. Y_bus keeps both.**
- **DC-OPF Laplacian.** `L = A diag(b) Aᵀ` is built from the same `A`, `b` factors `build_incidence` returns (so `L` and the reweighted `L₁` share a factorization), and equals `build_bprime` in the XB scheme. Default `b = 1/x` (paper-pure); `DcConvention::Matpower` uses `1/(x·τ)` plus a phase-shift injection.
- **DC-OPF is bus-indexed.** Generation is nodal (`p_g ∈ ℝⁿ`), so `Q`, `c`, and bounds are length-n (zero at load buses), scattered from generator space through `C_g`; gen-space vectors ride along as provenance. Cost map: MATPOWER `c2 p² + c1 p` → `q = 2c2`, `c = c1`. Per-unit by default (`Units::PerUnit` scales `q` by `base²`, `c` by `base`).
- **`gen`/`gencost` are optional.** A power-flow-only case parses with `gens` empty; the OPF builders return `Error::NoGenerators`. Exactly one `BusType::Ref` is required (`MpcCase::reference_bus_index`).
- **PTDF/LODF need a solve.** They factor the slack-grounded Laplacian `ground_at(L, r)` (SPD) with a self-contained dense Cholesky (`matrix::sensitivity`) — no external solver dep. PTDF is dense `m×n`; large-scale sparse PTDF is future work.
- **MTX output is lower triangle, 1 based, spec compliant.** `sprs::io::write_matrix_market_sym` writes the *upper* triangle, so `io::mtx::write_mtx` ships its own writer.
- **`CooBuilder`.** HashMap COO with O(nnz) inserts; replaces the old O(nnz²) Vec search.
- **TUI lives in the library.** `src/tui/`. Testable via `ratatui::backend::TestBackend`. Binary calls `netmat::tui::run`.
- **petgraph view.** `MpcCase::to_petgraph()` returns `UnGraph<usize, usize>` where node weight = dense bus index, edge weight = branch index. Use it for connectivity, radial detection, spanning trees (LinDist3Flow).
- **Adding a format.** New `src/parser/<format>/mod.rs`, expose `parse_<format>(_file)`, re-export. Keep `MpcCase` as the unifying domain type.

## Test fixtures

`tests/data/case{9,14,30,57,118}.m` are vendored verbatim from `https://github.com/MATPOWER/matpower/tree/master/data` (BSD-3). Add new sizes by curl from upstream.

## Relationship to GridFM

Intended as the fast Rust data layer beneath `gridfm-datakit` (Python, scenario generation) and `gridfm-graphkit` (PyTorch Geometric, GNN training). Planned Parquet output (issue #4) matches gridfm-datakit's column schemas.
