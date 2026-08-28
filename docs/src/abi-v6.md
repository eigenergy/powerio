# ABI history and symbol replacement

One row per break, append only; a row lands in the same change that increments the macro.

One row per break, append only. Add a row in the same change that increments the macro.

| ABI | breaking change |
|---|---|
| 1 | First versioned surface: opaque handles, typed extractors, JSON transport ([#54](https://github.com/eigenergy/powerio/pull/54)). |
| 2 | `pio_parse` → `pio_parse_file`, `pio_convert` → `pio_convert_file`, `pio_write_matpower` → `pio_to_matpower` with an `errbuf` ([#69](https://github.com/eigenergy/powerio/pull/69)). |
| 3 | `pio_case_free` → `pio_network_free`; `PioCase` → `PioNetwork`, an opaque typedef ([#77](https://github.com/eigenergy/powerio/pull/77)). |
| 4 | The naming grammar and the bus/node/branch vocabulary rule. Case formats take `pio_to_format`/`pio_parse_str`, directory formats take `pio_write_dir`/`pio_read_dir`, and the extractors, warning queries, reference bus and island queries and conversion entry points take fixed signatures. |
| 5 | Seven conversion signatures move to a warnings out-pointer, five extractors move to the star-lowered space, three `pio_acopf_*` symbols are removed, and six JSON documents change shape ([#323](https://github.com/eigenergy/powerio/pull/323)). [Guide](abi-v5.md). |
| 6 | The 1.0 handle model: every opaque handle gains a `retain`/`release` pair, new entry points return structured `PioError` handles, and the stored module (`pio_module_*`) and DC branch data (`pio_dc_data_*`) surfaces are added. The 0.9 package and SCOPF entry points (`pio_package_*`, `pio_scopf_*`) are removed; the surviving v5 network surface keeps its signatures. [Guide](abi-v6.md). |

The ABI 4 break worth remembering is the one that did not fail loudly: `pio_convert_file` kept its symbol, its arity and its parameter types while arguments 2 and 3 swapped from `(path, to, from)` to `(path, from, to)`. Every other ABI 4 change renamed a symbol or changed an arity, so a stale caller failed at link or load. That one linked and read the formats reversed. It is why the `pio_abi_version()` handshake is not optional, and why `scripts/capi-header-regen.sh` diffs the whole generated header rather than comparing symbol names.

A version gets a migration guide when it has a consumer to migrate. ABI 5 has one; the earlier bumps predate any binding outside this repository.

The ABI 4 break worth remembering is the one that did not fail loudly: `pio_convert_file` kept its symbol, its arity, and its parameter types while arguments 2 and 3 swapped from `(path, to, from)` to `(path, from, to)`. Every other ABI 4 change renamed a symbol or changed an arity, so a stale caller failed at link or load. That one linked and read the formats reversed. It is why the `pio_abi_version()` handshake is not optional, and why `scripts/capi-header-regen.sh` diffs the whole generated header rather than comparing symbol names.

## Migrating to ABI 6

ABI v6 is the 1.0 handle model. Every opaque handle is an independently
owned reference with a `retain`/`release` pair, failures cross as structured
`PioError` handles, and the stored module and DC branch data surfaces are
new. The surviving v5 entry points keep their signatures, but the 0.9
package and SCOPF surfaces are removed. The list below is the exact set of
symbols declared in the v0.9.0 header and absent from the v6 header
(`scripts/capi-removed-surface.sh` holds it to that set difference):

<!-- removed-c-surface:begin -->
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
`pio_scopf_instance_free`,
`pio_scopf_parse_str`,
`pio_scopf_to_json`,
`pio_scopf_to_json_with_index_base`
<!-- removed-c-surface:end -->

A v5 caller of the package group ports to the module surface: parse with
`pio_module_parse_file` / `pio_module_parse_str` / `pio_module_parse_bytes`,
take networks out with `pio_module_as_network` /
`pio_module_as_dist_network`, lower with `pio_module_lower_to_balanced`,
read findings with `pio_module_diagnostics_json`, and serialize with
`pio_module_write_json`. A v5 caller of the SCOPF group parses the same way
and reads the instance as JSON through `pio_module_inspect_json` or
`pio_module_write_json`; the module's release call replaces the freed
handle. A caller of the surviving network surface recompiles without
edits. `PIO_ABI_VERSION` is 6.

## The handle lifecycle

- `pio_*_retain` mints a new handle over the same immutable value;
  `pio_*_release` drops one handle; `release(NULL)` is a no-op.
- Releasing a parent never invalidates a retained child. A DC data result
  built from a module stays valid after the module's release.
- `pio_network_free` and `pio_dist_network_free` remain as the release
  spelling existing callers link; the `_release` names are the same
  operation.
- Concurrent immutable calls on one handle are allowed. Releasing a raw
  handle concurrently with a call on that same raw handle is caller error.

## Structured errors

New v6 entry points take a `PioError**` out parameter (NULL to ignore)
instead of a character buffer. On failure they return NULL (or false) and
store a handle whose `pio_error_code`, `pio_error_message`, and
`pio_error_diagnostics_json` spans stay valid until `pio_error_release`.
Panics never unwind across the boundary; they become `BIND.CAPI.PANIC`
errors.

## The stored module surface

`pio_module_read_json` reads stored `.pio.json` (version 1, or a released
0.9 package upgraded one way), `pio_module_parse_file`/`_parse_str` compile
any case family, `pio_module_write_json` writes the stored document, and
`pio_module_inspect_json`, `pio_module_state_inventory_json`,
`pio_module_export_state`, `pio_module_lowering_readiness_json`, and
`pio_module_lower_to_balanced` carry inspection, typed state selection, and
the explicit balanced lowering.

## DC branch data

`pio_dc_data_build` returns the owned DC branch data under a named
susceptance formula (`series_susceptance`, `tap_adjusted_reactance`,
`reactance_only`), with the signed incidence row endpoints
(`A[e, from] = +1`, `A[e, to] = -1`), the per row phase shift angles
(`pio_dc_data_shift`, radians), the phase shift bus injection
`p_shift = A' (b .* shift)`, stable module element IDs for every included
row and bus column, and omitted branches with IDs and reasons. Susceptance
carries the PowerModels sign, tables describe the analysis network after
three winding transformer expansion, and spans stay valid until the
handle's release. `pio_dc_data_fill_branch_flow` writes the complete
affine flow `p_branch = -b .* (va_from - va_to) + b .* shift` into the
caller's buffer, so `A' p_branch` matches the bus injection.
`pio_module_diagnostics_json` returns the module's findings as owned JSON.
The same values reach Rust as `powerio_tx::dc_network_data`, Python as
`BalancedNetwork.dc_data`, and Julia as `DcData`.
