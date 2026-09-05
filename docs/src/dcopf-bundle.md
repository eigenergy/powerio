# DC OPF bundle

`powerio dcopf <case>.m -o <out>` assembles a `DcOpfInstance` and writes
`<out>/<case>_dcopf/`. Rust callers pass an assembled instance to
`powerio_matrix::emit_dcopf_bundle`. The directory contains Matrix
Market files and `dcopf_meta.json`.

The bundle writes the DC problem in the solver's positive form, where `b.mtx`
holds the positive susceptance magnitude of each branch and `A.mtx` is bus
by branch. The public calculations in [Matrices and graphs](matrices.md) use
the PowerModels form instead, where `calc_branch_susceptances` is negative for
an inductive branch and `calc_incidence_matrix` is branch by bus. So the
bundle's `b` is the negated public `b`, and `A.mtx` is the transposed public
`A`.

## Definitions

- **Format.** Matrix Market. Matrices are `coordinate real`; square symmetric
  ones (`L`, `L_grounded`) use the `symmetric` header and store the lower
  triangle only. Vectors are `array real general`, one value per line.
- **Index base.** `.mtx` row and column indices are 1-based, as Matrix Market
  requires. `reference_buses` in the manifest are 0-based dense bus indices.
- **Sign convention.** The DC bus susceptance matrix \\(L\\) uses the positive
  M-matrix form: stored nonzero off-diagonal entries are negative, diagonals are
  nonnegative, and \\(L_{ii} = \sum_j \lvert L_{ij} \rvert\\). An off-diagonal
  entry is \\(L_{ij} = -b_e\\) for the branch between \\(i\\) and \\(j\\), so a
  consumer recovers the branch susceptance as \\(-L_{ij} > 0\\).
- **Units.** The default `PerUnit` divides power by `base_mva` and rescales
  cost so it is a function of per unit power,
  \\(q \leftarrow 2c_2 \cdot \mathrm{base}^2\\) and
  \\(c \leftarrow c_1 \cdot \mathrm{base}\\). `Native` keeps MW and native
  cost. The manifest records which one you chose.
- **Generator costs.** The default export policy for costs is `require`, so
  an in service generator without cost data is an error. Pass
  `--missing-gen-cost` to fill the missing rows for a feasibility test.
- **Reference buses.** `reference_buses` in the manifest lists every grounded bus
  as a 0-based dense index. Each in service island needs at least one reference.
  If several references lie in one island, the bundle fixes all of those voltage
  angles to zero; it is not a participation factor slack model.
- **Branch susceptance formula.** `b.mtx` holds \\(b_e\\), positive for an inductive branch,
  the coefficient on \\(\theta_f - \theta_t\\). The complete flow is
  \\(f_e = b_e(\theta_f - \theta_t) - b_e\delta_e\\), where \\(\delta_e\\) is
  the phase shift in `shift.mtx`. The default, `SeriesSusceptance`, uses
  \\(b_e = x/(r^2 + x^2)\\) plus the phase shift terms,
  with no tap scaling. `TapAdjustedReactance` uses \\(b_e = 1/(x \tau)\\) plus `p_shift`.
  `ReactanceOnly` (\\(b_e = 1/x\\), taps and shifts ignored) is kept because it
  is the textbook DC linearization, and reproducing a published result needs it
  exactly as written. The manifest records the formula.

## Matrices

| file | shape | what |
|------|-------|------|
| `A.mtx` | \\(n \times m\\) | signed incidence matrix; column \\(e\\) has \\(+1\\) at from-bus, \\(-1\\) at to-bus |
| `L.mtx` | \\(n \times n\\) | DC bus susceptance matrix \\(L = A \operatorname{diag}(b) A^\mathsf{T}\\); with positive branch weights, its rank is \\(n-c\\) for \\(c\\) connected components |
| `L_grounded.mtx` | \\((n-k) \times (n-k)\\) | \\(L\\) with \\(k\\) reference rows and columns removed; SPD when every island is grounded |
| `BAt.mtx` | \\(m \times n\\) | branch flow matrix \\(B A^\mathsf{T}\\) over the bundle's positive susceptance magnitudes; complete flow adds `flow_offset`. In PowerModels signs the same flow is \\(p_\text{branch} = -B_f\,v_a + b \odot \text{shift}\\) with negated susceptances |
| `Cg.mtx` | \\(n \times n_{\mathrm{gen}}\\) | generator-to-bus incidence, one \\(1\\) per column |

