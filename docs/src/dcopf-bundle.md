# DC OPF bundle schema

`powerio dcopf <case>.m -o <out>` assembles a `DcOpfInstance` and writes
`<out>/<case>_dcopf/`. Rust callers pass an assembled instance to
`powerio_matrix::write_dcopf_bundle`. The directory contains Matrix
Market files and `dcopf_meta.json`.

## Conventions

- **Format.** Matrix Market. Matrices are `coordinate real`; square symmetric
  ones (`L`, `L_grounded`) use the `symmetric` header and store the lower
  triangle only. Vectors are `array real general`, one value per line.
- **Index base.** `.mtx` row/column indices are **1-based** (Matrix Market
  standard). `reference_buses` in the manifest are **0-based** dense bus indices.
- **Sign convention.** The DC bus susceptance matrix \\(L\\) uses the positive
  M-matrix form: stored nonzero off-diagonal entries are negative, diagonals are
  nonnegative, and \\(L_{ii} = \sum_j \lvert L_{ij} \rvert\\). An off-diagonal
  entry is \\(L_{ij} = -b_e\\) for the branch between \\(i\\) and \\(j\\), so a
  consumer recovers the branch susceptance as \\(-L_{ij} > 0\\).
- **Units.** `PerUnit` by default: power divided by `base_mva`, cost scaled so
  it is a function of per unit power:
  \\(q \leftarrow 2c_2 \cdot \mathrm{base}^2\\) and
  \\(c \leftarrow c_1 \cdot \mathrm{base}\\). `Native` keeps MW / native cost.
  The choice is recorded in the manifest.
- **Generator costs.** The default DC OPF export policy is `require`: an
  in-service generator without cost data is an error. Use `--missing-gen-cost`
  to explicitly fill missing rows for feasibility tests.
- **Reference buses.** `reference_buses` in the manifest lists every grounded bus
  as a 0-based dense index. Each in-service island needs at least one reference.
  If several references lie in one island, the bundle fixes all of those voltage
  angles to zero; it is not a participation factor slack model.
- **DC convention.** `b.mtx` holds \\(b_e\\), positive for an inductive branch,
  the coefficient on \\(\theta_f - \theta_t\\). The complete flow is
  \\(f_e = b_e(\theta_f - \theta_t) - b_e\delta_e\\), where \\(\delta_e\\) is
  the phase shift in `shift.mtx`. `SeriesSusceptance` by default:
  \\(b_e = x/(r^2 + x^2)\\) plus the phase shift terms,
  with no tap scaling. `TapAdjustedReactance` uses \\(b_e = 1/(x \tau)\\) plus `p_shift`.
  `ReactanceOnly` (\\(b_e = 1/x\\), taps and shifts ignored) stays: it is the
  textbook DC linearization, and reproducing a published result needs it exactly
  as written. Recorded in the manifest.

## Matrices

| file | shape | what |
|------|-------|------|
| `A.mtx` | \\(n \times m\\) | signed incidence matrix; column \\(e\\) has \\(+1\\) at from-bus, \\(-1\\) at to-bus |
| `L.mtx` | \\(n \times n\\) | DC bus susceptance matrix \\(L = A \operatorname{diag}(b) A^\mathsf{T}\\); with positive branch weights, its rank is \\(n-c\\) for \\(c\\) connected components |
| `L_grounded.mtx` | \\((n-k) \times (n-k)\\) | \\(L\\) with \\(k\\) reference rows and columns removed; SPD when every island is grounded |
| `BAt.mtx` | \\(m \times n\\) | angle dependent flow map \\(B A^\mathsf{T}\\) over the bundle's positive weights; complete flow adds `flow_offset`. In PowerModels signs the same flow is \\(p_\text{branch} = -B_f\,v_a + b \odot \text{shift}\\) with negated susceptances |
| `Cg.mtx` | \\(n \times n_{\mathrm{gen}}\\) | generator-to-bus incidence, one \\(1\\) per column |

