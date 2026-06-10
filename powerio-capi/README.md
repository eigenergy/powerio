# powerio-capi

A C ABI over `powerio`: parse any supported power system case format, query it,
convert it, and pull out the numeric tables a solver needs to assemble matrices.
Any language with a C foreign function interface can call it.

The header is
[`include/powerio.h`](https://github.com/eigenergy/powerio/blob/main/powerio-capi/include/powerio.h).

## Build

```
cargo build -p powerio-capi --release
# → target/release/libpowerio_capi.{so,dylib}  (cdylib)
#   target/release/libpowerio_capi.a            (staticlib)
```

Regenerate the header after changing the ABI:

```
cbindgen --config powerio-capi/cbindgen.toml --crate powerio-capi \
  --output powerio-capi/include/powerio.h
```

## C

```c
#include "powerio.h"
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    char err[256];
    PioNetwork *c = pio_parse_file("case9.m", NULL, err, sizeof err);
    if (!c) { fprintf(stderr, "parse: %s\n", err); return 1; }

    size_t n = pio_n_buses(c), m = pio_n_branches(c);
    printf("%zu buses, %zu branches, baseMVA %g\n", n, m, pio_base_mva(c));

    /* Pull the branch table to build a susceptance matrix yourself. */
    int64_t *from = malloc(m * sizeof *from), *to = malloc(m * sizeof *to);
    double  *x    = malloc(m * sizeof *x);
    pio_branches(c, from, to, NULL, x, NULL, NULL, NULL, NULL);
    /* ... assemble B' from (from, to, 1/x) ... */

    char *matpower = pio_to_matpower(c, err, sizeof err);
    if (matpower) { /* ... use MATPOWER text ... */ pio_string_free(matpower); }

    char warn[256];
    char *json = pio_to_format(c, "powermodels-json", warn, sizeof warn, err, sizeof err);
    if (json) { /* ... use PowerModels JSON text ... */ pio_string_free(json); }

    char *raw = pio_convert_file("case9.m", "psse", NULL, NULL, 0, err, sizeof err);
    if (raw) { /* ... use PSS/E text ... */ pio_string_free(raw); }

    free(from); free(to); free(x);
    pio_network_free(c);
    return 0;
}
```

## Julia (`ccall`)

For a typed Julia API, use [PowerIO.jl](https://github.com/eigenergy/PowerIO.jl),
which wraps this ABI (`set_library!`, `parse_file`, `parse_str`, `convert_file`,
and the `to_*` transforms). The raw `ccall` below is the low-level reference it
builds on.

```julia
const LIB = "libpowerio_capi"  # on the load path

function parse_file(path)
    err = zeros(UInt8, 256)
    h = ccall((:pio_parse_file, LIB), Ptr{Cvoid},
              (Cstring, Ptr{Cvoid}, Ptr{UInt8}, Csize_t),
              path, C_NULL, err, length(err))
    h == C_NULL && error(unsafe_string(pointer(err)))
    h
end

h = parse_file("case9.m")
n = ccall((:pio_n_buses, LIB), Csize_t, (Ptr{Cvoid},), h)
m = ccall((:pio_n_branches, LIB), Csize_t, (Ptr{Cvoid},), h)

from = Vector{Int64}(undef, m); to = Vector{Int64}(undef, m)
x = Vector{Float64}(undef, m)
ccall((:pio_branches, LIB), Cvoid,
      (Ptr{Cvoid}, Ptr{Int64}, Ptr{Int64}, Ptr{Float64}, Ptr{Float64},
       Ptr{Float64}, Ptr{Float64}, Ptr{Float64}, Ptr{UInt8}),
      h, from, to, C_NULL, x, C_NULL, C_NULL, C_NULL, C_NULL)
# build your matrices from (from, to, x), then:
ccall((:pio_network_free, LIB), Cvoid, (Ptr{Cvoid},), h)
```

## JSON transport

For consumers that want the whole case rather than the dense table slices,
`pio_to_json` serializes the entire `Network` (buses, loads, shunts, branches,
generators, storage, HVDC, and extras) to a string, and `pio_from_json` rebuilds
a handle from it. This is the transport the Julia package consumes: one call
instead of stitching the ~dozen table extractors together. The retained source
text is not part of the JSON, so a `from_json` handle reformats on write rather
than echoing a byte-exact original.

## API names

The release ABI uses the same verb taxonomy as the Rust, Python, and Julia APIs:

- `pio_parse_file` and `pio_parse_str` turn files or text into handles.
- `pio_to_format`, `pio_to_matpower`, `pio_to_json`, and `pio_to_normalized`
  derive new values from a handle.
- `pio_convert_file` converts a file path to output text in one call.
- `pio_export_arrow` uses `export` because it fills Arrow C Data Interface
  structs with release callbacks.

## Safety contract

Every entry point is hardened at the boundary:

- Panics never cross the FFI boundary: each function catches unwinds and turns
  them into an error return (NULL handle, `-1`, or a zero count) with a message
  in `errbuf`.
- NULL is safe everywhere: a NULL handle returns the documented default, NULL
  output pointers are skipped rather than written, and a NULL/zero-length
  `errbuf` suppresses the message.
- Error and warning buffers are always NUL-terminated; a message longer than
  the buffer is truncated to fit. `PIO_ERRBUF_MIN` (256) is a comfortable size.
- Input strings must be valid UTF-8; anything else is rejected as an error,
  never dereferenced past its NUL.
- Ownership is symmetric: handles from `pio_parse_*`/`pio_from_json`/
  `pio_to_normalized` are freed with `pio_network_free`, strings from the `pio_to_*`
  functions with `pio_string_free`, each exactly once. The Arrow export hands
  the caller two C Data Interface structs whose non-NULL `release` callbacks
  must each be invoked exactly once.
- The table extractors (`pio_branches`, `pio_gens`, ...) write exactly the
  matching `pio_n_*` count of elements into each non-NULL buffer; the caller
  must size them accordingly.

## ABI history

Compare `pio_abi_version()` against the `PIO_ABI_VERSION` your binary was
compiled against before any other call; a mismatch means the library and the
header disagree on the contract below. Breaking changes bump the version,
additive symbols do not.

| ABI | Breaking change |
|---|---|
| 1 | First versioned surface: opaque handles, typed extractors, JSON transport (#54). |
| 2 | `pio_parse` → `pio_parse_file`, `pio_convert` → `pio_convert_file`, `pio_write_matpower` → `pio_to_matpower` with an `errbuf` (#69). |
| 3 | `pio_case_free` → `pio_network_free`; `PioCase` → `PioNetwork` (opaque typedef) (#77). |

From v0.1.0 the ABI is additive only: new symbols may appear, but an existing
signature never changes or disappears without a `PIO_ABI_VERSION` bump released
in lockstep with PowerIO.jl.

## Scope

powerio-capi covers the `powerio` surface: parse / convert / query / table
and JSON extraction. It deliberately has no matrix builders; those live in
`powerio-matrix`. A future `powerio-matrix-capi` can hand back assembled CSR
matrices (B', Y_bus, PTDF, DC OPF) over the same ABI style; for now a consumer
builds matrices from the extracted tables.
