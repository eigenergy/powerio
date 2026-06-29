# Generator Cost Policy

PSS/E `.raw` files do not carry generator cost curves in powerio. When a PSS/E
case is converted to MATPOWER, powerio writes `mpc.gen` and omits `mpc.gencost`;
it does not invent zero costs. The conversion warning says the cost block was
omitted.

Use an explicit policy when a target workflow needs costs:

```sh
powerio convert case.raw --from psse --to matpower --missing-gen-cost zero -o case.m
powerio dcopf case.m -o out --missing-gen-cost quadratic --default-gen-cost 0.01,2.0,0.0
powerio gridfm case.raw --from psse -o out --missing-gen-cost zero
```

`--gen-cost-csv` overrides costs by generator row before the missing-cost policy
runs. The CSV header is:

```csv
gen_index,bus,c2,c1,c0,startup,shutdown
```

`gen_index` is zero based in the current generator table. `bus` must match the
generator's bus id, which catches stale tables after reordering. `startup` and
`shutdown` are optional and default to zero.

Policies:

- `preserve`: leave missing costs absent. This is the default for conversion and
  GridFM export.
- `require`: fail if an in-service generator has no cost. This is the default for
  DC OPF bundle export.
- `zero`: fill missing rows with a MATPOWER polynomial cost `[0, 0, 0]`.
- `quadratic`: fill missing rows with `--default-gen-cost C2,C1,C0`.

GridFM stores `cp0/cp1/cp2` columns. Missing or unsupported costs still write
zero cost columns, but the manifest separates `missing_cost_gens`,
`unsupported_cost_gens`, `zeroed_cost_gens`, and `synthesized_gen_costs`.

OPFDataset is not a supported powerio format yet. Add a schema or sample fixture
before implementing an adapter; if its schema requires cost columns, it should use
the same policy rather than silently treating missing costs as real zeros.