## Vectors

Bus-indexed (length \\(n\\)): `pd` (load), `gs` (shunt conductance, the constant
real power a shunt draws at one per unit voltage; a nodal balance subtracts it
beside `pd`), `q`/`c`/`c0` (cost diag/linear/constant),
`pmax`/`pmin`
(generation bounds), `e_r` (reference indicator: \\(1\\) at every reference bus, else \\(0\\)),
`p_shift` (phase shift injection; zero only under `ReactanceOnly`, which ignores
shifts, or when the case has no phase shifter), and `fixed_withdrawal`, equal to
`pd + gs + p_shift`.
Branch-indexed (length \\(m\\)): `b` (susceptances), `shift` (radians),
`flow_offset` (equal to `-b * shift` elementwise), `fmax` (thermal limits;
\\(0\\) means unlimited per MATPOWER), and the radian limits `angle_min` and
`angle_max`.
Generator space data
(length \\(n_{\mathrm{gen}}\\)): `q_gen`, `c_gen`, `c0_gen`, `pmax_gen`, and `pmin_gen`.

The bundle schema represents polynomial generator costs only. A preparation
with a piecewise linear generator cost returns a typed error instead of writing
zero polynomial coefficients. The in-memory generator space preparation
retains those breakpoints exactly.

The constant cost terms `c0`/`c0_gen` do not move the argmin; they exist so a
consumer reporting objective values reconstructs the full cost.

Generator space is canonical. The nodal `q`, `c`, `c0`, `pmax`, and `pmin`
files aggregate the generators at each bus. The bounds are the sum of the
generator bounds. The cost curves combine by the parallel rule
\\(q = 1 / \sum_i 1/q_i\\), which is the curve of the split that costs least, so
it agrees with generator space only while that split stays inside the bound of
each generator. A bus with one generator keeps that generator's curve.

## Manifest (`dcopf_meta.json`)

Schema `powerio.dcopf`, stamped with the writing release in `powerio_version`,
writes Matrix Market files plus structured metadata:

- `dimensions`: `n_buses`, `n_source_branches`, `n_branch_columns`,
  `n_generators`, `n_reference_buses`, and `n_grounded_buses`.
- `index_base`: `dense = 0` for manifest bus, branch, generator, and reference
  indices; `matrix_market = 1` for `.mtx` coordinates.
- `dc_convention`, `units`, `build_options`, and `zero_impedance`.
  `build_options` records both `skip_zero_impedance` and
  `synthesize_unrated_limits`. The zero impedance block records the skip flag,
  denominator rule, skipped count, and skipped source branch rows.
- `grounding`: reference buses, removed rows and columns, the grounded operator
  (`L_grounded`), and the reference selector (`e_r`).
- `operators[]`: one entry per emitted operator with `name`, `file`, `kind`,
  `rows`, `cols`, `index_space`, and `units`.

The legacy aliases `n`, `m`, `n_gen`, `reference_buses`, and `convention` remain
for current readers. `cost_policy`, `synthesized_gen_costs`,
`patched_gen_costs`, `files[]`, and `powerio_version` remain top level fields.

## Solving with it

The complete affine equations are
\\[
f = B A^\mathsf{T}\theta + \texttt{flow\_offset}
\\]
and
\\[
L\theta = C_g p_g - \texttt{fixed\_withdrawal}.
\\]
The grounded system is the one to factor: `L_grounded` is SPD when every island
has a reference. Drop all `reference_buses` entries from the right hand side,
solve the reduced system, and set each reference angle to \\(0\\). `e_r`
identifies the grounded buses without parsing the manifest. The full singular
\\(L\\) can be used instead when the right hand side sums to zero within each
connected component.

An interior point DC OPF solver builds reweighted bus Laplacians each Newton
step from the same `A` and `b` (only the edge weights change), so `A` is the
durable operator to hand over.
