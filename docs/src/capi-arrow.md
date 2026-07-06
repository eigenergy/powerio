# C ABI Arrow Policy

This page records the C ABI Arrow rules while PowerIO prepares for v1. It does
not describe a v1 release. The next releases are expected to be v0.6.3 and
v0.7.0 before v1.

The C ABI stays handle based. Parsed transmission cases use `PioNetwork`,
distribution cases use `PioDistNetwork`, and `.pio.json` documents use
`PioPackage`. Callers get rich model transport through JSON, small copied
arrays through dense extractors, and bulk typed tables through the Arrow C Data
Interface.

## Arrow tables

Arrow table ids are append only. Existing ids keep their meaning and column
order. Matrix tables added axis metadata without changing their triplet columns:

| id | table | format | row axis | col axis |
| --- | --- | --- | --- | --- |
| 15 | `ybus` | `coo` | `matrix_bus` | `matrix_bus` |
| 16 | `incidence` | `coo` | `matrix_bus` | `matrix_branch` |
| 17 | `bprime` | `coo` | `matrix_bus` | `matrix_bus` |
| 18 | `bdoubleprime` | `coo` | `matrix_bus` | `matrix_bus` |
| 19 | `matrix_bus` | `axis_map` | `matrix_bus` | |
| 20 | `matrix_branch` | `axis_map` | `matrix_branch` | |

Matrix schema metadata carries:

```text
powerio.table
powerio.schema_version
powerio.format
powerio.row_axis
powerio.col_axis
powerio.row_count
powerio.col_count
powerio.index_space   # legacy alias, still "solver_bus" for bus indexed matrices
```

`matrix_bus` gives bindings a dense matrix row and column map without inferring
from `solver_bus`. It includes the dense index, source bus id, source row,
reference flag, and component label. `matrix_branch` gives incidence column
meaning: dense incidence column, source branch row, from bus id, and to bus id.
Branches that do not contribute an incidence column, such as self-loops or
skipped zero reactance rows, are not on this axis.

## Arrow catalog JSON

`pio_arrow_catalog_json(errbuf, errlen)` returns compact JSON that lets a
binding discover the Arrow tables compiled into the C library. It is build
based, not case based: `available` tells whether this library was built with the
needed features, not whether a particular network has nonempty rows.

Shape:

```json
{
  "schema_version": "1",
  "producer": "powerio-capi",
  "tables": [
    {
      "id": 17,
      "name": "bprime",
      "schema_version": "1",
      "format": "coo",
      "feature_requirements": ["arrow", "matrix"],
      "available": true,
      "row_axis": "matrix_bus",
      "col_axis": "matrix_bus",
      "units": {
        "value": "per_unit",
        "matrix_index_base": "zero"
      },
      "columns": [
        {"name": "row_index", "type": "int64", "nullable": false},
        {"name": "col_index", "type": "int64", "nullable": false},
        {"name": "value", "type": "float64", "nullable": false}
      ]
    }
  ]
}
```

Bindings should read the catalog before assuming optional ids exist. The table
ids are still exposed as C macros for callers that compile against
`powerio.h`.

## Binding policy

Julia keeps `copy=true` as the default for Arrow tables. That copies primitive
columns into owned Julia vectors and releases the producer Arrow structs
immediately. `copy=false` remains opt in and keeps the Arrow owner alive so zero
copy views cannot outlive their buffers.

The Julia binding is not a generic Arrow engine. It decodes the primitive table
shapes PowerIO exports today. New Arrow tables need binding tests for copied and
zero copy lifetime behavior before they are considered stable.

## Deferred solver cost tables

Normalized solver cost Arrow tables are not part of this pass. The current
normalized generator cost model has variable width polynomial coefficients and
policy choices for absent costs. Exporting that cleanly needs a separate schema
and binding decoder work, so it should not ride on the axis map change.

## Future Derived Product Handle

DC OPF bundles and sensitivity products should use a separate opaque handle
later, not new `PioNetwork` table ids in this pass.

Sketch:

```c
typedef struct PioDerivedProduct PioDerivedProduct;

PioDerivedProduct *pio_derive_product(const PioNetwork *net,
                                      const char *kind,
                                      const char *options_json,
                                      char *errbuf,
                                      size_t errlen);

char *pio_derived_catalog_json(const PioDerivedProduct *product,
                               char *errbuf,
                               size_t errlen);

int32_t pio_derived_to_arrow(const PioDerivedProduct *product,
                             int32_t table,
                             struct ArrowArray *out_array,
                             struct ArrowSchema *out_schema,
                             char *errbuf,
                             size_t errlen);

char *pio_derived_to_json(const PioDerivedProduct *product,
                          const char *name,
                          char *errbuf,
                          size_t errlen);

void pio_derived_product_free(PioDerivedProduct *product);
```

Rules:

- `kind` selects a product family such as `dcopf` or `sensitivities`.
- `options_json` carries choices that would otherwise grow the C ABI: DC
  convention, grounding, units, missing cost policy, and selected sensitivity
  columns.
- The derived handle owns the computed product. Arrow exports move their own
  buffers to the caller and remain valid after `pio_derived_product_free`.
- The product catalog owns its table id space. It must not reuse or renumber the
  `PioNetwork` Arrow ids.
- Product table metadata uses the same keys as matrix tables:
  `powerio.schema_version`, `powerio.format`, `powerio.row_axis`, and
  `powerio.col_axis`.

Schema sketches:

| product | table | format | row axis | col axis | columns |
| --- | --- | --- | --- | --- | --- |
| `dcopf` | `dcopf_bus` | `axis_map` | `dcopf_bus` | | `index`, `matrix_bus`, `bus_id`, `is_reference`, `is_grounded` |
| `dcopf` | `dcopf_branch` | `axis_map` | `dcopf_branch` | | `index`, `matrix_branch`, `source_row`, `from_bus_id`, `to_bus_id` |
| `dcopf` | `dcopf_incidence` | `coo` | `dcopf_bus` | `dcopf_branch` | `row_index`, `col_index`, `value` |
| `dcopf` | `dcopf_laplacian` | `coo` | `dcopf_bus` | `dcopf_bus` | `row_index`, `col_index`, `value` |
| `dcopf` | `dcopf_grounded_laplacian` | `coo` | `dcopf_grounded_bus` | `dcopf_grounded_bus` | `row_index`, `col_index`, `value` |
| `dcopf` | `dcopf_cost` | `dense` | `dcopf_bus` | | `bus_index`, `q`, `c`, `pmin`, `pmax`, `pd` |
| `sensitivities` | `ptdf` | `dense_matrix` | `matrix_branch` | `matrix_bus` | `row_index`, `col_index`, `value` |
| `sensitivities` | `lodf` | `dense_matrix` | `matrix_branch` | `matrix_branch` | `row_index`, `col_index`, `value` |

Benchmark gates for that future work:

- Rust: product construction time, Arrow export time, and memory peak against
  the existing `powerio-matrix` direct builders.
- Julia: copied and zero copy table lifetime tests, plus sparse constructor time
  driven only by catalog axes.
- Cross tool: parse plus matrix construction in `benchmarks/bench_julia.jl`
  stays alongside PowerModels.jl and ExaPowerIO.jl, with product handle timings
  added as separate rows.
- Size: release dylib sizes for core, `arrow,matrix`, and all features before
  and after enabling the product feature.
