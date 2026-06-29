# Matrix outputs and conventions

The `powerio-matrix` crate builds sparse matrices and graph outputs for common power system representations. The outputs are derived from a parsed `Network`. The builders take the densely indexed `IndexedNetwork`, which maps bus ids to a
contiguous \\([0,n)\\).

The DC OPF bundle has its own schema in
[the DC OPF bundle guide](https://eigenergy.github.io/powerio/guide/dcopf-bundle.html). Per-builder API detail is in the
[crate docs](https://eigenergy.github.io/powerio/powerio_matrix/).

## Capabilities

| matrix | shape | builder | notes |
| --- | --- | --- | --- |
| B' (FDPF) | \\(n \times n\\) | `build_bprime` | singular positive Laplacian, \\(\operatorname{rank}(L) = n - 1\\), shuntless |
| B'' (FDPF) | \\(n \times n\\) | `build_bdoubleprime` | SDDM when bus shunts are present |
| \\(\Re(Y_{\mathrm{bus}})\\), \\(-\Im(Y_{\mathrm{bus}})\\) | \\(n \times n\\) | `build_ybus` | full admittance, keeps taps and shifts |
| LACPF (linear AC power flow) block | \\(2n \times 2n\\) | `build_lacpf` | \\(\begin{bmatrix}G & -B \\\\ -B & -G\end{bmatrix}\\), flat start, indefinite |
| signed incidence \\(A\\) | \\(n \times m\\) | `build_incidence` | column \\(e\\) has \\(+1\\) at from-bus, \\(-1\\) at to-bus |
| weighted Laplacian \\(L\\) | \\(n \times n\\) | `build_weighted_laplacian` | \\(L = A \operatorname{diag}(w) A^\mathsf{T}\\), `ground_at` removes a row/col |
| flow map \\(B A^\mathsf{T}\\) | \\(m \times n\\) | `build_flow_map` | \\(f = B A^\mathsf{T}\theta\\) |
| PTDF | \\(m \times n\\) | `build_ptdf` | dense; factors the Laplacian grounded at the reference buses |
| LODF | \\(m \times m\\) | `build_lodf` | dense DC line-outage factors |
| adjacency | \\(n \times n\\) | `build_adjacency` | sparse graph adjacency |
| petgraph graph | n/a | `IndexedNetwork::to_petgraph` | `UnGraph<bus_idx, branch_idx>` |

Computing PTDF and LODF matrices requires a linear solve. Both factor the
Laplacian with one row and column removed for each reference bus, using the dense
Cholesky in
`matrix::sensitivity`. Every connected component must contain at least one
reference bus. PTDF is dense \\(m \times n\\). The DC OPF
instance bundle (\\(A\\), \\(b\\), \\(L\\), costs, bounds, thermal limits, \\(C_g\\)) is documented in
[the DC OPF bundle guide](https://eigenergy.github.io/powerio/guide/dcopf-bundle.html).

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

- **Positive Laplacian matrices.** Off-diagonal \\(< 0\\), diagonal \\(> 0\\), with
  \\(L_{ii} = \sum_j \lvert L_{ij} \rvert\\)
  for B' susceptance matrices. This is the M-matrix form an SDDM (symmetric diagonally dominant
  M-matrix) or Cholesky solver expects; a consumer can recover an edge weight as
  \\(-L_{ij} > 0\\).
- **Bus indexing.** Bus ids are 1-based and preserved on the model as a newtype
  (the Rust [New Type Idiom](https://doc.rust-lang.org/rust-by-example/generics/new_types.html)).
  `IndexedNetwork::bus_index(id)` is the only mapping into the dense \\([0,n)\\); an id
  out of range is an `Error::UnknownBus`.
- **Taps and shifts.** \\(\mathrm{tap} = 0\\) means \\(\mathrm{tap} = 1\\)
  (`Branch::effective_tap`). B' ignores taps and shifts; B'' keeps taps and
  zeros only shifts; \\(Y_{\mathrm{bus}}\\) keeps both.
- **Branch shunt admittance is stored per unit.** `Branch::charging` is the
  stored per terminal admittance when present: `g_fr`, `b_fr`, `g_to`, and
  `b_to` are already per unit on the system base. `Branch::b` is the legacy
  MATPOWER `BR_B` total projection for formats that carry only one charging
  value. Matrix builders use `Branch::terminal_charging()`, so terminal values
  feed \\(Y_{\mathrm{bus}}\\) even when the legacy total is zero or stale.
- **B' scheme.** `Scheme` selects between the two fast decoupled load flow
  variants for B': `Xb` weights a branch by \\(1/x\\) (series resistance ignored),
  `Bx` (the default) by \\(x/(r^2 + x^2)\\).
- **Zero impedance branches.** B' skips them by default
  (`BuildOptions::skip_zero_impedance`; set it `false` to get
  `Error::ZeroImpedance`). \\(Y_{\mathrm{bus}}\\) scatters no admittance for them
  (\\(r^2 + x^2 = 0\\))
  and the incidence builder drops them, both unconditionally. The gridfm export
  counts the drops (`dropped_zero_impedance` in `gridfm_meta.json`).
- **Reference coverage.** `IndexedNetwork::check_reference_coverage` verifies that
  every in-service island has a reference bus.
- **Susceptance conventions for the DC approximation.** `DcConvention` selects
  the branch weight the DC builders (incidence, weighted Laplacian, PTDF/LODF,
  the DC OPF bundle) use. The default `PaperPure` is the textbook DC power flow
  weight \\(b = 1/x\\), taps and shifts ignored; the resulting
  \\(L = A \operatorname{diag}(b) A^\mathsf{T}\\)
  equals B' under `Scheme::Xb`. `Matpower` reproduces MATPOWER's `makeBdc`:
  \\(b = 1/(x\tau)\\) for a transformer with tap ratio \\(\tau\\), plus the phase shift
  injection vector `p_shift`.

## Output

Matrices write as Matrix Market files or stay in memory. A symmetric matrix is
stored as its lower triangle with the `symmetric` header and 1-based indices
(`io::mtx::write_mtx`). The `sensitivities` and `dcopf` CLI subcommands bundle
the relevant family with a JSON manifest.

`IndexedNetwork::to_petgraph` returns the network as an undirected
[petgraph](https://docs.rs/petgraph) graph, one node per bus and one edge per
in-service branch. The connectivity report and the radial check are built on
it. Use the returned graph directly for other petgraph algorithms.
