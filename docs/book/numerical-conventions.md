# Numerical Conventions

PowerIO follows MATPOWER and PowerModels.jl conventions unless a source format
requires an explicit conversion.

Key invariants:

- Branch impedance and `BR_B` are already per unit on `baseMVA`; do not divide
  them by `baseMVA` again.
- `tap == 0` means a line and is treated as an effective tap of `1`.
- B' ignores taps and phase shifts.
- B'' includes taps and shunts but zeros phase shifts.
- `Y_bus` keeps taps and phase shifts.
- Positive Laplacians use negative off diagonal entries and positive diagonal
  entries.
- Reference buses are a set. A solvable grounded matrix needs one reference per
  connected component.
- DC OPF cost maps `c2 p^2 + c1 p` to `q = 2c2`, `c = c1`; per unit costs scale
  with the system base.

The full convention table is maintained in
[format fidelity](../guides/format-fidelity.html). Matrix signs, taps, phase shifts,
reference buses, PTDF, LODF, and DC OPF details live in
[matrices](../guides/matrices.html).
