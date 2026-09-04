# Matrices and graphs

`powerio-matrix` calculates sparse matrices and graph data from parsed
networks. Every numerical result carries the element mappings needed to read
its rows and columns: source bus identifiers are not dense indices, and the
dense `[0, n)` space exists only inside results that state their own mapping.

`powerio-prob` owns the calculation instances and stays matrix free; the
matrix crate projects them into sparse operators. The DC OPF bundle files are
defined in [DC OPF bundle](dcopf-bundle.md). The Rust API is documented in the
[crate reference](https://eigenergy.github.io/powerio/powerio_matrix/).

## Capabilities

| matrix | shape | calculation | notes |
| --- | --- | --- | --- |
| MATPOWER `Bp` (FDPF) | \\(n \times n\\) | `calc_bprime_matrix` | `-Im(Y_bus)` after the `makeB` `Bp` edits |
| MATPOWER `Bpp` (FDPF) | \\(n \times n\\) | `calc_bdoubleprime_matrix` | `-Im(Y_bus)` after the `makeB` `Bpp` edits |
| \\(\Re(Y_{\mathrm{bus}})\\), \\(-\Im(Y_{\mathrm{bus}})\\) | \\(n \times n\\) | `calc_admittance_matrix` | full admittance, keeps taps and shifts |
| LACPF (linear AC power flow) block | \\(2n \times 2n\\) | `calc_lacpf_matrix` | \\(\begin{bmatrix}G & -B \\\\ -B & -G\end{bmatrix}\\), flat start, indefinite |
| PowerModels DC incidence \\(A\\) | \\(m \times n\\) | `DcOperators::calc_incidence_matrix` | row \\(e\\) has \\(+1\\) at the from bus, \\(-1\\) at the to bus |
| DC branch susceptances \\(b\\) | \\(m\\) | `DcOperators::calc_branch_susceptances` | one signed susceptance per in service branch |
| DC branch flow matrix \\(B_f\\) | \\(m \times n\\) | `DcOperators::calc_branch_flow_matrix` | \\(B_f = \operatorname{diag}(b)A\\) |
| DC bus susceptance \\(B\\) | \\(n \times n\\) | `DcOperators::calc_bus_susceptance_matrix` | \\(B = A^\mathsf{T}\operatorname{diag}(b)A\\) |
| DC branch phase shift injection | \\(m\\) | `DcOperators::calc_branch_phase_shift_injection` | \\(b .* shift\\) |
| DC bus phase shift injection \\(p_{shift}\\) | \\(n\\) | `DcOperators::calc_bus_phase_shift_injection` | \\(p_{shift} = A^\mathsf{T}(b .* shift)\\) |
| DC bus injection \\(p_{bus}\\) | \\(n\\) | `DcOperators::calc_bus_injection_dc` | \\(p_{bus} = -Bv_a + p_{shift}\\) |
| weighted bus factor \\(L\\) | \\(n \times n\\) | `calc_weighted_laplacian` | \\(L = C \operatorname{diag}(w) C^\mathsf{T}\\); internal solver data |
| solver branch flow matrix | \\(m \times n\\) | `calc_solver_branch_flow_matrix` | positive solver susceptance magnitudes times \\(C^\mathsf{T}\\); internal solver data |
| PTDF | \\(m \times n\\) | `calc_ptdf` | routes through `Auto` solver selection; `calc_ptdf_lodf_with_options` exposes the choice |
| LODF | \\(m \times m\\) | `calc_lodf` | routes through `Auto` solver selection; option based builds can prune small output entries |
| AC power flow Jacobian | \\(2n \times 2n\\) | `calc_power_flow_jacobian` | polar or rectangular voltage coordinates |
| multiconductor admittance | conductor by conductor | `calc_multiconductor_admittance_matrix` | from a `MulticonductorNetwork`; Rust only in 0.11 |
| adjacency | \\(n \times n\\) | `calc_adjacency_matrix` | sparse graph adjacency |
| petgraph graph | n/a | `IndexedNetwork::to_petgraph` | `UnGraph<usize, usize>` |

Computing PTDF and LODF matrices requires a linear solve. Each calculation
(`calc_ptdf`, `calc_lodf`, `calc_ptdf_lodf`, and the option based
`calc_ptdf_lodf_with_options`) routes through the same solver selection.
`SensitivitySolver`, the `solver` field on `SensitivityOptions`, names the choice: `Dense` forces the dense grounded
factorization, `Sparse` factors the grounded DC bus susceptance matrix once
with a sparse Cholesky and reuses the factorization across every right hand
side, and `Auto` selects dense up to a reduced dimension of 512 (with a memory
ceiling) and sparse above it. The sparse path avoids forming the
\\((n-r) \times (n-r)\\) dense inverse; the PTDF/LODF outputs themselves can still
be large. The sparse path requires positive finite internal factor weights
`w = -b`, so the grounded matrix `L = -B` is positive definite after
reference coverage is checked; the dense path handles nonsingular indefinite
cases.
Every connected component must contain at least one reference bus. The DC OPF
bundle (\\(A\\), \\(b\\), \\(L\\), costs, bounds, thermal limits,
\\(C_g\\)) is prepared from a `DcOpfInstance` and documented in
[DC OPF bundle](dcopf-bundle.md).

`Bp` and `Bpp` are the fast decoupled power flow matrices from MATPOWER
`makeB`. Solvers reduce `Bp` to PV+PQ buses for active power mismatch to voltage
angle updates, and reduce `Bpp` to PQ buses for reactive power mismatch to
voltage magnitude updates. PowerIO exports the full \\(n \times n\\) matrices so
callers can apply their own bus type reduction.

`DcOperators` exposes the public DC calculations through
`calc_incidence_matrix`, `calc_branch_susceptances`,
`calc_bus_susceptance_matrix`, `calc_branch_flow_matrix`,
`calc_branch_phase_shift_injection`, `calc_bus_phase_shift_injection`,
`calc_bus_injection_dc`, and `calc_branch_flow_dc`. The `calc_*` prefix marks a newly computed result;
nouns remain stored fields or borrowed accessors. The incidence matrix follows
PowerModels: branches by buses, with \\(+1\\) at the from bus and \\(-1\\) at the
to bus. Phase shift injection stays separate. The transposed incidence factor
used inside the sensitivity and DC OPF builders is compiler data rather than a
second public incidence API.

## GridFM datasets

GridFM reading and writing need the `gridfm` cargo feature, which the CLI and
the Python wheel carry. The export is a Parquet dataset under `<case>/raw/` with `bus_data`,
`gen_data`, `branch_data`, and `y_bus_data`. A single parsed case writes one
scenario. A scenario batch row stacks snapshots that share the same element set
and uses the `scenario` column as the key. Batch construction requires unique
scenario ids, one system base, and matching bus, branch, and generator row
identities. Column names and units follow the
[GridFM data kit output schema](https://gridfm.github.io/gridfm-datakit/manual/outputs/).

GridFM reads recover its complete native balanced representation: bus types,
voltages and limits; nodal load and shunt totals; generator dispatch, bounds,
and quadratic costs; branch parameters and terminal flows; and `base_mva`.
Dense bus indices, nodal demand, and a fixed quadratic cost triple are facts of
the GridFM tables, so reading them does not report a conversion loss.

Writing a richer network reports each projection that GridFM requires. Those
diagnostics cover bus renumbering, aggregation of several load or shunt
records, omitted equipment and metadata, costs outside the fixed quadratic
columns, and terminal flows evaluated from bus voltages when the source states
no branch solution. A GridFM dataset read and written without an intervening
edit has none of these findings.

## Conventions

- **Weighted bus solver factors.** Stored nonzero off-diagonal entries are
  negative; diagonals are nonnegative and positive for buses incident to a
  positive weight branch. For
  \\(L = C \operatorname{diag}(w) C^\mathsf{T}\\) with nonnegative branch weights
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
  (`Branch::calc_effective_tap`). MATPOWER `Bp` clears bus shunts and line
  charging, sets tap magnitudes to one, and keeps phase shifts. MATPOWER `Bpp`
  keeps bus shunts, line charging, and tap magnitudes while clearing phase
  shifts. \\(Y_{\mathrm{bus}}\\) keeps both tap magnitudes and phase shifts.
- **Branch shunt admittance is stored per unit.** `Branch::charging` is the
  stored per terminal admittance when present: `g_fr`, `b_fr`, `g_to`, and
  `b_to` are already per unit on the system base. `Branch::b` is the legacy
  MATPOWER `BR_B` total projection for formats that carry only one charging
  value. Matrix builders use `Branch::calc_terminal_charging()`, so terminal values
  feed \\(Y_{\mathrm{bus}}\\) even when the legacy total is zero or stale.
- **FDPF scheme.** `Scheme` selects between the two MATPOWER fast decoupled
  forms. `Xb` clears resistance for `Bp`; `Bx` clears resistance for `Bpp`.
  The default is `Bx`.
- **Zero impedance branches.** `BuildOptions::skip_zero_impedance` controls the
  builders whose branch denominator can be zero. The default `false` returns
  `Error::ZeroImpedance`; `true` skips the branch and records the skipped
  source branch rows in `MatrixStats` as `skipped_zero_impedance` and
  `skipped_zero_impedance_branches`. Full AC admittance builders use
  \\(r^2 + x^2\\); DC incidence and reactance only FDPF forms use \\(x\\).
  The gridfm export still zeros its admittance and flow columns for these rows
  and records `dropped_zero_impedance` in `gridfm_meta.json`.
- **Reference coverage.** `IndexedNetwork::check_reference_coverage` verifies that
  every in-service island has a reference bus.
- **Branch susceptance formulas.** `BranchSusceptanceFormula` selects the
  branch susceptance vector \\(b\\) and, for formulas that include phase shifts,
  the phase shift injection. The direct PowerModels operation
  `DcOperators::calc_incidence_matrix` returns the \\(m \\times n\\) branch by
  bus matrix \\(A_{pm}\\). Each branch row has \\(+1\\) at the from bus and
  \\(-1\\) at the to bus. The signed matrix combines with
  \\(b\\) to form the direct PowerModels operators
  \\(B = A_{pm}^\mathsf{T} \operatorname{diag}(b) A_{pm}\\) and
  \\(B_f = \operatorname{diag}(b) A_{pm}\\). The phase shift injection is
  \\(p_{shift} = A_{pm}^\mathsf{T}(b \circ shift)\\), so
  \\(p_{bus}=-B\theta+p_{shift}\\). \\(b\\) is negative for an inductive
  branch, the PowerModels series susceptance sign.

  Solver preparation retains the \\(n \\times m\\) bus by branch factor
  \\(A_s=A_{pm}^\mathsf{T}\\). It uses the positive factor weight \\(w=-b\\),
  so its sparse matrix is
  \\(L=A_s \operatorname{diag}(w) A_s^\mathsf{T}=-B\\). The two
  orientations carry separate names so that no language boundary transposes
  an incidence matrix silently.

  The default `SeriesSusceptance` uses \\(b = -x/(r^2 + x^2)\\), so it reads the
  whole series impedance, plus the phase shift
  injection vector `p_shift`. A tap does not scale it. It reduces to
  \\(b = -1/x\\) when the branch has no resistance.

  `TapAdjustedReactance` reproduces MATPOWER's `makeBdc`:
  \\(b = -1/(x\tau)\\) for a transformer with tap ratio \\(\tau\\), plus `p_shift`.

  `ReactanceOnly` is the textbook \\(b = -1/x\\) with resistance, taps, and
  shifts ignored. The resulting \\(-B\\) matches MATPOWER `Bp` under
  `Scheme::Xb` when phase shifts are zero.

## Output

Matrices write as Matrix Market files or stay in memory. A symmetric matrix is
stored as its lower triangle with the `symmetric` header and 1-based indices
(`io::mtx::emit_mtx`). The `sensitivities` command writes
`<case>_ptdf.mtx`, `<case>_lodf.mtx`, and `<case>_sensitivity_meta.json`. Use
`--formula series-susceptance|tap-adjusted-reactance|reactance-only` to select
the branch susceptance formula. Use
`--solver dense|sparse|auto` to choose the PTDF/LODF solve path and
`--drop-tolerance <value>` to omit entries with absolute value at or below the
tolerance. When the CLI uses the sparse path, it writes retained Matrix
Market coordinates through temp files and does not hold the full sparse output
in memory. The Rust `calc_ptdf_lodf_with_options` API still returns `CsMat`
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
