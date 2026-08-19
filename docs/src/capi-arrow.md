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
| 21 | `solver_gen_cost` | `record_batch` | `solver_gen` | |
| 22 | `solver_gen_cost_coeff` | `coeff_list` | `solver_gen_cost_coeff` | |

Matrix schema metadata carries:

```text
powerio.table
powerio.version
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

## Generator cost

Table 21 is dense over solver generators: one row per `solver_gen` row, in the same order, with `index` mirroring `solver_gen.index`. It carries `model`, `startup`, `shutdown`, `ncost`, `coeff_count` and `coeff_offset`. Table 22 is the flattened coefficient vector, one row per stored value, as `(gen_index, position, value)`, grouped by `gen_index` ascending and ordered by `position` inside a group.

A generator's coefficients are rows `[coeff_offset, coeff_offset + coeff_count)` of table 22. Offsets are nondecreasing over the rows that have one, and the slices partition table 22 exactly. `model == 0` means the generator carries no cost row at all; such a row has `ncost == 0`, `coeff_count == 0` and the `coeff_offset` sentinel `-1`, which is also what an otherwise present cost row with no stored values gets.

`position` is read against `model`:

- `model == 2` (polynomial): position `i` of a `coeff_count` long slice is the coefficient of `p^(coeff_count - 1 - i)`, in currency per hour per (per unit active power) to that power.
- `model == 1` (piecewise linear): even positions are active power at a breakpoint in per unit, odd positions are the curve value there in currency per hour. Breakpoint `j` is positions `2j` and `2j + 1`. A trailing even position with no partner comes from a malformed source row and is exported rather than hidden.
- any other `model`: the values are whatever the source stated, unscaled. A consumer that does not recognize the model reads `model` and stops.

`ncost` is the count the source declared, `coeff_count` is the number of values actually stored. They agree for every case a reader produces from a MATPOWER or PowerModels source; they can disagree for a network built through `GenCost::with_ncost` or read from a `.pio.json` payload. A consumer that wants to reject a malformed curve compares the two; a consumer that wants powerio's own reading uses `min(ncost, coeff_count)`.

Values are per unit on the network's MVA base, the same basis as the other solver tables. Both tables carry that base in schema metadata, so converting currency per hour per per unit power to currency per MWh needs no second call:

```text
powerio.table
powerio.version
powerio.format        # "record_batch" for 21, "coeff_list" for 22
powerio.row_axis
powerio.base_mva
powerio.group_axis    # table 22 only, "solver_gen"
powerio.group_column  # table 22 only, "gen_index"
```

`solver_bus` (table 6) gained `area` and `zone` at the end of its column list in the same release, so an area or zone keyed consumer does not have to fall back to the JSON snapshot for two integers per bus. `solver_storage` (table 13) gained `charge_efficiency` and `discharge_efficiency` the same way, so a storage constraint that prices conversion losses reads them from the table it already decodes.

## Arrow catalog JSON

`pio_arrow_catalog_json(errbuf, errlen)` returns compact JSON that lets a
binding discover the Arrow tables compiled into the C library. It describes
the build: `available` tells whether this library was built with the needed
features, and a particular network's row counts play no part.

Shape:

```json
{
  "powerio_version": "0.9.0",
  "producer": "powerio-capi",
  "tables": [
    {
      "id": 17,
      "name": "bprime",
      "powerio_version": "0.9.0",
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

`PioNetwork` Arrow tables describe a network, a generic matrix projection, or
the cost curves the network states. They do not carry cost policy or a solver
formulation: what to do with a generator that has no cost row, and which curve
shapes a formulation accepts, stay outside the Arrow surface. `powerio-prob`
owns complete problem instances. The C `prob` feature currently exposes a
matrix free SCOPF instance through a JSON document. DC OPF
instances and bundles have no C entry points.
