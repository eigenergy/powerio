# Matrix outputs and conventions

`powerio-matrix` builds sparse matrices and graph data from the parsed
networks. Every numerical result carries the element mappings needed to read
its rows and columns: source bus identifiers are not dense indices, and the
dense `[0, n)` space exists only inside results that state their own mapping.

`powerio-prob` owns the calculation instances (matrix free by design), and
the matrix crate projects them into sparse operators. The DC OPF bundle schema is in
[the DC OPF bundle guide](https://eigenergy.github.io/powerio/guide/dcopf-bundle.html). Per-builder API detail is in the
[crate docs](https://eigenergy.github.io/powerio/powerio_matrix/).

## Capabilities

| matrix | shape | builder | notes |
| --- | --- | --- | --- |
| MATPOWER `Bp` (FDPF) | \\(n \times n\\) | `build_bprime` | `-Im(Y_bus)` after the `makeB` `Bp` edits |
| MATPOWER `Bpp` (FDPF) | \\(n \times n\\) | `build_bdoubleprime` | `-Im(Y_bus)` after the `makeB` `Bpp` edits |
| \\(\Re(Y_{\mathrm{bus}})\\), \\(-\Im(Y_{\mathrm{bus}})\\) | \\(n \times n\\) | `build_ybus` | full admittance, keeps taps and shifts |
| LACPF (linear AC power flow) block | \\(2n \times 2n\\) | `build_lacpf` | \\(\begin{bmatrix}G & -B \\\\ -B & -G\end{bmatrix}\\), flat start, indefinite |
| signed incidence matrix \\(A\\) | \\(n \times m\\) | `build_incidence` | column \\(e\\) has \\(+1\\) at from-bus, \\(-1\\) at to-bus |
| weighted bus Laplacian \\(L\\) | \\(n \times n\\) | `build_weighted_laplacian` | \\(L = A \operatorname{diag}(w) A^\mathsf{T}\\); for DC OPF and PTDF/LODF, \\(w\\) is the branch susceptance vector \\(b\\) |
| flow map \\(B A^\mathsf{T}\\) | \\(m \times n\\) | `build_flow_map` | \\(f = B A^\mathsf{T}\theta\\) |
| PTDF | \\(m \times n\\) | `build_ptdf` | routes through `Auto` solver selection; `build_ptdf_lodf_with_options` exposes the choice |
| LODF | \\(m \times m\\) | `build_lodf` | routes through `Auto` solver selection; option based builds can prune small output entries |
| adjacency | \\(n \times n\\) | `build_adjacency` | sparse graph adjacency |
| petgraph graph | n/a | `IndexedNetwork::to_petgraph` | `UnGraph<usize, usize>` |

Computing PTDF and LODF matrices requires a linear solve. Every builder —
`build_ptdf`, `build_lodf`, `build_ptdf_lodf`, and the option based
`build_ptdf_lodf_with_options` — routes through the same solver selection.
`SensitivitySolver`, the `solver` field on `SensitivityOptions`, names the choice: `Dense` forces the dense grounded
factorization, `Sparse` factors the grounded DC bus susceptance matrix once
with a sparse Cholesky and reuses the factorization across every right hand
side, and `Auto` selects dense up to a reduced dimension of 512 (with a memory
ceiling) and sparse above it. The sparse path avoids forming the
\\((n-r) \times (n-r)\\) dense inverse; the PTDF/LODF outputs themselves can still
be large. The sparse path requires positive finite branch susceptances, so
the grounded DC bus susceptance matrix is positive definite after reference
coverage is checked; the dense path handles nonsingular indefinite cases.
Every connected component must contain at least one reference bus. The DC OPF
instance bundle (\\(A\\), \\(b\\), \\(L\\), costs, bounds, thermal limits,
\\(C_g\\)) is produced by `powerio-prob` and documented in
[the DC OPF bundle guide](https://eigenergy.github.io/powerio/guide/dcopf-bundle.html).

`Bp` and `Bpp` are the fast decoupled power flow matrices from MATPOWER
`makeB`. Solvers reduce `Bp` to PV+PQ buses for active power mismatch to voltage
angle updates, and reduce `Bpp` to PQ buses for reactive power mismatch to
voltage magnitude updates. PowerIO exports the full \\(n \times n\\) matrices so
callers can apply their own bus type reduction.

## GridFM datasets

The GridFM export is a Parquet dataset under `<case>/raw/` with `bus_data`,
`gen_data`, `branch_data`, and `y_bus_data`. A single parsed case writes one
scenario. A scenario batch row stacks snapshots that share the same element set
and uses the `scenario` column as the key.

GridFM reads recover bus types, voltages, limits, nodal load and shunt totals,
generator dispatch and bounds, branch parameters, and `base_mva`. They cannot
recover source bus IDs, per element load and shunt granularity, piecewise and
cubic costs, HVDC, or storage. These losses are returned as warnings.

## Conventions

- **Weighted bus Laplacian matrices.** Stored nonzero off-diagonal entries are
  negative; diagonals are nonnegative and positive for buses incident to a
  positive weight branch. For
  \\(L = A \operatorname{diag}(w) A^\mathsf{T}\\) with nonnegative branch weights
  \\(w\\), \\(L_{ii} = \sum_j \lvert L_{ij} \rvert\\). This is the M-matrix form
  an SDDM (symmetric diagonally dominant M-matrix) or Cholesky solver expects
  once the grounded matrix is positive definite; a consumer can recover an edge
  weight as \\(-L_{ij} > 0\\).
- **Bus indexing.** Source bus IDs are preserved on the model as a newtype and
  need not be contiguous. `IndexedNetwork::bus_index(id)` maps them into dense
  zero based indices in \\([0,n)\\), returning `None` for an unknown source ID.
  The matrix builders turn that into `Error::UnknownBus` at the point they
  need a bus that is not in the network.
- **Taps and shifts.** \\(\mathrm{tap} = 0\\) means \\(\mathrm{tap} = 1\\)
  (`Branch::effective_tap`). MATPOWER `Bp` clears bus shunts and line
  charging, sets tap magnitudes to one, and keeps phase shifts. MATPOWER `Bpp`
  keeps bus shunts, line charging, and tap magnitudes while clearing phase
  shifts. \\(Y_{\mathrm{bus}}\\) keeps both tap magnitudes and phase shifts.
- **Branch shunt admittance is stored per unit.** `Branch::charging` is the
  stored per terminal admittance when present: `g_fr`, `b_fr`, `g_to`, and
  `b_to` are already per unit on the system base. `Branch::b` is the legacy
  MATPOWER `BR_B` total projection for formats that carry only one charging
  value. Matrix builders use `Branch::terminal_charging()`, so terminal values
  feed \\(Y_{\mathrm{bus}}\\) even when the legacy total is zero or stale.
- **FDPF scheme.** `Scheme` selects between the two MATPOWER fast decoupled
  variants. `Xb` clears resistance for `Bp`; `Bx` clears resistance for `Bpp`.
  The default is `Bx`.
- **Zero impedance branches.** `BuildOptions::skip_zero_impedance` controls the
  builders whose branch denominator can be zero. The default `true` skips the
  branch and records the skipped source branch rows in `MatrixStats` as
  `skipped_zero_impedance` and `skipped_zero_impedance_branches`; `false`
  returns `Error::ZeroImpedance`. Full AC admittance builders use
  \\(r^2 + x^2\\); DC incidence and reactance only FDPF variants use \\(x\\).
  The gridfm export still zeros its admittance and flow columns for these rows
  and records `dropped_zero_impedance` in `gridfm_meta.json`.
- **Reference coverage.** `IndexedNetwork::check_reference_coverage` verifies that
  every in-service island has a reference bus.
- **Susceptance conventions for the DC approximation.** `DcConvention` selects
  the branch susceptance vector \\(b\\) and, for conventions that carry shifts,
  the phase shift injection. In this crate \\(A\\) is the \\(n \times m\\)
  bus by branch incidence matrix: a column has \\(+1\\) at the from bus and
  \\(-1\\) at the to bus. Public values carry PowerModels signs: \\(b\\) is
  negative for an inductive branch, the imaginary part of the series
  admittance the selected formula models. Therefore
  \\(B = A \operatorname{diag}(b) A^\mathsf{T}\\),
  \\(B_f = \operatorname{diag}(b) A^\mathsf{T}\\),
  \\(p_{shift} = A(b \circ shift)\\), and
  \\(p_{bus} = -B\,v_a + p_{shift}\\). The positive factor weight a sparse
  solver assembles is the separate `DcConvention::solver_edge_weight`, the
  elementwise negation of public \\(b\\); sign conversion happens while
  filling public output buffers, never inside them. This reverses 0.9, whose
  `branch_susceptance` returned the positive weight — the
  [migration guide](migration-v1.md) states the consumer action.

  The default `SeriesSusceptance` uses \\(b = -x/(r^2 + x^2)\\), so it reads
  the whole series impedance, plus the phase shift injection vector
  `p_shift`. A tap does not scale it. It reduces to \\(b = -1/x\\) when the
  branch has no resistance.

  `TapAdjustedReactance` matches MATPOWER's `makeBdc` up to MATPOWER's own
  sign spelling: \\(b = -1/(x\tau)\\) for a transformer with tap ratio
  \\(\tau\\), plus `p_shift`.

  `ReactanceOnly` is the textbook \\(b = -1/x\\) with resistance, taps, and
  shifts ignored. The resulting grounded system matches MATPOWER `Bp` under
  `Scheme::Xb` when phase shifts are zero. Reproducing a published result
  needs it exactly as written, so it stays.

## Output

Matrices write as Matrix Market files or stay in memory. A symmetric matrix is
stored as its lower triangle with the `symmetric` header and 1-based indices
(`io::mtx::write_mtx`). The `sensitivities` command writes
`<case>_ptdf.mtx`, `<case>_lodf.mtx`, and `<case>_sensitivity_meta.json`. Use
`--solver dense|sparse|auto` to choose the PTDF/LODF solve path and
`--drop-tolerance <value>` to omit entries with absolute value at or below the
tolerance. When the CLI uses the sparse path, it writes retained Matrix
Market coordinates through temp files and does not hold the full sparse output
in memory. The Rust `build_ptdf_lodf_with_options` API still returns `CsMat`
values and is intended for outputs that fit in memory. The metadata records the
requested solver, the actual solver path, matrix dimensions, nonzero counts,
tolerance, and dropped entry counts. The `dcopf` CLI subcommand bundles its
matrix family with a JSON manifest.

The standard case solver property fixture lives at
`powerio-matrix/tests/fixtures/solver_matrix_stats.json`. It records `bprime`,
`bdoubleprime`, and `ybus_imag` stats for `case9`, `case14`, `case30`, `case57`, and
`case118`: `n`, `nnz`, min diagonal, M-matrix sign pattern, diagonal dominance
margin, zero impedance skips, row sum checks, SPD checks, and a condition
estimate when the solver input is SPD.

`IndexedNetwork::to_petgraph` returns the network as an undirected
[petgraph](https://docs.rs/petgraph) graph, one node per bus and one edge per
in-service branch. The connectivity report and the radial check are built on
it. Use the returned graph directly for other petgraph algorithms.
