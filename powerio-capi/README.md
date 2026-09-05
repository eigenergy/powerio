# PowerIO C ABI

ABI 7 exposes PowerIO modules, typed electrical values, diagnostics, grid
exchange formats, PowerIO IR, updates, and DC calculations to C, C++, Julia,
and other FFI consumers. [PowerIO.jl](https://github.com/eigenergy/PowerIO.jl)
is the supported Julia binding.

The checked in header is `include/powerio.h`. Regenerate it after changing a
public `pio_*` function:

```sh
cbindgen --config powerio-capi/cbindgen.toml --crate powerio-capi \
  --output powerio-capi/include/powerio.h
```

Build and check the ABI with:

```sh
cargo build -p powerio-capi --release
cargo test -p powerio-capi
scripts/capi-header-parity.sh
scripts/capi-smoke.sh
```

ABI 7 exports one fixed symbol set. The `gridfm` cargo feature adds GridFM
Parquet support behind the same entry points, and the other feature names the
release build passes do not gate anything. `pio_schema_report` tells you what
a library was built with.

## Parse, inspect, and emit

```c
#include "powerio.h"
#include <stdio.h>
#include <string.h>

int main(void) {
    if (pio_abi_version() != PIO_ABI_VERSION) return 1;

    PioError *error = NULL;
    const char *path = "case9.m";
    PioSource *source = pio_source_open(path, strlen(path), &error);
    PioModule *module = pio_parse(source, NULL, 0, &error);
    pio_source_release(source);
    if (!module) {
        PioStringView message = pio_error_message(error);
        fprintf(stderr, "%.*s\n", (int)message.len, message.data);
        pio_error_release(error);
        return 1;
    }

    PioValueHandle *value = pio_module_value(module);
    PioStringView type = pio_value_type_name(value);
    printf("%.*s\n", (int)type.len, type.data);

    PioBalancedNetwork *network =
        pio_value_balanced_network(value, &error);
    printf("%zu buses, %zu branches\n",
           pio_balanced_network_bus_count(network),
           pio_balanced_network_branch_count(network));

    PioDestination *destination =
        pio_destination_memory("case9.raw", 9, &error);
    PioEmitResult *result =
        pio_emit(module, "psse", 4, destination, &error);
    PioArtifact *artifact = pio_emit_result_artifact(result, 0, &error);
    PioByteView bytes = pio_artifact_bytes(artifact);
    /* bytes.data remains valid until artifact is released. */

    pio_artifact_release(artifact);
    pio_emit_result_release(result);
    pio_destination_release(destination);
    pio_balanced_network_release(network);
    pio_value_release(value);
    pio_module_release(module);
    return 0;
}
```

For text or binary content already in memory, build the `PioSource` with
`pio_source_from_memory` instead. `pio_parse` is the grid exchange parser;
`pio_geo_layer_parse` reads a geographic layer from text in memory. For DOE
GO Challenge 3, a directory containing the problem file parses to an
`AcScucInstance`, and if you add the matching solution file to that directory
the same call parses an `AcScucSolution`.

`pio_emit` handles every grid exchange output. A memory destination returns
artifact bytes; a path destination writes a file or directory and returns the
list of artifacts it wrote. PowerIO IR has its own pair,
`pio_module_serialize` and `pio_module_deserialize`.

## Values and ownership

`pio_module_value` returns an owner rooted `PioValueHandle`.
`pio_value_type_name` returns a canonical structural name such as
`powerio.BalancedNetwork`, `powerio.DcOpfInstance`, or
`powerio.TimeSeries<powerio.MulticonductorNetwork>`. Use
`pio_value_is_type` for an exact predicate and then call the matching typed
accessor.

The typed accessors cover balanced and multiconductor networks, operating
points, PF/OPF/SCUC instances and solutions, time series, and scenario sets.
They neither serialize nor clone the value, and a child handle keeps its
module owner alive, so it stays valid even after you release the original
module handle.

Every opaque handle has `retain` and `release` functions, and releasing
`NULL` does nothing. Borrowed string, byte, index, and floating point views
stay valid until their documented owner is released or mutated.

## Diagnostics and errors

Diagnostics are stored on the `PioModule` and read with
`pio_module_diagnostics`; emission diagnostics belong to the `PioEmitResult`.
Both use `PioDiagnostics` and the `pio_diagnostic_*` accessors.

Every fallible function takes a `PioError **`. Pass `NULL` only when you
really do not want to know why it failed. Branch on `pio_error_code`; the
rendered message is for people to read.

## Typed updates

Construct a stable `PioComponentId`, a replacement value with an explicit
unit, and one typed update. Wrap operating point and network updates as
`PioCalculationUpdate` and pass the complete array to `pio_apply_updates`.
PowerIO validates the whole batch before it changes the module.

`PioUpdateReport` lists the exact component identity, field, and optional
terminal that changed. `pio_update_report_connectivity_changed` is true only
when electrical connectivity changed. If borrowed value handles exist, the
module detaches by copy on write, and those handles keep referring to the
value from before the edit.

## DC calculations

The public calculations return generic CSR matrix and vector handles:

- `pio_calc_incidence_matrix`
- `pio_calc_branch_susceptances`
- `pio_calc_bus_susceptance_matrix`
- `pio_calc_branch_flow_matrix`
- `pio_calc_branch_phase_shift_injection`
- `pio_calc_bus_phase_shift_injection`
- `pio_calc_branch_flow_dc`
- `pio_calc_bus_injection_dc`

The branch susceptance formula is named explicitly as
`series_susceptance`, `tap_adjusted_reactance`, or `reactance_only`.
