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

The test suite pins the checked-in header shape. Run the core and optional
surfaces before changing `powerio.h` or an exported `pio_*` function:

```
cargo test -p powerio-capi --no-default-features
cargo test -p powerio-capi --features arrow
cargo test -p powerio-capi --features gridfm
cargo test -p powerio-capi --features dist
cargo test -p powerio-capi --features arrow,gridfm,dist,pkg
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
    /* ... assemble B' from (from, to, 1/x) ... */

    /* Every format is a string: matpower echoes byte-exact, powerio-json is
     * the lossless snapshot, powermodels-json/psse/... convert with warnings. */
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
ccall((:pio_branches, LIB), Csize_t,
      (Ptr{Cvoid}, Ptr{Int64}, Ptr{Int64}, Ptr{Float64}, Ptr{Float64},
       Ptr{Float64}, Ptr{Float64}, Ptr{Float64}, Ptr{UInt8}, Csize_t),
      h, from, to, C_NULL, x, C_NULL, C_NULL, C_NULL, C_NULL, m)
# build your matrices from (from, to, x), then:
ccall((:pio_network_free, LIB), Cvoid, (Ptr{Cvoid},), h)
```

## The powerio-json snapshot

For consumers that want the whole case rather than the dense table slices, the
`powerio-json` format name serializes the entire `Network` (buses, loads,
shunts, branches, generators, storage, HVDC, and extras) through
`pio_to_format`, and `pio_parse_str` validates it back into a handle. This is
the transport the Julia package consumes: one call instead of stitching the
~dozen table extractors together. The retained source text is the one field the
snapshot omits, so a reloaded handle reformats on write rather than echoing a
byte-exact original.

## The `.pio.json` package surface

The default build includes the package surface (`PIO_PKG`). Probe it with
`pio_has_feature("pkg")` when loading dynamically. `PioPackage` is an opaque
compiler package handle, distinct from the parsed network handles:

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
- `pio_package_multiconductor_to_balanced_preflight_json` reports structured
  blockers before lowering, and `pio_package_lower_multiconductor_to_balanced`
  returns a new balanced package when the input is ready.

Options cross as JSON objects. `NULL` and `{}` mean defaults. The balanced
constructor accepts `include_solver_metadata` (default `false`). The
multiconductor lowering calls accept `base_mva` (default `100.0`) and
`convention` (default `fortescue_power_invariant`). Unknown option keys are
errors.

## API names

The grammar is written out in the header preamble; the short version:

- Verb-led names are operations, and the verb fixes the return family:
  `pio_parse_file`, `pio_parse_str`, `pio_read_dir`, and `pio_normalize`
  return a new handle; `pio_write_dir` writes the filesystem;
  `pio_convert_file`/`pio_convert_str` transcode without keeping a handle.
- `pio_to_format` is the one text serializer; `pio_to_arrow` earns its own
  symbol only because its output type is Arrow C Data Interface structs.
- `pio_package_*` functions operate on the package envelope, not on a new
  network handle family.
- Format names never appear in symbols: `matpower`, `psse`, `powerio-json`,
  `pypsa-csv`, `gridfm`, and every future format are strings, so a new format
  never changes this ABI.
- Noun phrases are queries: `pio_n_*` counts, `pio_is_radial`,
  `pio_bus_ids`/`pio_branches`/`pio_gens`/`pio_bus_demand`/`pio_bus_shunt`
  extractors, `pio_warnings` for the handle's fidelity warnings.
- One meaning per word, transmission and distribution alike: a *bus* is a
  named connection point (this surface is bus granular), a *node* is one
  conductor's point at a bus (reserved for the multiconductor surface), and a
  *branch* is any two-terminal series element, lines and transformers alike.

## Safety model

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

Two notes on the trust model:

- Malformed or hostile input surfaces as an error or, at worst, a caught
  panic; the parsers are safe Rust and fuzzed (see `fuzz/`), so undefined
  behavior is out of reach on any input. Resource use is the caveat: memory
  scales with input size and no size caps are enforced, so cap untrusted
  inputs yourself if you parse them in bulk.
- The panic guards assume the default `panic = "unwind"`. A downstream
  rebuild with `panic = "abort"` turns a caught-class bug into an orderly
  process abort instead of an error return.

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
| 4 | The naming grammar: format symbols folded into format strings (`pio_to_matpower`/`pio_to_json`/`pio_from_json` → `pio_to_format`/`pio_parse_str` with `powerio-json`; `pio_write_pypsa_csv_folder` → `pio_write_dir`; `pio_read_gridfm`/`pio_gridfm_scenario_ids` → `pio_read_dir`/`pio_scenario_ids`), `pio_to_normalized` → `pio_normalize`, `pio_export_arrow` → `pio_to_arrow`, cap/count extractors, byte-length `pio_warnings`, `pio_ref_bus_index`/`pio_ref_bus_indices`, `pio_n_islands`, `pio_bus_demand`/`pio_bus_shunt`, `pio_convert_*(input, from, to, ...)`, new `pio_convert_str`. |

One v4 break deserves a callout: `pio_convert_file` kept its symbol, arity,
and parameter types but reordered arguments 2 and 3 from `(path, to, from)`
to `(path, from, to)`. Every other v4 change renames a symbol or changes an
arity, so a stale caller fails at link or load; this one links fine and reads
the formats reversed. It is the reason the `pio_abi_version()` handshake is
not optional.

The grammar v4 fixed is the freeze: existing signatures never change again,
new data means new symbols, and rich or multiconductor data rides the Arrow,
`powerio-json`, and `.pio.json` schemas, which evolve without touching a C
signature. Any future break would bump `PIO_ABI_VERSION` in lockstep with
PowerIO.jl.

The optional `pio_dist_*` surface has its own version check:
`pio_dist_abi_version()` against `PIO_DIST_ABI_VERSION`, after confirming
`pio_has_feature("dist")`. Supported direct C use of the distribution surface
starts at `PIO_DIST_ABI_VERSION = 1`, with one-shot conversion ordered as
`pio_dist_convert_*(input, from, to, ...)`. The dist conversion symbols that
appeared before this split should be treated as experimental.

Every public `PIO_*` macro, opaque typedef, and `pio_*` prototype in
`powerio.h` is pinned by a Cargo test, and CI compiles the C smoke program
against the no-default core ABI plus the arrow, gridfm, dist, and pkg feature
surfaces. CI also compiles and links a C++ header sanity program to keep the
`extern "C"` path honest. Source/header symbol parity is checked separately, so
adding, renaming, or deleting a public entry point fails before release.

## Scope

powerio-capi covers the `powerio` surface: parse / convert / query / table
and JSON extraction. It deliberately has no matrix builders; those live in
`powerio-matrix`. The one `powerio-matrix` surface it does expose is the gridfm
reader (`--features gridfm`), because it just returns a plain network handle. A
future `powerio-matrix-capi` can hand back assembled CSR matrices (B', Y_bus,
PTDF, DC OPF) over the same ABI style; for now a consumer builds matrices from
the extracted tables.