## Vectors

The bus indexed vectors, of length \\(n\\), are `pd` (load), `gs` (shunt
conductance, the constant real power a shunt draws at one per unit voltage; a
nodal balance subtracts it beside `pd`), `q`, `c`, and `c0` (the diagonal,
linear, and constant cost terms), `pmax` and `pmin` (generation bounds),
`e_r` (reference indicator: \\(1\\) at every reference bus, else \\(0\\)),
`p_shift` (phase shift injection; zero only under `ReactanceOnly`, which ignores
shifts, or when the case has no phase shifter), and `fixed_withdrawal`, equal to
`pd + gs + p_shift`.

The branch indexed vectors, of length \\(m\\), are `b` (susceptances), `shift`
(radians), `flow_offset` (equal to `-b * shift` elementwise), `fmax` (thermal
limits; \\(0\\) means unlimited per MATPOWER), and the radian limits
`angle_min` and `angle_max`.

The generator space vectors, of length \\(n_{\mathrm{gen}}\\), are `q_gen`,
`c_gen`, `c0_gen`, `pmax_gen`, and `pmin_gen`.

The bundle schema only represents polynomial generator costs. If a generator
has a piecewise linear cost, preparation returns a typed error instead of
writing zero polynomial coefficients; the in memory generator space
preparation keeps those breakpoints exactly.

The constant cost terms `c0` and `c0_gen` do not move the argmin. They are
there so that a consumer reporting objective values can reconstruct the full
cost.

Generator space is canonical. The nodal `q`, `c`, `c0`, `pmax`, and `pmin`
files aggregate the generators at each bus: the bounds are the sums of the
generator bounds, and the cost curves combine by the parallel rule
\\(q = 1 / \sum_i 1/q_i\\), which is the curve of the split that costs least.
That aggregate agrees with generator space only while the cheapest split stays
inside the bound of each generator. A bus with one generator keeps that
generator's curve.

## Manifest (`dcopf_meta.json`)

The manifest has schema `powerio.dcopf` and is stamped with the writing
release in `powerio_version`. It describes the Matrix Market files with
structured metadata:

- `dimensions`: `n_buses`, `n_source_branches`, `n_branch_columns`,
  `n_generators`, `n_reference_buses`, and `n_grounded_buses`.
- `index_base`: `dense = 0` for manifest bus, branch, generator, and reference
  indices; `matrix_market = 1` for `.mtx` coordinates.
- `branch_susceptance_formula`, `units`, `build_options`, and `zero_impedance`.
  `build_options` records both `skip_zero_impedance` and
  `synthesize_unrated_limits`. The zero impedance block records the skip flag,
  denominator rule, skipped count, and skipped source branch rows.
- `grounding`: reference buses, removed rows and columns, the grounded operator
  (`L_grounded`), and the reference selector (`e_r`).
- `operators[]`: one entry per emitted operator with `name`, `file`, `kind`,
  `rows`, `cols`, `index_space`, and `units`.

`cost_policy`, `synthesized_gen_costs`, `patched_gen_costs`, `files[]`, and
`powerio_version` are top level fields.

## Solving with it

The complete affine equations are
\\[
f = B A^\mathsf{T}\theta + \texttt{flow\_offset}
\\]
and
\\[
L\theta = C_g p_g - \texttt{fixed\_withdrawal}.
\\]
Factor the grounded system, since `L_grounded` is SPD when every island has a
reference. Drop all `reference_buses` entries from the right hand side, solve
the reduced system, and set each reference angle to \\(0\\). `e_r` identifies
the grounded buses without parsing the manifest. You can use the full singular
\\(L\\) instead when the right hand side sums to zero within each connected
component.

An interior point DC OPF solver builds reweighted bus Laplacians at each Newton
step from the same `A` and `b` (only the edge weights change), so `A` is the
durable operator to hand over.
