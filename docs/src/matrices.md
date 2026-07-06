# Matrix outputs and conventions

The `powerio-matrix` crate builds sparse matrices and graph outputs for common power system representations. The outputs are derived from a parsed `Network`. The builders take the densely indexed `IndexedNetwork`, which maps bus ids to a
contiguous \\([0,n)\\).

The DC OPF bundle has its own schema in
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
| PTDF | \\(m \times n\\) | `build_ptdf` | dense oracle builder; `build_ptdf_lodf_with_options` can use iterative solves |
| LODF | \\(m \times m\\) | `build_lodf` | dense oracle builder; option based builds can prune small output entries |
| adjacency | \\(n \times n\\) | `build_adjacency` | sparse graph adjacency |
| petgraph graph | n/a | `IndexedNetwork::to_petgraph` | `UnGraph<bus_idx, branch_idx>` |

Computing PTDF and LODF matrices requires a linear solve. The stable
`build_ptdf`, `build_lodf`, and `build_ptdf_lodf` builders keep the dense
grounded inverse path and remain the small case oracle. The option based
`build_ptdf_lodf_with_options` path accepts `SensitivityOptions`: `Dense` forces
the dense oracle path, `Iterative` uses preconditioned conjugate gradient on one
grounded right hand side at a time, and `Auto` selects dense up to a reduced
dimension of 512 and iterative above it. The iterative path avoids forming the
\\((n-r) \times (n-r)\\) dense inverse; the PTDF/LODF outputs themselves can still
be large. The iterative path requires positive finite branch susceptances, so
the grounded DC bus susceptance matrix is positive definite after reference
coverage is checked; the dense path remains the fallback for nonsingular
indefinite cases.
Every connected component must contain at least one reference bus.
The DC OPF
instance bundle (\\(A\\), \\(b\\), \\(L\\), costs, bounds, thermal limits, \\(C_g\\)) is documented in
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

GridFM read is the ML to classical return path. It recovers bus types, voltages,
limits, nodal load and shunt totals, generator dispatch and bounds, branch
parameters, and `base_mva`. It cannot recover original bus ids, per element load
and shunt granularity, piecewise and cubic costs, HVDC, or storage; those losses
are returned as warnings.

## Conventions

- **Weighted bus Laplacian matrices.** Stored nonzero off-diagonal entries are
  negative; diagonals are nonnegative and positive for buses incident to a
  positive weight branch. For
  \\(L = A \operatorname{diag}(w) A^\mathsf{T}\\) with nonnegative branch weights
  \\(w\\), \\(L_{ii} = \sum_j \lvert L_{ij} \rvert\\). This is the M-matrix form
  an SDDM (symmetric diagonally dominant M-matrix) or Cholesky solver expects
  once the grounded matrix is positive definite; a consumer can recover an edge
  weight as \\(-L_{ij} > 0\\).
- **Bus indexing.** Bus ids are 1-based and preserved on the model as a newtype
  (the Rust [New Type Idiom](https://doc.rust-lang.org/rust-by-example/generics/new_types.html)).
  `IndexedNetwork::bus_index(id)` is the only mapping into the dense \\([0,n)\\); an id
  out of range is an `Error::UnknownBus`.
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
  the branch susceptance vector \\(b\\) and, for the MATPOWER convention, the
  phase shift injection. The signed incidence matrix \\(A\\) combines with
  \\(b\\) to form the DC bus susceptance matrix
  \\(L = A \operatorname{diag}(b) A^\mathsf{T}\\), which feeds PTDF/LODF and the
  DC OPF bundle. The default `PaperPure` is the textbook DC power flow weight
  \\(b = 1/x\\), taps and shifts ignored; the resulting
  \\(L = A \operatorname{diag}(b) A^\mathsf{T}\\)
  matches MATPOWER `Bp` under `Scheme::Xb` when phase shifts are zero.
  `Matpower` reproduces MATPOWER's `makeBdc`:
  \\(b = 1/(x\tau)\\) for a transformer with tap ratio \\(\tau\\), plus the phase shift
  injection vector `p_shift`.

## Output

Matrices write as Matrix Market files or stay in memory. A symmetric matrix is
stored as its lower triangle with the `symmetric` header and 1-based indices
(`io::mtx::write_mtx`). The `sensitivities` command writes
`<case>_ptdf.mtx`, `<case>_lodf.mtx`, and `<case>_sensitivity_meta.json`. Use
`--solver dense|iterative|auto` to choose the PTDF/LODF solve path and
`--drop-tolerance <value>` to omit entries with absolute value at or below the
tolerance. When the CLI uses the iterative path, it writes retained Matrix
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
