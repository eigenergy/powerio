# caseio-capi

A C ABI over `caseio`: parse any supported power-system case format, query it,
convert losslessly, and pull out the numeric tables a solver needs to assemble
matrices. This is the polyglot substrate — anything that speaks C (C, C++,
Julia, Python ctypes, …) can drive caseio through it.

The header is [`include/caseio.h`](include/caseio.h).

## Build

```
cargo build -p caseio-capi --release
# → target/release/libcaseio_capi.{so,dylib}  (cdylib)
#   target/release/libcaseio_capi.a            (staticlib)
```

Regenerate the header after changing the ABI:

```
cbindgen --config caseio-capi/cbindgen.toml --crate caseio-capi \
  --output caseio-capi/include/caseio.h
```

## C

```c
#include "caseio.h"
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    char err[256];
    CioCase *c = cio_parse("case9.m", NULL, err, sizeof err);
    if (!c) { fprintf(stderr, "parse: %s\n", err); return 1; }

    size_t n = cio_n_buses(c), m = cio_n_branches(c);
    printf("%zu buses, %zu branches, baseMVA %g\n", n, m, cio_base_mva(c));

    /* Pull the branch table to build a susceptance matrix yourself. */
    int64_t *from = malloc(m * sizeof *from), *to = malloc(m * sizeof *to);
    double  *x    = malloc(m * sizeof *x);
    cio_branches(c, from, to, NULL, x, NULL, NULL, NULL, NULL);
    /* ... assemble B' from (from, to, 1/x) ... */

    char *raw = cio_convert("case9.m", "psse", NULL, NULL, 0, err, sizeof err);
    if (raw) { /* ... use PSS/E text ... */ cio_string_free(raw); }

    free(from); free(to); free(x);
    cio_case_free(c);
    return 0;
}
```

## Julia (`ccall`)

```julia
const LIB = "libcaseio_capi"  # on the load path

function parse_case(path)
    err = zeros(UInt8, 256)
    h = ccall((:cio_parse, LIB), Ptr{Cvoid},
              (Cstring, Ptr{Cvoid}, Ptr{UInt8}, Csize_t),
              path, C_NULL, err, length(err))
    h == C_NULL && error(unsafe_string(pointer(err)))
    h
end

h = parse_case("case9.m")
n = ccall((:cio_n_buses, LIB), Csize_t, (Ptr{Cvoid},), h)
m = ccall((:cio_n_branches, LIB), Csize_t, (Ptr{Cvoid},), h)

from = Vector{Int64}(undef, m); x = Vector{Float64}(undef, m)
ccall((:cio_branches, LIB), Cvoid,
      (Ptr{Cvoid}, Ptr{Int64}, Ptr{Int64}, Ptr{Float64}, Ptr{Float64},
       Ptr{Float64}, Ptr{Float64}, Ptr{Float64}, Ptr{UInt8}),
      h, from, C_NULL, C_NULL, x, C_NULL, C_NULL, C_NULL, C_NULL)
# build your matrices from (from, x), then:
ccall((:cio_case_free, LIB), Cvoid, (Ptr{Cvoid},), h)
```

## Scope

caseio-capi covers the `caseio` surface: parse / write / convert / query / table
extraction. It deliberately has no matrix builders — those live in `casemat`. A
future `casemat-capi` can hand back assembled CSR matrices (B', Y_bus, PTDF,
DC-OPF) over the same ABI style; for now a consumer builds matrices from the
extracted tables.
