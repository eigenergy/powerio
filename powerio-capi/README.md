# powerio-capi

The C ABI parses, queries, and converts PowerIO networks through opaque handles.
It also exposes copied numeric tables, Arrow tables, `.pio.json` packages, and
SCOPF problem instances behind feature gates.

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

The test suite pins the checked-in header shape. Run the core and optional
surfaces before changing `powerio.h` or an exported `pio_*` function:

```
cargo test -p powerio-capi --no-default-features
cargo test -p powerio-capi --features arrow
cargo test -p powerio-capi --features arrow,matrix
cargo test -p powerio-capi --features gridfm
cargo test -p powerio-capi --features dist
cargo test -p powerio-capi --features arrow,matrix,gridfm,dist,pkg,prob
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
    char err[256];
    PioNetwork *c = pio_parse_file("case9.m", NULL, err, sizeof err);
    if (!c) { fprintf(stderr, "parse: %s\n", err); return 1; }

    size_t n = pio_n_buses(c), m = pio_n_branches(c);
    printf("%zu buses, %zu branches, baseMVA %g\n", n, m, pio_base_mva(c));

    /* Pull the branch table to build a susceptance matrix yourself. Extractors
     * write up to cap entries and return the total, so a short buffer is
     * detectable; NULL out (or cap 0) is the count query. */
    int64_t *from = malloc(m * sizeof *from), *to = malloc(m * sizeof *to);
    double  *x    = malloc(m * sizeof *x);
    pio_branches(c, from, to, NULL, x, NULL, NULL, NULL, NULL, m);
    /* ... assemble L = A diag(1/x) A^T from (from, to, x) ... */

    /* Every case format is a string. MATPOWER echoes byte exact;
     * PowerModels JSON and PSS/E conversions can report warnings. */
    char warn[256];
    char *matpower = pio_to_format(c, "matpower", warn, sizeof warn, err, sizeof err);
    if (matpower) { /* ... use MATPOWER text ... */ pio_string_free(matpower); }

    char *json = pio_to_format(c, "powermodels-json", warn, sizeof warn, err, sizeof err);
    if (json) { /* ... use PowerModels JSON text ... */ pio_string_free(json); }

    char *raw = pio_convert_file("case9.m", NULL, "psse", NULL, 0, err, sizeof err);
    if (raw) { /* ... use PSS/E text ... */ pio_string_free(raw); }

    free(from); free(to); free(x);
    pio_network_free(c);
    return 0;
}
```

## Julia (`ccall`)

