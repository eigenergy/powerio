# Migrating to ABI 6

ABI v6 is the 1.0 handle model. Every opaque handle is an independently
owned reference with a `retain`/`release` pair, failures cross as structured
`PioError` handles, and the stored module and DC branch data surfaces are
new. The v5 entry points are unchanged, so a v5 caller recompiles against
the v6 header without edits; `PIO_ABI_VERSION` is 6.

## The handle lifecycle

- `pio_*_retain` mints a new handle over the same immutable value;
  `pio_*_release` drops one handle; `release(NULL)` is a no-op.
- Releasing a parent never invalidates a retained child. A DC data result
  built from a module stays valid after the module's release.
- `pio_network_free`, `pio_dist_network_free`, and `pio_scopf_instance_free`
  remain as the release spelling existing callers link; the `_release` names
  are the same operation.
- The 0.9 package handle (`pio_package_*`) stays single owner deliberately:
  its API mutates in place, and the module handle supersedes it.
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
(`A[e, from] = +1`, `A[e, to] = -1`), the phase shift bus injection, stable
module element IDs for every included row and bus column, and omitted
branches with IDs and reasons. Spans stay valid until the handle's release.
`pio_dc_data_fill_branch_flow` converts sign while filling the caller's
buffer. The same values reach Rust as `powerio_tx::dc_network_data`, Python
as `BalancedNetwork.dc_data`, and Julia as `DcData`.
