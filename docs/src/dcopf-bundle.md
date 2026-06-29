# DC OPF Bundle Schema (experimental)

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
  diagonal \(> 0\), off-diagonal \(< 0\), with
  \(L_{ii} = \sum_j \lvert L_{ij} \rvert\) for \(L\). An off-diagonal entry is
  \(L_{ij} = -b_e\) for the branch between \(i\) and \(j\), so a consumer
  recovers the edge weight as \(-L_{ij} > 0\).
- **Units.** `PerUnit` by default: power divided by `base_mva`, cost scaled so
  it is a function of per unit power:
  \(q \leftarrow 2c_2 \cdot \mathrm{base}^2\) and
  \(c \leftarrow c_1 \cdot \mathrm{base}\). `Native` keeps MW / native cost.
  The choice is recorded in the manifest.
- **Generator costs.** The default DC OPF export policy is `require`: an
  in-service generator without cost data is an error. Use `--missing-gen-cost`
  to explicitly fill missing rows for feasibility tests.
- **Reference buses.** `reference_buses` in the manifest lists every grounded bus
  as a 0-based dense index. Each in-service island needs at least one reference.
  If several references lie in one island, the bundle fixes all of those voltage
  angles to zero; it is not a participation factor slack model.
- **DC convention.** `PaperPure` by default (\(b_e = 1/x\), taps and phase shifts
  ignored). `Matpower` uses \(b_e = 1/(x \tau)\) plus the phase shift injection
  `p_shift`. Recorded in the manifest.

## Matrices

| file | shape | what |
|------|-------|------|
| `A.mtx` | \(n \times m\) | signed incidence; column \(e\) has \(+1\) at from-bus, \(-1\) at to-bus |
| `L.mtx` | \(n \times n\) | generic Laplacian \(L = A \operatorname{diag}(b) A^\mathsf{T}\), singular with \(\operatorname{rank}(L) = n - 1\), \(\mathbf{1} \in \ker L\) |
| `L_grounded.mtx` | \((n-k) \times (n-k)\) | \(L\) with \(k\) reference rows and columns removed; SPD when every island is grounded |
| `BAt.mtx` | \(m \times n\) | flow map \(B A^\mathsf{T}\), where \(f = B A^\mathsf{T} \theta\) |
| `Cg.mtx` | \(n \times n_{\mathrm{gen}}\) | generator-to-bus incidence, one \(1\) per column |

## Vectors

Bus-indexed (length \(n\)): `pd` (load), `q`/`c` (cost diag/linear), `pmax`/`pmin`
(generation bounds), `e_r` (reference indicator: \(1\) at every reference bus, else \(0\)),
`p_shift` (phase shift injection, all zero unless `Matpower` + shifters).
Branch-indexed (length \(m\)): `b` (susceptances), `fmax` (thermal limits; \(0\) means
unlimited per MATPOWER). Generator-space provenance (length \(n_{\mathrm{gen}}\)): `q_gen`,
`c_gen`, `pmax_gen`, `pmin_gen`.

## Manifest (`dcopf_meta.json`)

`case_name, base_mva, n, m, n_gen, reference_buses` (0-based), `convention`,
`units`, `cost_policy`, `synthesized_gen_costs`, `patched_gen_costs`, `files[]`,
`powerio_version`.

## Solving with it

The grounded system is the one to factor: `L_grounded` is SPD when every island
has a reference. For DC power flow \(L\theta = p\) with net injection
\(p = g - d\), drop all `reference_buses` entries from \(p\), solve
\(L_{\mathrm{grounded}}\theta_{\mathrm{red}} = p_{\mathrm{red}}\), and set each
reference angle to \(0\). `e_r` identifies the grounded buses without parsing the
manifest. The full singular \(L\) can be used instead with a consistent zero-sum
RHS.

An interior point DC OPF solver builds *reweighted* Laplacians each Newton step
from the same `A` and `b` (only the edge weights change), so `A` is the durable
operator to hand over.
