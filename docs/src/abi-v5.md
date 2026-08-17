# Migrating to C ABI 5

ABI 5 ships with powerio 0.9.0. It touches seventeen symbols and renames none, so a binding written against ABI 4 keeps compiling except at the seven signatures below. That is the danger: most of this migration is behavior and JSON shape, which the compiler cannot find for you.

`PIO_ABI_VERSION` is 5. Bindings gate on equality, so a binding built against 4 refuses a 0.9.0 library and a binding built against 5 refuses everything earlier. There is no partial compatibility to arrange.

## What changed

| symbol | change |
|---|---|
| `pio_to_format` | signature: warnings return through `char **out_warnings` |
| `pio_convert_file` | signature: same |
| `pio_convert_str` | signature: same |
| `pio_write_dir` | signature: same |
| `pio_dist_to_format` | signature: same |
| `pio_dist_convert_file` | signature: same |
| `pio_dist_convert_str` | signature: same |
| `pio_n_buses` | behavior: reports the star-lowered space |
| `pio_bus_ids` | behavior: same |
| `pio_n_branches` | behavior: same |
| `pio_branches` | behavior: same |
| `pio_branch_charging` | behavior: same |
| `pio_acopf_from_network` | removed |
| `pio_acopf_to_json` | removed |
| `pio_acopf_instance_free` | removed |
| `pio_build_info` | new |
| `pio_parse_bytes` | new |

The `PioAcopfInstance` typedef goes with its three symbols. Nothing else in the header moved.

## Conversion warnings

The seven conversion entry points filled a caller buffer and truncated into it when the fidelity loss list outran the buffer. The length that would have told the caller was discarded, and the header advertised 256 bytes as sufficient. They take an out-pointer now.

```c
/* ABI 4 */
char warnbuf[PIO_ERRBUF_MIN];
char *text = pio_to_format(net, "matpower", warnbuf, sizeof warnbuf, errbuf, sizeof errbuf);

/* ABI 5 */
char *warnings = NULL;
char *text = pio_to_format(net, "matpower", &warnings, errbuf, sizeof errbuf);
if (warnings) {
    /* the conversion lost something; the string is yours */
    pio_string_free(warnings);
}
```

`NULL` on return means the conversion lost nothing. Any other value is an owned string you free with `pio_string_free`. Passing `NULL` for the parameter itself discards the warnings without allocating. The call writes the out-pointer before it does any work, so a value left over from an earlier call is never read as this one's.

**The trap.** You own the string whether or not you asked for it. A binding that passes a real pointer and then decides downstream it does not want the text compiles, runs, and leaks on every conversion whose source format differs from its target — nothing about the call says the allocation happened. Decide at the call site: pass `NULL` to discard, or pass a pointer and free it unconditionally. PowerIO.jl's `_format_from_handle` is the worked example, threading a `want_warnings` flag down to the pointer itself.

`pio_warnings` and `pio_dist_warnings` are unchanged. They use the size-then-fill idiom and cannot truncate.

## The star-lowered space

A case with an in-service 3-winding transformer lowers before the dense extractors run, adding one star bus and three branches per transformer. Through ABI 4 the bus and branch tables reported the unexpanded case file while `pio_bus_demand`, `pio_bus_shunt` and `pio_n_islands` reported the expansion. A per-bus buffer sized from `pio_n_buses` read short, its trailing entries had no id, and a matrix built from the pair left the star point isolated.

Five symbols move to the lowered space so the whole surface agrees: `pio_n_buses`, `pio_bus_ids`, `pio_n_branches`, `pio_branches`, `pio_branch_charging`.

Two assertions are the migration test, and they hold on 5 and fail on 4:

```c
assert(pio_bus_ids(net, NULL, 0) == pio_n_buses(net));
/* and: every branch endpoint is a bus the API reports */
```

Assert the closure rather than the counts. On the case9 variant carrying one in-service 3-winding transformer, ABI 4 reported 9 buses and 9 branches and ABI 5 reports 10 and 12. Every one of those four numbers is plausible read on its own, so a test that pins a count passes for the wrong reason as easily as the right one.

The Arrow tables keep the distinction explicit rather than resolving it. `PIO_ARROW_TABLE_BUS` and `PIO_ARROW_TABLE_BRANCH` are the case file's own rows and stay unexpanded. `PIO_ARROW_TABLE_MATRIX_BUS` and `PIO_ARROW_TABLE_MATRIX_BRANCH` are the lowered space these extractors now agree with. A consumer that joins a raw table against an extractor column is reading two spaces and needs to pick one.

## Removed

