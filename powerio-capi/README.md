# powerio-capi

The C ABI parses power system sources into module handles, exposes typed accessors and structured diagnostics over them, converts and writes supported formats, and serves numeric tables and Arrow exports for matrix assembly. It is how every non-Rust consumer except the PyO3 Python package reaches powerio; PowerIO.jl resolves these symbols from a pinned artifact.

The header is [`include/powerio.h`](https://github.com/eigenergy/powerio/blob/main/powerio-capi/include/powerio.h). It is generated and checked in; its comment block states the naming, ownership, and error grammars and is authoritative where prose disagrees.

## Build

```
cargo build -p powerio-capi --release --features arrow,matrix,gridfm,dist,prob
# → target/release/libpowerio_capi.{so,dylib}  (cdylib)
#   target/release/libpowerio_capi.a            (staticlib)
```

Regenerate the header after changing the ABI:

```
cbindgen --config powerio-capi/cbindgen.toml --crate powerio-capi \
  --output powerio-capi/include/powerio.h
```

The test suite pins the checked-in header shape. Run the core and optional surfaces before changing `powerio.h` or an exported `pio_*` function:

```
cargo test -p powerio-capi --no-default-features
cargo test -p powerio-capi --features arrow
cargo test -p powerio-capi --features dist
cargo test -p powerio-capi --features arrow,matrix,gridfm,dist,prob
bash scripts/ci-clippy.sh capi-release
scripts/capi-header-parity.sh
scripts/capi-smoke.sh
```

## C

```c
#include "powerio.h"
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    /* Check the handshake before anything else. */
    if (pio_abi_version() != PIO_ABI_VERSION) return 1;

    PioError *error = NULL;
    PioModule *module = pio_parse_file("case9.m", NULL, &error);
    if (!module) {
        fprintf(stderr, "%s: %s\n", pio_error_code(error), pio_error_message(error));
        pio_error_release(error);
        return 1;
    }

    /* The detected value kind is a stable string. */
    printf("kind %s\n", pio_module_kind(module));

    /* The reader's findings, as structured records. */
    PioDiagnostics *findings = pio_module_diagnostics(module, &error);
    for (size_t i = 0; i < pio_diagnostics_len(findings); i++)
        printf("%s %s: %s\n", pio_diagnostic_severity(findings, i),
               pio_diagnostic_code(findings, i), pio_diagnostic_message(findings, i));
    pio_diagnostics_release(findings);

    /* The typed value: an independently owned network handle. */
    PioBalancedNetwork *net = pio_module_balanced_network(module, &error);
    size_t n = pio_balanced_network_n_buses(net);
    size_t m = pio_balanced_network_n_branches(net);
    printf("%zu buses, %zu branches, baseMVA %g\n",
           n, m, pio_balanced_network_base_mva(net));

    /* Pull the branch table to build a susceptance matrix yourself. The
     * extractors write up to cap entries and return the total, so a short
     * buffer is detectable; NULL out (or cap 0) is the count query. */
    int64_t *from = malloc(m * sizeof *from), *to = malloc(m * sizeof *to);
    double  *x    = malloc(m * sizeof *x);
    pio_balanced_network_branches(net, from, to, NULL, x, NULL, NULL, NULL, NULL, m);
    /* ... assemble L = A diag(1/x) A^T from (from, to, x) ... */

    /* One write operation on the module: the same format echoes byte exact,
     * a cross format write reports its losses through the out handle. */
    char *matpower = pio_module_write_str(module, "matpower", NULL, &error);
    if (matpower) { /* ... byte exact MATPOWER text ... */ pio_string_release(matpower); }

    PioDiagnostics *losses = NULL;
    char *json = pio_module_write_str(module, "powermodels-json", &losses, &error);
    if (json) { /* ... PowerModels JSON text ... */ pio_string_release(json); }
    pio_diagnostics_release(losses);

    /* One call conversion without keeping a handle. */
    char *raw = pio_convert_file("case9.m", NULL, "psse", NULL, NULL, &error);
    if (raw) { /* ... PSS/E text ... */ pio_string_release(raw); }

    free(from); free(to); free(x);
    pio_balanced_network_release(net);
    pio_module_release(module);
    return 0;
}
```

Every handle type has a `retain`/`release` pair; `release(NULL)` is a no-op, and releasing the module never invalidates the network handle taken from it. Every fallible entry point takes a `PioError **` out parameter (NULL to ignore) and catches panics at the boundary.

## Julia

Use [PowerIO.jl](https://github.com/eigenergy/PowerIO.jl): `parse_file(path)` returns the typed `PioModule{T}` over this ABI, with the ownership rules held by finalizers and borrowed views that root their owner. The raw `ccall` shape it builds on is one symbol per operation, resolved with `dlsym` after the `pio_abi_version()` handshake.

## Balanced model JSON

For consumers that want the whole case rather than the dense table slices, `pio_balanced_network_to_json` and `pio_balanced_network_from_json` carry the entire balanced network: buses, loads, shunts, branches, generators, storage, HVDC, and extras. It is a network serialization rather than a case format; a bare `.json` holding it classifies as `model-json`.

## The stored module

`pio_module_read_json` and `pio_module_write_json` carry the versioned `.pio.json` document for any value kind, including the one way upgrade of released 0.9 documents. State selection over series and scenario sets (`pio_module_export_state`, `pio_module_state_inventory_json`), the explicit balanced lowering (`pio_module_lower_to_balanced`), and DC branch data (`pio_dc_data_*`, feature `prob`) operate on the same handles.

## Optional features

Probe at runtime with `pio_has_feature("arrow" | "matrix" | "gridfm" | "dist" | "prob")`; each symbol's own header guard states what it needs, and a build without a feature exports nothing for it. `pio_build_info` reports the build's version, ABI integer, features, foreign schema versions, and stable token sets in one JSON document.
