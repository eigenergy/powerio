# C ABI Arrow policy

The C ABI stays handle based. Parsed transmission cases use `PioNetwork`,
distribution cases use `PioDistNetwork`, and `.pio.json` documents use
`PioPackage`. Callers get full model transport through JSON, small copied
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

The Julia binding decodes the primitive table shapes listed in the catalog. A
new Arrow table requires binding tests for copied and zero copy lifetime
behavior.

## Problem data boundary

`PioNetwork` Arrow tables describe a network or a generic matrix projection.
They do not carry solver cost policy or a solver formulation. `powerio-prob`
owns complete problem instances. The C `prob` feature currently exposes a
matrix free SCOPF instance through a versioned JSON wire document. DC OPF
instances and bundles have no C entry points.
