# Migrating to 1.0

PowerIO 1.0 makes `PioModule<PioValue>` the one runtime unit and `.pio.json`
version 1 its stored form. Everything a released 0.9 package carried either
upgrades one way on read or is refused with a directed instruction.

## The stored document

- The header is `"schema": "powerio.module"`, `"version": 1`. The reader
  dispatches on it before exact typed decoding; unknown semantic fields and
  unknown versions are refused with their stated identity.
- One typed `value` per document: the network kinds (`balanced_network`,
  `multiconductor_network`), the collections (`balanced_network_time_series`,
  `balanced_operating_point_time_series`,
  `multiconductor_operating_point_time_series`,
  `balanced_network_scenario_set`), and the seven problem instances and
  seven solutions (`dc_pf_instance` through `ac_scuc_solution`). There is no
  per value version.
- The common records are `producer`, `sources`, `source_map`, `diagnostics`,
  `history`, and namespaced `extensions`, omitted when empty. Nonfinite
  floats spell `"Infinity"`, `"-Infinity"`, `"NaN"`; `null` is refused.
- A released 0.9 package reads through the same entry point and upgrades one
  way: legacy operating points become the primary typed operating point time
  series, legacy element paths translate into the value's own pointer
  grammar, and the upgrade is recorded as history plus a
  `READ.MODULE.UPGRADED` diagnostic. A nonempty legacy `study` is refused
  with the materialize instruction (`powerio package --materialize` in 0.9).
  The pre 0.9 lineage is refused and must be regenerated.

## Typed state selection

`state_inventory`, `select_state`, and `export_state` replace the JSON
materialization path: selection returns the existing typed item with no
clone and no serialization, and export is the separate explicit operation
that produces an independent static module with the selection in its
history. Refusals are coded `REQUEST.STATE.*` diagnostics.

## The explicit balanced lowering

`lower_module_to_balanced` accepts a multiconductor module and returns a
balanced module with the records carried over and the pass's findings and
assumptions appended. The pass now lowers a supported three phase two
winding `wye_delta`/`delta_wye` transformer and merges an unrated identity
closed switch (recording `merged_buses` and `removed_switches`); a rated
closed switch, a cross phase switch, and a merge conflict refuse with their
own codes, and nothing ever invents an epsilon impedance.

## The removed 0.9 surfaces

- Rust: `powerio::package` is gone; the lowering lives at
  `powerio::transform`, the geo layer at `powerio::dist_geo`, and the code
  registry at `powerio::codes`. The `Network` alias and the SCOPF projection
  (`parse_scopf_str`, its `IndexBase`, and the solver JSON document) are
  gone; GO Challenge 3 parses to a typed `AcScucInstance`.
- Python: `powerio.parse(source, from_, include_root=..., value_type=...)`
  replaces `parse_file`, `parse_str`, `parse_bytes`, and
  `read_pypsa_csv_folder`; `powerio.StoredModule` replaces the `Package`
  class; `parse_scopf`, `to_dense`, the `Dense*` rows, and the 0.8 renamed
  alias hooks are gone.
- C: `pio_package_*`, `pio_scopf_*`, and the 0.9 solver row Arrow tables
  (ids 6 to 14, 21, 22) are gone; the ids stay burned. The module surface
  and `pio_dc_data_*` replace them.
- CLI: `powerio package` writes the stored module (`--scenario` exports one
  scenario of a set), and every single case command reads a stored
  `.pio.json` directly.

## The C ABI

ABI v6: owned handles with `retain`/`release`, structured `PioError`
handles, the stored module surface, and the DC branch data. See
[Migrating to ABI 6](abi-v6.md).
