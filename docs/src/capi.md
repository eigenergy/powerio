# C ABI

`powerio-capi` exports ABI 7 through `powerio-capi/include/powerio.h`. The
header is generated from the Rust declarations and checked in. Regenerate it
with `scripts/capi-header-regen.sh`; do not edit it by hand.

ABI 7 is the only C surface in PowerIO 1.0. Symbols from ABI 4, 5, and 6 are
not aliases and are not exported. A caller must compare `pio_abi_version()`
with `PIO_ABI_VERSION` before using the library.

## Parse and inspect a module

```c
PioError *error = NULL;
PioSource *source = pio_source_open("case9.m", 7, &error);
PioModule *module = pio_parse(source, NULL, 0, &error);
PioValueHandle *value = pio_module_value(module);

if (!pio_value_is_type(value, "powerio.BalancedNetwork", 23)) {
    /* handle an unexpected PowerIO type */
}

PioBalancedNetwork *network = pio_value_balanced_network(value, &error);
size_t buses = pio_balanced_network_bus_count(network);

PioDiagnostics *diagnostics = pio_module_diagnostics(module);
for (size_t i = 0; i < pio_diagnostics_len(diagnostics); i++) {
    PioStringView code = pio_diagnostic_code(diagnostics, i);
    /* code.data has code.len bytes and is not NUL terminated */
}

pio_diagnostics_release(diagnostics);
pio_balanced_network_release(network);
pio_value_release(value);
pio_module_release(module);
pio_source_release(source);
```

Use `pio_source_from_memory` for text or binary bytes already in memory. Both
source constructors feed the same `pio_parse` operation.

`pio_module_value` returns an owner rooted value handle. Exact typed accessors
return owner rooted views without serializing or copying the module value.
Releasing a module does not invalidate a retained child. Every opaque handle
has matching `retain` and `release` functions; `release(NULL)` is a no-op.

Structural type names replace ordinal kind integers. Use
`pio_value_type_name`, `pio_value_is_type`, and the exact typed accessor for the
type the caller handles.

## Diagnostics and errors

Fallible functions take one `PioError **` output. A null return or documented
failure value indicates failure. Inspect `pio_error_code`,
`pio_error_message`, and `pio_error_diagnostics`; branch on the stable code,
not the message text. Passing a null error output discards the error.

All strings and buffers carry explicit lengths. `PioStringView`,
`PioByteView`, `PioSizeView`, and `PioF64View` borrow their data from an owning
handle. They need not end in NUL.

## Emit and serialize

`pio_emit` produces a grid exchange representation. A memory destination keeps
artifact bytes in the returned `PioEmitResult`; a path destination writes the
artifact and returns the same inventory and diagnostics.

```c
PioDestination *destination = pio_destination_memory("case", 4, &error);
PioEmitResult *result = pio_emit(module, "matpower", 8, destination, &error);

for (size_t i = 0; i < pio_emit_result_artifact_count(result); i++) {
    PioArtifact *artifact = pio_emit_result_artifact(result, i, &error);
    PioStringView name = pio_artifact_name(artifact);
    PioByteView bytes = pio_artifact_bytes(artifact);
    /* consume name and bytes before releasing artifact */
    pio_artifact_release(artifact);
}

pio_emit_result_release(result);
pio_destination_release(destination);
```

`pio_module_serialize` writes PowerIO IR. `pio_module_deserialize` reads it.
The IR header is `"schema": "powerio.module"` and `"version": 1`; ABI 7 does
not expose prerelease document readers or module JSON aliases.

## Collections, updates, and calculations

Time series and scenario set handles expose length and owner rooted element
access. Positions are zero based in C. Scenario sets also support lookup by
scenario ID.

Typed update constructors produce `PioOperatingPointUpdate`,
`PioNetworkUpdate`, and `PioCalculationUpdate`. `pio_apply_updates` validates
the complete batch before applying it and returns a `PioUpdateReport` with the
exact component IDs and fields changed. The report states whether energized
connectivity changed.

Named matrix and vector functions expose the public DC calculations directly:

```text
pio_calc_incidence_matrix
pio_calc_branch_susceptances
pio_calc_bus_susceptance_matrix
pio_calc_branch_flow_matrix
pio_calc_branch_phase_shift_injection
pio_calc_bus_phase_shift_injection
pio_calc_branch_flow_dc
pio_calc_bus_injection_dc
```

Sparse matrices use owned CSR arrays. Vectors use owned `double` arrays. The C
surface has no public DC data bundle.
