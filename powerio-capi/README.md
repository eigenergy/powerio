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
    PioCase *c = pio_parse("case9.m", NULL, err, sizeof err);
    if (!c) { fprintf(stderr, "parse: %s\n", err); return 1; }

    size_t n = pio_n_buses(c), m = pio_n_branches(c);
    printf("%zu buses, %zu branches, baseMVA %g\n", n, m, pio_base_mva(c));

    /* Pull the branch table to build a susceptance matrix yourself. */
    int64_t *from = malloc(m * sizeof *from), *to = malloc(m * sizeof *to);
    double  *x    = malloc(m * sizeof *x);
    pio_branches(c, from, to, NULL, x, NULL, NULL, NULL, NULL);
    /* ... assemble B' from (from, to, 1/x) ... */

    char *raw = pio_convert("case9.m", "psse", NULL, NULL, 0, err, sizeof err);
    if (raw) { /* ... use PSS/E text ... */ pio_string_free(raw); }

    free(from); free(to); free(x);
    pio_case_free(c);
    return 0;
}
```

## Julia (`ccall`)

For a typed Julia API, use [PowerIO.jl](https://github.com/eigenergy/PowerIO.jl),
which wraps this ABI (`set_library!`, `parse_case`, `convert_case`, and ecosystem
bridges). The raw `ccall` below is the low-level reference it builds on.

```julia
const LIB = "libpowerio_capi"  # on the load path

function parse_case(path)
    err = zeros(UInt8, 256)
    h = ccall((:pio_parse, LIB), Ptr{Cvoid},
              (Cstring, Ptr{Cvoid}, Ptr{UInt8}, Csize_t),
              path, C_NULL, err, length(err))
    h == C_NULL && error(unsafe_string(pointer(err)))
    h
end

h = parse_case("case9.m")
n = ccall((:pio_n_buses, LIB), Csize_t, (Ptr{Cvoid},), h)
m = ccall((:pio_n_branches, LIB), Csize_t, (Ptr{Cvoid},), h)

from = Vector{Int64}(undef, m); x = Vector{Float64}(undef, m)
ccall((:pio_branches, LIB), Cvoid,
      (Ptr{Cvoid}, Ptr{Int64}, Ptr{Int64}, Ptr{Float64}, Ptr{Float64},
       Ptr{Float64}, Ptr{Float64}, Ptr{Float64}, Ptr{UInt8}),
      h, from, C_NULL, C_NULL, x, C_NULL, C_NULL, C_NULL, C_NULL)
# build your matrices from (from, x), then:
ccall((:pio_case_free, LIB), Cvoid, (Ptr{Cvoid},), h)
```

## JSON transport

For consumers that want the whole case rather than the dense table slices,
`pio_to_json` serializes the entire `Network` (buses, loads, shunts, branches,
generators, storage, HVDC, and extras) to a string, and `pio_from_json` rebuilds
a handle from it. This is the transport the Julia package consumes: one call
instead of stitching the ~dozen table extractors together. The retained source
text is not part of the JSON, so a `from_json` handle reformats on write rather
than echoing a byte-exact original.

## Scope

powerio-capi covers the `powerio` surface: parse / write / convert / query / table
and JSON extraction. It deliberately has no matrix builders; those live in
`powerio-matrix`. A future `powerio-matrix-capi` can hand back assembled CSR
matrices (B', Y_bus, PTDF, DC OPF) over the same ABI style; for now a consumer
builds matrices from the extracted tables.
