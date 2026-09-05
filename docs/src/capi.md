# C ABI

`powerio-capi` exports ABI 7 through `powerio-capi/include/powerio.h`. The
header is generated from the Rust declarations and checked in. Regenerate it
with:

```sh
cbindgen --config powerio-capi/cbindgen.toml --crate powerio-capi \
  --output powerio-capi/include/powerio.h
```

`scripts/capi-header-parity.sh` compares the exported symbols with the header
in every CI feature job, and `scripts/capi-header-regen.sh` regenerates the
header with cbindgen and diffs it once. Do not edit the header by hand.

ABI 7 is the only C API in PowerIO 0.11; symbols from ABI 4, 5, and 6 are
not exported and have no aliases. Compare `pio_abi_version()` with
`PIO_ABI_VERSION` before you use the library.

The exported symbol set is fixed. The `gridfm` cargo feature adds GridFM
Parquet parsing and emission behind the same entry points, and the `arrow`,
`matrix`, `dist`, and `prob` feature names are still accepted by the build but
gate nothing. `pio_schema_report` returns a JSON document with the release
(`powerio_version`), the ABI (`abi`), the PowerIO IR schema name and
generation (`powerio_ir` with `schema` and `version`), the BMOPF schema
version, the compiled features, and the diagnostic namespaces and error
categories.

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

Use `pio_source_from_memory` for text or binary bytes you already hold in
memory; both source constructors feed the same `pio_parse`.
`pio_geo_layer_parse` reads a geographic layer straight from text, with no
source object, for callers that have the layer document in memory.

`pio_module_value` returns an owner rooted value handle, and the exact typed
accessors return owner rooted views without serializing or copying the module
value. Releasing a module does not invalidate a child you have retained.
Every opaque handle has matching `retain` and `release` functions, and
`release(NULL)` is a no-op.

Structural type names have replaced ordinal kind integers. Check the type
with `pio_value_type_name` or `pio_value_is_type`, then call the exact typed
accessor for the type you handle.

## Diagnostics and errors

Fallible functions take one `PioError **` output and signal failure with a
null return or a documented failure value. Inspect `pio_error_code`,
`pio_error_message`, and `pio_error_diagnostics`, and branch on the stable
code rather than the message text. If you pass a null error output the error
is discarded.

Every string and buffer comes with an explicit length. `PioStringView`,
`PioByteView`, `PioSizeView`, and `PioF64View` borrow their data from an owning
handle and need not end in NUL.

## Emit and serialize

`pio_emit` writes a grid exchange format. With a memory destination the
artifact bytes stay in the returned `PioEmitResult`; with a path destination
the artifacts are written to disk and the result holds the same list of
artifacts and the same diagnostics.

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

`pio_module_serialize` writes PowerIO IR and `pio_module_deserialize` reads
it. The IR header is `"schema": "pio-ir"` with integer `"version": 2`, and
`pio_schema_report` reports both; the producer record names the PowerIO
release separately. `pio_module_deserialize` refuses an unsupported schema
name or generation and tells you what it found. ABI 7 has no reader for
earlier generations and no module JSON aliases.

## Collections, updates, and calculations

Time series and scenario set handles give you the length and owner rooted
access to each element. Positions are zero based in C, and scenario sets can
also be looked up by scenario ID.

Typed update constructors produce `PioOperatingPointUpdate`,
`PioNetworkUpdate`, and `PioCalculationUpdate`. `pio_apply_updates` validates
the whole batch before it applies anything and returns a `PioUpdateReport`
listing the exact component IDs and fields it changed, and whether energized
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

Sparse matrices come back as owned CSR arrays and vectors as owned `double`
arrays. The C API has no public DC data bundle.
