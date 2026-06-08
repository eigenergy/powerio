# Matrix outputs and conventions

`powerio-matrix` builds sparse matrices and graph views from a [`Network`]. The
builders take the dense-indexed `IndexedNetwork`, which maps MATPOWER bus ids to a
contiguous `[0, n)`. This document is the convention reference; the DC-OPF bundle
has its own schema in [dcopf-bundle.md](dcopf-bundle.md), and per-builder API
detail is in the [crate docs](https://eigenergy.github.io/powerio/powerio_matrix/).

## What it builds

| matrix | shape | builder | notes |
| --- | --- | --- | --- |
| B' (FDPF) | n×n | `build_bprime` | singular positive Laplacian, rank n−1, shuntless |
| B'' (FDPF) | n×n | `build_bdoubleprime` | SDDM when bus shunts are present |
| `Re(Y_bus)`, `-Im(Y_bus)` | n×n | `build_ybus` | full admittance, keeps taps and shifts |
| LACPF block | 2n×2n | `build_lacpf` | `[[G, −B], [−B, −G]]`, flat start, indefinite |
| signed incidence `A` | n×m | `build_incidence` | column `e`: `+1` at from-bus, `−1` at to-bus |
| weighted Laplacian `L` | n×n | `build_weighted_laplacian` | `L = A diag(w) Aᵀ`, `ground_at` removes a row/col |
| flow map `B Aᵀ` | m×n | `build_flow_map` | `f = B Aᵀ θ` |
| PTDF | m×n | `build_ptdf` | dense; factors the Laplacian grounded at the slack |
| LODF | m×m | `build_lodf` | dense DC line-outage factors |
| adjacency | n×n | `build_adjacency` | `MatrixKind::Adjacency` |
| petgraph view | — | `IndexedNetwork::to_petgraph` | `UnGraph<bus_idx, branch_idx>` |

PTDF and LODF need a linear solve; both factor `ground_at(L, r)` (SPD) with the
self-contained dense Cholesky in `matrix::sensitivity`, no external solver. PTDF is
dense `m×n`; sparse large-scale PTDF is future work. The DC-OPF instance bundle (`A`,
`b`, `L`, costs, bounds, thermal limits, `C_g`) is documented in
[dcopf-bundle.md](dcopf-bundle.md).

## Conventions

- **Positive Laplacian.** Off-diagonal `< 0`, diagonal `> 0`, with `diag = Σ|off-diag|`
  for B'. This is the M-matrix form an SDDM or Cholesky solver expects; a consumer
  recovers an edge weight as `−L[i,j] > 0`.
- **Bus indexing.** MATPOWER bus ids are 1-based and preserved on the model.
  `IndexedNetwork::bus_index(id)` is the only mapping into the dense `[0, n)`; an id
  out of range is an `Error::UnknownBus`, never clamped.
- **Taps and shifts.** `tap == 0` means `tap = 1` (`Branch::effective_tap`). B'
  ignores taps and shifts; B'' keeps taps and zeros only shifts; Y_bus keeps both.
- **`BR_B` is already per unit.** Line charging susceptance is per unit on `baseMVA`
  in MATPOWER; never divide by `base_mva` again.
- **DC susceptance.** The default is `b = 1/x` (`DcConvention::PaperPure`, taps and
  shifts ignored), which makes `L = A diag(b) Aᵀ` equal `build_bprime`.
  `DcConvention::Matpower` uses `b = 1/(x·τ)` plus a phase-shift injection `p_shift`.

## Output

Matrices write as Matrix Market or stay in memory. A symmetric matrix writes the
lower triangle with the `symmetric` header, 1-based indices, via `io::mtx::write_mtx`
(sprs writes the upper triangle, so powerio ships its own spec-compliant writer).
The `sensitivities` and `dcopf` CLI subcommands bundle the relevant family with a
JSON manifest.

The petgraph view (`UnGraph<usize, usize>`, node weight = dense bus index, edge
weight = branch index) backs the connectivity and radial diagnostics and is the
substrate for spanning-tree work (LinDist3Flow).
