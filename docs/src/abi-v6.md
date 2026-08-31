# ABI history and symbol replacement

One row per break, append only; a row lands in the same change that increments the macro.

| ABI | breaking change |
|---|---|
| 1 | First versioned surface: opaque handles, typed extractors, JSON transport ([#54](https://github.com/eigenergy/powerio/pull/54)). |
| 2 | `pio_parse` → `pio_parse_file`, `pio_convert` → `pio_convert_file`, `pio_write_matpower` → `pio_to_matpower` with an `errbuf` ([#69](https://github.com/eigenergy/powerio/pull/69)). |
| 3 | `pio_case_free` → `pio_network_free`; `PioCase` → `PioNetwork`, an opaque typedef ([#77](https://github.com/eigenergy/powerio/pull/77)). |
| 4 | The naming grammar and the bus/node/branch vocabulary rule. Case formats take `pio_to_format`/`pio_parse_str`, directory formats take `pio_write_dir`/`pio_read_dir`, and the extractors, warning queries, reference bus and island queries and conversion entry points take fixed signatures. |
| 5 | Seven conversion signatures move to a warnings out-pointer, five extractors move to the star-lowered space, three `pio_acopf_*` symbols are removed, and six JSON documents change shape ([#323](https://github.com/eigenergy/powerio/pull/323)). [Guide](abi-v5.md). |
| 6 | The 1.0 handle model: every opaque handle gains a `retain`/`release` pair, new entry points return structured `PioError` handles, and the stored module (`pio_module_*`) and `PioDcData` array owner (`pio_dc_data_*`) surfaces are added. The 0.9 package and SCOPF entry points (`pio_package_*`, `pio_scopf_*`) are removed, and the v5 network surface is withdrawn and re-exposed under the `pio_balanced_network_*` prefix. [Guide](abi-v6.md). |

The ABI 4 break worth remembering is the one that did not fail loudly: `pio_convert_file` kept its symbol, its arity and its parameter types while arguments 2 and 3 swapped from `(path, to, from)` to `(path, from, to)`. Every other ABI 4 change renamed a symbol or changed an arity, so a stale caller failed at link or load. That one linked and read the formats reversed. It is why the `pio_abi_version()` handshake is not optional, and why `scripts/capi-header-regen.sh` diffs the whole generated header rather than comparing symbol names.

A version gets a migration guide when it has a consumer to migrate. ABI 5 has one; the earlier bumps predate any binding outside this repository.

## Migrating to ABI 6

ABI 6 is the replacement surface, not v5 plus a second API. One parse
family returns module handles, typed accessors return independently owned
network handles named for their value, every fallible entry point reports
through a structured `PioError`, diagnostics cross as structured row
handles, and every handle type carries a `retain`/`release` pair. Of the
83 symbols the v0.9.0 header declared, 6 survive unchanged
(`pio_abi_version`, `pio_version`, `pio_has_feature`, `pio_build_info`,
`pio_schema_versions_json`, `pio_classify_str`), 7 keep their names with
new structured error signatures (`pio_arrow_catalog_json`, `pio_convert_file`, `pio_convert_str`, `pio_geo_parse`, `pio_parse_bytes`, `pio_parse_file`, `pio_parse_str`),
and the rest are withdrawn or renamed. `scripts/abi-delta.py` derives this
classification mechanically and gates the ABI number on it. The list below
is the exact set of symbols declared in the v0.9.0 header and absent from
the v6 header (`scripts/capi-removed-surface.sh` holds it to that set
difference):

<!-- removed-c-surface:begin -->
`pio_base_mva`,
`pio_branch_charging`,
`pio_branches`,
`pio_bus_demand`,
`pio_bus_ids`,
`pio_bus_shunt`,
`pio_dist_abi_version`,
`pio_dist_capabilities_json`,
`pio_dist_convert_file`,
`pio_dist_convert_str`,
`pio_dist_from_json`,
`pio_dist_geo_apply`,
`pio_dist_geo_extract`,
`pio_dist_graph_json`,
`pio_dist_network_free`,
`pio_dist_parse_file`,
`pio_dist_parse_str`,
`pio_dist_summary_json`,
`pio_dist_to_format`,
`pio_dist_to_json`,
`pio_dist_warnings`,
`pio_from_json`,
`pio_gens`,
`pio_geo_apply`,
`pio_geo_extract`,
`pio_is_radial`,
`pio_matrix_available`,
`pio_n_branches`,
`pio_n_buses`,
`pio_n_gens`,
`pio_n_islands`,
`pio_n_switches`,
`pio_network_free`,
`pio_network_name`,
`pio_normalize`,
`pio_package_diagnostics_json`,
`pio_package_free`,
`pio_package_from_balanced_network`,
`pio_package_from_multiconductor_network`,
`pio_package_lower_multiconductor_to_balanced`,
`pio_package_materialize_operating_point`,
`pio_package_materialize_study_commit`,
`pio_package_multiconductor_to_balanced_preflight_json`,
`pio_package_operating_points_json`,
`pio_package_parse_file`,
`pio_package_parse_str`,
`pio_package_set_operating_points`,
`pio_package_study_json`,
`pio_package_to_balanced_network`,
`pio_package_to_json`,
`pio_package_to_multiconductor_network`,
`pio_package_validate`,
`pio_package_validation_json`,
`pio_read_dir`,
`pio_ref_bus_index`,
`pio_ref_bus_indices`,
`pio_scenario_ids`,
`pio_scopf_instance_free`,
`pio_scopf_parse_str`,
`pio_scopf_to_json`,
`pio_scopf_to_json_with_index_base`,
`pio_source_format`,
`pio_string_free`,
`pio_summary_json`,
`pio_switches`,
`pio_to_arrow`,
`pio_to_format`,
`pio_to_json`,
`pio_warnings`,
`pio_write_dir`
<!-- removed-c-surface:end -->

How a v5 caller ports, group by group:

| v5 | v6 |
|---|---|
| `pio_parse_file`/`_str`/`_bytes` (network out, errbuf) | the same names return `PioModule *` with a `PioError **`; `pio_module_balanced_network` takes the typed network out |
| `pio_dist_parse_file`/`_str` | the one parse family plus `pio_module_multiconductor_network` |
| `pio_package_*` | the module surface: parse, `pio_module_emit_*` with format `pio-json`, `pio_module_diagnostics`, `pio_module_export_state`, `pio_module_to_balanced` |
| `pio_scopf_*` | parse (an `ac_scuc_instance` module) plus `pio_module_inspect_json` / `pio_module_emit_*` |
| `pio_n_buses`, `pio_bus_ids`, the extractors, `pio_network_retain`/`_release`/`_free`, `pio_normalize`, `pio_to_json`/`pio_from_json`, `pio_to_arrow`, geo | the same operations under the `pio_balanced_network_` prefix, structured errors, no `free` verb |
| `pio_dist_summary_json`, `pio_dist_to_json`, `pio_dist_from_json`, `pio_dist_graph_json`, dist geo | the `pio_multiconductor_network_` prefix; graph projection is `pio_multiconductor_network_to_graph_json` with the noun form retained for compatibility |
| `pio_to_format`, `pio_write_dir`, `pio_dist_to_format` | `pio_module_emit_string` / `pio_module_emit_file` (wrap a bare network with `pio_balanced_network_to_module` / `pio_multiconductor_network_to_module`; the released `module_of_*` and `write` spellings remain aliases) |
| `pio_warnings`, `pio_dist_warnings` | `pio_module_diagnostics` structured rows |
| `pio_read_dir`, `pio_scenario_ids` | parse the dataset directory (a scenario set module), `pio_module_list_states_json`, `pio_module_export_state` |
| `pio_dist_abi_version`, `pio_dist_capabilities_json`, `pio_matrix_available`, `PIO_ERRBUF_MIN`, `PIO_DIST_ABI_VERSION` | one handshake plus `pio_has_feature` and `pio_build_info` |
| `pio_string_free` | `pio_string_release` |

`PIO_ABI_VERSION` is 6.

## The handle lifecycle

- `pio_*_retain` mints a new handle over the same immutable value;
  `pio_*_release` drops one handle; `release(NULL)` is a no-op.
- Releasing a parent never invalidates a retained child. A `PioDcData` array
  owner built from a module stays valid after the module's release.
- Concurrent immutable calls on one handle are allowed. Releasing a raw
  handle concurrently with a call on that same raw handle is caller error.

## Structured errors

Every fallible entry point takes a `PioError**` out parameter (NULL to
ignore) instead of a character buffer. On failure they return NULL (or false) and
store a handle whose `pio_error_code`, `pio_error_message`, and
`pio_error_diagnostics` rows stay valid until `pio_error_release`.
Panics never unwind across the boundary; they become `BIND.CAPI.PANIC`
errors.

## The stored module surface

`pio_parse_file` reads stored `.pio.json` (version 1, or a released 0.9
document upgraded one way) and every other case family.
`pio_module_emit_string`/`pio_module_emit_file` with format `pio-json` emit the
stored document. `pio_module_read_json` and `pio_module_write_json` remain ABI
conveniences for callers that already hold that document in memory, and
`pio_module_inspect_json`, `pio_module_list_states_json`,
`pio_module_export_state`, `pio_module_to_balanced_report_json`, and
`pio_module_to_balanced` carry inspection, typed state selection, and the
explicit balanced transformation. The released `lowering` and `lower` symbols
remain ABI 6 compatibility aliases. The released
`pio_module_state_inventory_json` spelling remains an alias for
`pio_module_list_states_json`.

The additive `pio_balanced_network_to_normalized`, both
`*_to_geo_layer_json` conversions, and both `*_to_module` wrappers apply the
receiver-first `to_*` transformation vocabulary. The verb-led
`*_apply_geo_layer` functions apply a sidecar to a new handle. Their released
`normalize`, `geo_extract`, `geo_apply`, and `module_of_*` spellings remain ABI
6 compatibility aliases.

`pio_module_inspect_json` keeps its released `operations` array byte-for-byte
compatible at the field level. Its additive `preferred_operations` and
`compatibility_operations` arrays separate the concise 1.0 path from the ABI
6 names retained for existing callers.

## Existing `PioDcData` handle

`pio_dc_data_build` returns an owned handle containing branch arrays under a
named susceptance formula (`series_susceptance`, `tap_adjusted_reactance`,
`reactance_only`): the signed incidence row endpoints
(`A[e, from] = +1`, `A[e, to] = -1`), the per row phase shift angles
(`pio_dc_data_shift`, radians), the phase shift bus injection
`p_shift = A' (b .* shift)`, stable module element IDs for every included
row and bus column, and omitted branches with IDs and reasons. Susceptance
carries the PowerModels sign, tables describe the analysis network after
three winding transformer expansion, and spans stay valid until the
handle's release. `pio_dc_data_calc_branch_flow` calculates the complete
affine flow `p_branch = -b .* (va_from - va_to) + b .* shift` into the
caller's buffer, so `A' p_branch` matches the bus injection.
`pio_module_diagnostics_json` returns the module's findings as owned JSON.

The handle exists so these C arrays have one retain/release owner. It is an ABI
transport object, not a power system term or the ordinary matrix surface. Rust
uses `DcOperators::calc_*`; Python and Julia expose direct
`calc_incidence_matrix`, `calc_bus_susceptance_matrix`,
`calc_branch_susceptance_matrix`, `calc_phase_shift_injection`, and
`calc_branch_flow_dc` operations. Their released `dc_data`, `DcData`,
`DcNetworkData`, and `dc_network_data` entries are removed in 1.0.
The released `pio_dc_data_fill_branch_flow_checked` and unchecked
`pio_dc_data_fill_branch_flow` spellings forward to the calculation.
