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
it. PowerIO 0.11.1 writes `"schema": "pio-ir"` with integer `"version": 2`, and
`pio_schema_report` reports both; the producer record names the PowerIO
release separately. `pio_module_deserialize` refuses an unsupported schema
name or generation and reports what it found. The reader accepts generation 2
and the structural types implemented by the library. C ABI 7 has no module JSON aliases.

## PSS/E contingency analysis files

A `.con`, `.sub`, or `.mon` file parses through `pio_parse` like any other
source; `pio_value_contingency_set`, `pio_value_subsystem_set`, and
`pio_value_monitored_set` take the typed handle from the module value, and
`pio_contingency_set_parse` reads a contingency file straight from an acquired
source with `pio_contingency_set_diagnostics` for the reader's notes.
`pio_emit` writes each one back under `psse-con`, `psse-sub`, or `psse-mon`.

A handle from `pio_value_contingency_set`, `pio_value_subsystem_set`, or
`pio_value_monitored_set` borrows the module value rather than copying it, and
holds the module owner alive, so it stays readable after the module handle is
released. A handle from `pio_contingency_set_parse` or
`pio_contingency_set_expand` owns its set instead.

`pio_contingency_set_resolve` binds every case to a balanced network and
returns a `PioContingencyResolution`. It reports rather than refuses: a case
naming an element the network does not hold is counted unresolved and keeps
its reason. `pio_contingency_set_expand` turns the automatic specifications
into explicit cases against a network and a subsystem set.

```c
PioContingencySet *cases = pio_contingency_set_parse(source, &error);
PioContingencyResolution *resolution =
    pio_contingency_set_resolve(cases, network, &error);

printf("%zu of %zu cases bound\n",
       pio_contingency_resolution_resolved_count(resolution),
       pio_contingency_resolution_case_count(resolution));

for (size_t i = 0; i < pio_contingency_resolution_case_count(resolution); i++) {
    size_t bound =
        pio_contingency_resolution_case_component_count(resolution, i);
    for (size_t j = 0; j < bound; j++) {
        PioContingencyComponentView component;
        if (!pio_contingency_resolution_case_component(resolution, i, j,
                                                       &component, &error)) {
            break;
        }
        /* component.id.component_type names the table component.row
           indexes; component.id.local_id is the element's own identity,
           and its len is 0 when the network states none for that row */
    }

    size_t missing =
        pio_contingency_resolution_case_unresolved_count(resolution, i);
    for (size_t j = 0; j < missing; j++) {
        PioStringView statement =
            pio_contingency_resolution_case_unresolved_action(resolution, i, j,
                                                              &error);
        PioStringView reason =
            pio_contingency_resolution_case_unresolved_reason(resolution, i, j,
                                                             &error);
        /* statement is the `.con` line of the action; reason is a fixed name
           such as "no_such_branch", the same name Python reports */
        printf("%.*s: %.*s\n", (int)statement.len, statement.data,
               (int)reason.len, reason.data);
    }
}

PioDiagnostics *notes = NULL;
PioContingencySet *expanded =
    pio_contingency_set_expand(cases, network, subsystems, &notes, &error);
PioString *text = pio_contingency_set_to_con(expanded, &error);
PioStringView view = pio_string_view(text);
printf("%.*s", (int)view.len, view.data);

pio_string_release(text);
pio_diagnostics_release(notes);
pio_contingency_set_release(expanded);
pio_contingency_resolution_release(resolution);
pio_contingency_set_release(cases);
```

The set accessors are `pio_contingency_set_case_count`,
`pio_contingency_set_case_name`, `pio_contingency_set_diagnostics`, and
`pio_contingency_set_to_con`; `pio_subsystem_set_count`,
`pio_subsystem_set_name`, and `pio_subsystem_set_to_sub`; and
`pio_monitored_set_statement_count` and `pio_monitored_set_to_mon`. Each of
the three writers returns owned text, read with `pio_string_view` and released
with `pio_string_release`, which is how an expanded set reaches a file. The
resolution accessors are `pio_contingency_resolution_case_count`,
`pio_contingency_resolution_resolved_count`,
`pio_contingency_resolution_unresolved_count`,
`pio_contingency_resolution_unrecognized_statement_count`,
`pio_contingency_resolution_case_name`,
`pio_contingency_resolution_case_is_resolved`,
`pio_contingency_resolution_case_component_count`,
`pio_contingency_resolution_case_component`,
`pio_contingency_resolution_case_unresolved_count`,
`pio_contingency_resolution_case_unresolved_reason`, and
`pio_contingency_resolution_case_unresolved_action`. Each handle has its own
`pio_contingency_set_retain` and `pio_contingency_set_release`,
`pio_subsystem_set_retain` and `pio_subsystem_set_release`,
`pio_monitored_set_retain` and `pio_monitored_set_release`, and
`pio_contingency_resolution_retain` and `pio_contingency_resolution_release`.

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

`pio_calc_dc_operators` builds the same operators once and returns a
`PioDcOperators` handle whose axes are named: `pio_dc_operators_bus_ids` maps
each bus row to its source bus id, `pio_dc_operators_branch_rows` maps each
branch row to its position in the branch table (three winding transformer
windings follow the branches), and `pio_dc_operators_branch_identity` states
the stable identity of one branch row. Out of service branches and self loops
have no row. The `skip_zero_impedance` argument drops a zero impedance branch
and lists it under `pio_dc_operators_skipped_branch_rows` instead of failing
the build with `BUILD.OPERATOR.ZERO_IMPEDANCE`. The eight calculations run
over the handle as `pio_dc_operators_incidence_matrix`,
`pio_dc_operators_branch_susceptances`,
`pio_dc_operators_bus_susceptance_matrix`,
`pio_dc_operators_branch_flow_matrix`,
`pio_dc_operators_branch_phase_shift_injection`,
`pio_dc_operators_bus_phase_shift_injection`,
`pio_dc_operators_branch_flow_dc`, and `pio_dc_operators_bus_injection_dc`;
release the handle with `pio_dc_operators_release`.