The three `pio_acopf_*` symbols had no C consumer. They are deleted rather than carried through the freeze, and will be re-cut additively when a consumer exists and can say what shape it needs. Deleting rather than deprecating is safe here: the symbol stops resolving, so a caller fails at load instead of reading something wrong.

## New

```c
char *pio_build_info(void);
PioNetwork *pio_parse_bytes(const uint8_t *bytes, size_t len, const char *format,
                            char *errbuf, size_t errlen);
```

`pio_build_info` returns one owned JSON document, shaped after `curl_version_info`, holding `powerio_version`, `abi`, `features` (`arrow`, `matrix`, `gridfm`, `dist`, `pkg`, `prob`), `foreign_schemas` (`bmopf`), and `error_categories`. That last key is the one worth binding early: the ABI reports failures as text and defines no error codes, so a caller that wants to branch on the kind of failure matches these tokens rather than parsing English.

`pio_parse_bytes` accepts every format name `pio_parse_str` takes plus `pwb`. PowerWorld binary has no text form and a NUL truncates it, so before this the only route to one was `pio_parse_file`, and a consumer holding an upload or an archive member had to stage a temporary file. It opens nothing, which is a security property rather than a convenience: it is the entry point for input you do not control, and it is why the 0.7.3 advisory fix works.

## The JSON documents, which is why the integer moved

Seven documents changed shape while their symbols kept their signatures. This is the part that a compiler cannot catch, and on its own it is the reason ABI 5 exists: a binding built against 4 would pass the handshake and then read `null` for keys it mirrors.

| document | change |
|---|---|
| `pio_schema_versions_json` | dropped four keys |
| `pio_dist_capabilities_json` | `schema_version` → `powerio_version` |
| `pio_arrow_catalog_json` | same rename; the per-table `schema_version` is gone |
| `pio_scopf_to_json` | same rename, plus new fields |
| `pio_package_to_json` | same rename |
| `pio_dist_summary_json` | same rename |
| `pio_summary_json` | gains `topology.n_buses` and `topology.n_branches` |
| Arrow schema metadata | key became `powerio.version` |

The rename is one edit everywhere. One release version now covers every document powerio authors, so a per-document `schema_version` frozen at `1.0.0` said nothing a caller could act on.

`pio_scopf_to_json` gains `violation_cost`, `device_class_layout`, `lengths.K`, `j_sh` on shunt rows, and eight generator row fields. Its indices stay 1-based.

`pio_summary_json`'s `counts` block stays the case file's own inventory, so a 3-winding transformer counts once there rather than as the bus and three branches it lowers to. The two new `topology` fields are the lowered space, which is what the extractors report. Reading `counts` where you want `topology` is the same class of error the bus space change fixes.

`pio_dist_graph_json`, `pio_package_validation_json`, `pio_package_diagnostics_json`, `pio_package_operating_points_json`, `pio_package_study_json` and `pio_package_multiconductor_to_balanced_preflight_json` are byte for byte what ABI 4 emitted.

## `PIO_DIST_ABI_VERSION`

Frozen at 1 and no longer meaningful. It existed to absorb distribution volatility, and that volatility lives in the BMOPF schema, which changes a reader, a writer and an emitted token but no C signature. One shared object carrying two compatibility promises is a thing no mature C library does.

The symbol stays because PowerIO.jl gates twelve distribution call sites on resolving it, and removing it would make a library that fully supports distribution refuse every distribution call. Do not build new gates on it. Read foreign schema versions from `pio_build_info`, which can say "BMOPF 0.2 but not 0.3" where an integer checked once at load cannot.

## What a real migration cost

PowerIO.jl is the one binding powerio owns, and its ABI 5 change is the honest estimate. Six categories of edit:

1. The warnings channel, at seven call sites across two files. The fixed 64 KiB buffer, the truncation marker and the machinery around them were deleted rather than adapted.
2. `pio_parse_bytes` bound as one new `ccall` plus two wrapper methods.
3. `pio_build_info` bound as one call, guarded to return nothing on an older library.
4. The ABI constant, one line.
5. A sentinel spelling: the dense view's `reference_bus` reports `nothing` rather than the C `-1`.
6. The removed `pio_acopf_*` symbols cost nothing, because nothing called them.

The star-lowered space required no binding edit at all, which is the point worth carrying away: the numbers changed and the code did not. A binding that sizes buffers from one extractor and fills them from another was already correct or already broken; ABI 5 decides which. Assert the closure and find out.

## What did not change

Arrow table ids and column order. The format tokens, which are strings and were never symbols. The opaque handle design. The panic guard on every entry point. `errbuf` and `errlen` last. `pio_abi_version`, `pio_version` and `pio_has_feature`.
