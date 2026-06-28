# Matrices and GridFM

`powerio-matrix` builds analysis views from a parsed `Network`. It does not
change the source model; it derives indexed rows through `IndexedNetwork`.

Matrix outputs:

| output | shape | convention |
| --- | --- | --- |
| B' | `n x n` | positive Laplacian, shuntless, ignores taps and phase shifts |
| B'' | `n x n` | includes shunts and taps, zeros phase shifts |
| `Y_bus` | `n x n` complex | keeps taps, phase shifts, and terminal admittance |
| `Re(Y_bus)` / `-Im(Y_bus)` | `n x n` real | real split for solvers that want real matrices |
| LACPF block | `2n x 2n` | `[[G, -B], [-B, -G]]`, indefinite flat start block |
| adjacency | `n x n` | undirected topology over in service branches |
| incidence and Laplacian | `n x m`, `n x n` | signed incidence, branch weights, grounded reference form |
| PTDF / LODF | dense sensitivity tables | grounded solve with explicit reference bus handling |
| DC OPF bundle | several matrices and vectors | incidence, weighted Laplacian, flow map, costs, limits, and loads |

Reference buses are a set. Builders that ground a matrix remove every reference
row and column, so each connected component needs a reference bus. The
single-reference helpers exist for bindings and simple cases; they return errors
when the case has none or more than one.

GridFM output is a Parquet dataset under `<case>/raw/` with `bus_data`,
`gen_data`, `branch_data`, and `y_bus_data`. A single parsed case writes one
scenario. A scenario batch row stacks snapshots that share the same element set
and uses the `scenario` column as the key.

GridFM read is the ML to classical return path. It recovers bus types, voltages,
limits, nodal load and shunt totals, generator dispatch and bounds, branch
parameters, and `base_mva`. It cannot recover original bus ids, per element
load and shunt granularity, piecewise and cubic costs, HVDC, or storage; those
losses are returned as warnings.

See [matrix conventions](../guides/matrices.html) and the GridFM notes in
[format fidelity](../guides/format-fidelity.html) for full formulas and limits.
