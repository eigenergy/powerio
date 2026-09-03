# Migrating from 0.9

PowerIO 0.10 is the public beta of the 1.0 API. API corrections may land before 1.0.0 as downstream integrations exercise the new design.

0.10 makes `PioModule` the one runtime unit and `.pio.json`
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
  with the materialize instruction (`powerio package --materialize` in a 0.9 install).
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

## The DC susceptance sign reversed

0.9's `DcConvention::branch_susceptance` returned the positive Laplacian
edge weight in every variant, and [the 0.9 migration guide](migration-v0.9.md)
told PowerModels aligned consumers to negate it. 0.10 returns the
PowerModels value itself: negative for an inductive branch, the imaginary
part of the series admittance the selected formula models. Every public
surface carries the same sign — `dc_network_data` rows, the C ABI DC data,
and the matrix outputs — and the internal positive factor weight is the
separate `DcConvention::solver_edge_weight`, whose name states its role.

A consumer that negated the 0.9 value must stop negating. A consumer that
used the 0.9 value directly as a factor weight switches to
`solver_edge_weight`. The relation is pinned by
`series_susceptance_reduces_to_negated_one_over_x` in `powerio-tx` and by
the cross assembly conformance test
`public_preparation_formulates_the_complete_dc_opf` in `powerio-matrix`:
`build_dc_opf_preparation`'s `branches.b` is the elementwise negation of
`dc_network_data`'s `susceptance`, with an identical phase shift injection.

## The removed 0.9 surfaces

- Rust: `powerio::package` is gone; the lowering lives at
  `powerio::transform`, the geo layer at `powerio::dist_geo`, and the code
  registry at `powerio::codes`. The `Network` alias and the SCOPF projection
  (`parse_scopf_str`, its `IndexBase`, and the solver JSON document) are
  gone; GO Challenge 3 parses to a typed `AcScucInstance`.
- Python: `powerio.parse(source, from_, include_root=..., value_type=...)`
  replaces `parse_file`, `parse_str`, `parse_bytes`, and
  `read_pypsa_csv_folder` (`include_root`, omitted by default, widens the
  include acquisition boundary from the file's containing directory to the
  named ancestor, widening what the parse may read). `powerio.PioModule`
  replaces the `Package` class, `value_type` asserts the kind without
  changing the returned module, and `module.value` reads the typed value.
  `parse_scopf`, `to_dense` solver rows, the `Dense*` rows, and the 0.8
  renamed alias hooks are gone.
- C: the whole 0.9 surface is replaced, not extended. `pio_package_*`,
  `pio_scopf_*`, the network returning parse family with caller error
  buffers, the separate distribution parse pair, and the solver row Arrow
  tables (ids 6 to 14, 21, 22; the ids stay burned) are gone. One parse
  family returns module handles, typed accessors return network handles,
  and every failure is a structured `PioError`. The complete classified
  delta and porting table: [ABI history](abi-v6.md).
- CLI: `powerio module` writes the stored module (`--scenario` exports one
  scenario of a set), and every single case command reads a stored
  `.pio.json` directly.

## Julia

`parse_file(path)` is the ordinary call after `using PowerIO` and returns
`PioModule{T}` for the detected kind; `parse_bytes` covers memory and
stream input. The `value_type` keyword, the type marker parse forms, the
public `StoredModule`, and the `read_module`/`parse_module` family are
gone: read the typed value from `case.value`, assert a kind with an
ordinary `::PioModule{MulticonductorNetwork}` annotation, and read
findings as native `Diagnostic` records from `diagnostics(case)`.

## The C ABI

ABI 6: owned handles with `retain`/`release`, structured `PioError`
handles, one module surface, structured diagnostics, and the DC branch
data. See [ABI history and symbol replacement](abi-v6.md).

The 0.9 `pio_dist_capabilities_json` fidelity flags reported which optional
BMOPF tables that build's writer could express. The 0.10 writer expresses
all of them, and the report is gone: gate on the release version from
`pio_version` or `pio_build_info`, and on the BMOPF schema vintage from
`pio_schema_versions_json`, when behavior must be pinned per release.
