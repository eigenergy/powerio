# DC OPF Bundle Schema

`powerio dcopf <case>.m -o <out>` (or `opf_pipeline::write_dcopf_bundle`) writes
`<out>/<case>_dcopf/`: a set of Matrix Market files plus `dcopf_meta.json`.
Everything is a pure function of the case. This documents what each file is and
the conventions a consumer (e.g. a C++ Laplacian solver) needs.

## Conventions

- **Format.** Matrix Market. Matrices are `coordinate real`; square symmetric
  ones (`L`, `L_grounded`) use the `symmetric` header and store the lower
  triangle only. Vectors are `array real general`, one value per line.
- **Index base.** `.mtx` row/column indices are **1-based** (Matrix Market
  standard). `reference_buses` in the manifest are **0-based** dense bus indices.
- **Sign convention.** The Laplacians are the **positive (M-matrix) form**:
  diagonal `> 0`, off-diagonal `< 0`, with `L[i,i] = Σ_j |L[i,j]|` for `L`. An
  off-diagonal entry is `L[i,j] = −b_e` for the branch between `i` and `j`, so a
  consumer recovers the edge weight as `−L[i,j] > 0`.
- **Units.** `PerUnit` by default: power divided by `base_mva`, cost scaled so
  it is a function of per unit power (`q ← 2c₂·base²`, `c ← c₁·base`). `Native`
  keeps MW / native cost. The choice is recorded in the manifest.
- **Reference buses.** `reference_buses` in the manifest lists every grounded bus
  as a 0-based dense index. Each in-service island needs at least one reference.
  If several references lie in one island, the bundle fixes all of those voltage
  angles to zero; it is not a participation factor slack model.
- **DC convention.** `PaperPure` by default (`b_e = 1/x`, taps and phase shifts
  ignored). `Matpower` uses `b_e = 1/(x·τ)` plus the phase shift injection
  `p_shift`. Recorded in the manifest.

## Matrices

| file | shape | what |
|------|-------|------|
| `A.mtx` | n×m | signed incidence; column `e` has `+1` at from-bus, `−1` at to-bus |
| `L.mtx` | n×n | DC Laplacian `L = A diag(b) Aᵀ`, singular (rank n−1), `1 ∈ ker L` |
| `L_grounded.mtx` | (n−k)×(n−k) | `L` with `k` reference rows and columns removed; SPD when every island is grounded |
| `BAt.mtx` | m×n | flow map `B Aᵀ` (`f = B Aᵀ θ`) |
| `Cg.mtx` | n×n_gen | generator→bus incidence, one `1` per column |

## Vectors

Bus-indexed (length n): `pd` (load), `q`/`c` (cost diag/linear), `pmax`/`pmin`
(generation bounds), `e_r` (reference indicator: `1` at every reference bus, else `0`),
`p_shift` (phase shift injection, all zero unless `Matpower` + shifters).
Branch-indexed (length m): `b` (susceptances), `fmax` (thermal limits; `0` means
unlimited per MATPOWER). Generator-space provenance (length n_gen): `q_gen`,
`c_gen`, `pmax_gen`, `pmin_gen`.

## Manifest (`dcopf_meta.json`)

`case_name, base_mva, n, m, n_gen, reference_buses` (0-based), `convention`,
`units`, `files[]`, `powerio_version`.

## Solving with it

The grounded system is the one to factor: `L_grounded` is SPD when every island
has a reference. For DC power flow `L θ = p` with net injection `p = g − d`, drop
all `reference_buses` entries from `p`, solve `L_grounded θ_red = p_red`, and set
each reference angle to `0`. `e_r` identifies the grounded buses without parsing
the manifest. The full singular `L` can be used instead with a consistent
zero-sum RHS.

The interior point DC OPF solver builds *reweighted* Laplacians each Newton step
from the same `A` and `b` (only the edge weights change), so `A` is the durable
operator to hand over.