For a typed Julia API, use [PowerIO.jl](https://github.com/eigenergy/PowerIO.jl),
which wraps this ABI (`set_library!`, `parse_file`, `parse_str`, `convert_file`,
and the `to_*` transforms). The raw `ccall` below is the low level reference it
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
ccall((:pio_branches, LIB), Csize_t,
      (Ptr{Cvoid}, Ptr{Int64}, Ptr{Int64}, Ptr{Float64}, Ptr{Float64},
       Ptr{Float64}, Ptr{Float64}, Ptr{Float64}, Ptr{UInt8}, Csize_t),
      h, from, to, C_NULL, x, C_NULL, C_NULL, C_NULL, C_NULL, m)
# build your matrices from (from, to, x), then:
ccall((:pio_network_free, LIB), Cvoid, (Ptr{Cvoid},), h)
```

## Balanced model JSON

For consumers that want the whole case rather than the dense table slices, the
`pio_to_json` and `pio_from_json` pair carries the entire balanced `Network`:
buses, loads, shunts, branches, generators, storage, HVDC, and extras. The
retained source text is excluded, so a reloaded handle converts from the model
instead of echoing the original source.

ABI v4 still accepts `powerio-json` in `pio_to_format` and `pio_parse_str`.
Those names are compatibility aliases for the explicit model JSON functions.

## The `.pio.json` document surface

The default build includes the package surface (`PIO_PKG`). Probe it with
`pio_has_feature("pkg")` when loading dynamically. `PioPackage` is an opaque
`.pio.json` document handle, distinct from the parsed network handles:

- `pio_package_parse_file` and `pio_package_parse_str` read `.pio.json`.
- `pio_package_to_json` returns compact `.pio.json`; free it with
  `pio_string_free`.
- `pio_package_from_balanced_network` wraps a `PioNetwork`, the historical C
  handle for `powerio::BalancedNetwork`.
- `pio_package_from_multiconductor_network` wraps a `PioDistNetwork`, the
  historical C handle for `powerio_dist::MulticonductorNetwork`, when both
  `PIO_PKG` and `PIO_DIST` are present.
- `pio_package_validate`, `pio_package_validation_json`, and
  `pio_package_diagnostics_json` expose structured validation state.
- `pio_package_operating_points_json` returns the replayable operating point
  series, or JSON `null` when the document has none.
- `pio_package_set_operating_points` replaces the operating point series
  from JSON. `null` or an empty series clears it. The call reruns package
  validation before returning.
- `pio_package_materialize_operating_point` returns a new static document with
  one operating point applied. Updates resolve by the model rows' `uid`
  identities; an unknown identity, an ambiguous (duplicated) uid, or a row that
  contradicts a resolved identity returns `NULL` with the message in `errbuf`.
- `pio_package_multiconductor_to_balanced_preflight_json` reports structured
  blockers before lowering, and `pio_package_lower_multiconductor_to_balanced`
  returns a new balanced document when the input is ready.

Constructor and lowering options cross as typed parameters. The balanced
constructor takes `include_solver_metadata`; any nonzero value records compact
normalized solver table metadata. The multiconductor lowering calls take
`base_mva`, the three phase system power base used for the balanced per-unit
projection. The transform convention is fixed by these functions; if another
convention becomes a real public option, it should get a new additive symbol.

## Problem instances

Build the library with `--features prob`; the generated header guards this
surface with `PIO_PROB`. `pio_scopf_parse_str` accepts source text and a format
name. It currently accepts `goc3-json` and returns an owned
`PioScopfInstance` handle. A null result indicates a parse or assembly error;
`errbuf` receives the message.

`pio_scopf_to_json` returns the versioned SCOPF wire document. The document
records its schema version and 1-based index convention. Free the returned
string with `pio_string_free` and the instance with
`pio_scopf_instance_free`.

## API names

The grammar is written out in the header preamble; the short version:

- Verb-led names are operations, and the verb fixes the return family:
  `pio_parse_file`, `pio_parse_str`, `pio_read_dir`, and `pio_normalize`
  return a new handle; `pio_write_dir` writes the filesystem;
  `pio_convert_file`/`pio_convert_str` transcode without keeping a handle.
- `pio_to_format` serializes named case formats. `pio_to_json` serializes the
  balanced model. `pio_to_arrow` fills Arrow C Data Interface structs.
- `pio_package_*` functions operate on `.pio.json` document metadata, not on a new
  network handle family.
- Case format names never appear in symbols: `matpower`, `psse`, `pypsa-csv`,
  `gridfm`, `goc3-json`, `surge-json`, and future formats are strings.
- Noun phrases are queries: `pio_n_*` counts, `pio_is_radial`,
  `pio_bus_ids`/`pio_branches`/`pio_gens`/`pio_bus_demand`/`pio_bus_shunt`
  extractors, `pio_warnings` for the handle's fidelity warnings.
- One meaning per word, transmission and distribution alike: a *bus* is a
  named connection point (this surface is bus granular), a *node* is one
  conductor's point at a bus (reserved for the multiconductor surface), and a
  *branch* is any two-terminal series element, lines and transformers alike.

## Safety model

Every entry point is hardened at the boundary:

- With Rust's default unwind panic strategy, each function catches a panic and
  returns its documented error value (a null handle, `-1`, or a zero count).
- NULL is safe everywhere: a NULL handle returns the documented default, NULL
  output pointers are skipped rather than written, and a NULL/zero-length
  `errbuf` suppresses the message.
- Error and warning buffers are always NUL-terminated; a message longer than
  the buffer is truncated to fit. `PIO_ERRBUF_MIN` (256) is the recommended
  size.
- Input strings must be valid UTF-8 and NUL terminated. Invalid UTF-8 returns an
  error.
- Ownership is symmetric: handles from `pio_parse_*`/`pio_read_dir`/
  `pio_normalize` are freed with `pio_network_free`, strings from
  `pio_to_format`/`pio_convert_*` with `pio_string_free`, package handles with
  `pio_package_free`, each exactly once.
  The Arrow export hands the caller two C Data Interface structs whose
  non-NULL `release` callbacks must each be invoked exactly once.
- The table extractors (`pio_branches`, `pio_gens`, ...) write at most `cap`
  elements into each non-NULL buffer and return the total available, so a
  miscounted buffer reads short instead of overflowing, and `(NULL, 0)` sizes
  it. `pio_warnings` returns the byte length needed the same way.
- A handle is immutable after construction: concurrent reads from any number
  of threads are safe. `pio_network_free` is not; free exactly once.

Input size is not capped. Callers that accept untrusted input must impose their
own byte and resource limits. The panic guards require the default
`panic = "unwind"`; a build with `panic = "abort"` terminates the process on a
panic.

## ABI history

Compare `pio_abi_version()` against the `PIO_ABI_VERSION` your binary was
compiled against before any other call; a mismatch means the library and the
header disagree on the rules below. Breaking changes bump the version, additive
symbols do not.

| ABI | Breaking change |
|---|---|
| 1 | First versioned surface: opaque handles, typed extractors, JSON transport (#54). |
| 2 | `pio_parse` → `pio_parse_file`, `pio_convert` → `pio_convert_file`, `pio_write_matpower` → `pio_to_matpower` with an `errbuf` (#69). |
| 3 | `pio_case_free` → `pio_network_free`; `PioCase` → `PioNetwork` (opaque typedef) (#77). |
| 4 | The naming grammar: case formats use `pio_to_format`/`pio_parse_str`; directory formats use `pio_write_dir`/`pio_read_dir`; `pio_normalize`, `pio_to_arrow`, cap/count extractors, byte length `pio_warnings`, reference bus and island queries, demand and shunt extractors, and string/file conversion entry points use fixed signatures. Balanced model JSON uses the additive `pio_to_json`/`pio_from_json` pair; `powerio-json` remains a compatibility format token. |

One v4 break deserves a callout: `pio_convert_file` kept its symbol, arity,
and parameter types but reordered arguments 2 and 3 from `(path, to, from)`
to `(path, from, to)`. Every other v4 change renames a symbol or changes an
arity, so a stale caller fails at link or load; this one links fine and reads
the formats reversed. It is the reason the `pio_abi_version()` handshake is
not optional.

The grammar v4 fixed is the freeze: existing signatures never change again,
new data means new symbols, and rich or multiconductor data rides the Arrow,
model JSON, and `.pio.json` schemas. Any future signature break would bump
`PIO_ABI_VERSION` in lockstep with PowerIO.jl.

The optional `pio_dist_*` surface has its own version check:
`pio_dist_abi_version()` against `PIO_DIST_ABI_VERSION`, after confirming
`pio_has_feature("dist")`. Supported direct C use of the distribution surface
starts at `PIO_DIST_ABI_VERSION = 1`, with one-shot conversion ordered as
`pio_dist_convert_*(input, from, to, ...)`. The dist conversion symbols that
appeared before this split should be treated as experimental.

Every public `PIO_*` macro, opaque typedef, and `pio_*` prototype in
`powerio.h` is pinned by a Cargo test, and CI compiles the C smoke program
against the no-default core ABI plus the arrow, matrix, gridfm, dist, pkg, and prob
feature surfaces. CI also compiles and links a C++ header sanity program to keep the
`extern "C"` path honest. Source/header symbol parity is checked separately, so
adding, renaming, or deleting a public entry point fails CI.

## Scope

`powerio-capi` covers the `powerio` parse, convert, query, table,
and JSON extraction. Build with `--features arrow,matrix` to export the first
balanced sparse matrix family over `pio_to_arrow` as COO triplet tables:
`ybus`, `incidence`, `bprime`, and `bdoubleprime`, all in dense solver bus index
space with dimensions and axis names in Arrow schema metadata. The stable matrix
ABI is Arrow COO plus `matrix_bus` and `matrix_branch` axis map tables; C stays
language neutral, while Julia, Python, and other bindings own their native
sparse matrix assembly. Runtime consumers can call
`pio_matrix_available()` before selecting those table ids. Larger matrix
PTDF and LODF remain in `powerio-matrix`. The `prob` feature exposes matrix free
SCOPF instances. DC OPF instances and bundles do not yet have C entry points.
