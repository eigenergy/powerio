# Migrating to C ABI 5

ABI 5 ships with powerio 0.9.0. It touches twenty-two symbols and renames none, so a binding written against ABI 4 keeps compiling except at the nine signatures below. That is the danger: most of this migration is behavior and JSON shape, which the compiler cannot find for you.

`PIO_ABI_VERSION` is 5. Bindings gate on equality, so a binding built against 4 refuses a 0.9.0 library and a binding built against 5 refuses everything earlier. There is no partial compatibility to arrange.

## What changed

| symbol | change |
|---|---|
| `pio_to_format` | signature: findings return through `char **out_diagnostics_json`, plus `const PioWriteOptions *opts` |
| `pio_convert_file` | signature: same |
| `pio_convert_str` | signature: same |
| `pio_write_dir` | signature: same |
| `pio_dist_to_format` | signature: same |
| `pio_dist_convert_file` | signature: same |
| `pio_dist_convert_str` | signature: same |
| `pio_normalize` | signature: `const PioNormalizeOptions *opts` |
| `pio_normalize_with_options` | removed, folded into `pio_normalize` |
| `pio_geo_parse` | signature: the reader's notes return through `char **out_diagnostics_json` |
| `pio_n_buses` | behavior: reports the star-lowered space |
| `pio_bus_ids` | behavior: same |
| `pio_n_branches` | behavior: same |
| `pio_branches` | behavior: same |
| `pio_branch_charging` | behavior: same |
| `pio_classify_str` | behavior: answers `model-json` for a bare model JSON document |
| `pio_parse_str` | behavior: the `powerio-json` token is gone |
| `pio_acopf_from_network` | removed |
| `pio_acopf_to_json` | removed |
| `pio_acopf_instance_free` | removed |
| `pio_build_info` | new |
| `pio_parse_bytes` | new |

The `PioAcopfInstance` typedef goes with its three symbols. Nothing else in the header moved.

## Conversion findings

The seven conversion entry points filled a caller buffer and truncated into it when the fidelity loss list outran the buffer. The length that would have told the caller was discarded, and the header advertised 256 bytes as sufficient. They take an out-pointer now, and what comes back is structured.

```c
/* ABI 4 */
char warnbuf[PIO_ERRBUF_MIN];
char *text = pio_to_format(net, "matpower", warnbuf, sizeof warnbuf, errbuf, sizeof errbuf);

/* ABI 5 */
char *diagnostics = NULL;
char *text = pio_to_format(net, "matpower", NULL, &diagnostics, errbuf, sizeof errbuf);
if (diagnostics) {
    /* a JSON array of records; the string is yours */
    pio_string_free(diagnostics);
}
```

`NULL` on return means the conversion lost nothing. Any other value is an owned JSON array you free with `pio_string_free`. Each element carries `code`, `severity` and `message`, and where known `element_path`, `source_ref`, `details` and `suggested_action`. Passing `NULL` for the parameter itself discards the findings without allocating. The call writes the out-pointer before it does any work, so a value left over from an earlier call is never read as this one's.

A warning is a record with severity `warning`, which is why there is one channel rather than two. A caller that only wants lines renders each record as `code + ": " + message`, which is exactly what the handle accessors below return.

**The trap.** You own the string whether or not you asked for it. A binding that passes a real pointer and then decides downstream it does not want the payload compiles, runs, and leaks on every conversion whose source format differs from its target — nothing about the call says the allocation happened. Decide at the call site: pass `NULL` to discard, or pass a pointer and free it unconditionally.

`pio_warnings` and `pio_dist_warnings` keep their signatures and stay the plain text view for a caller with no JSON parser. They use the size-then-fill idiom and cannot truncate.

## Write options

The four transmission write entry points take `const PioWriteOptions *opts` ahead of the diagnostics out-pointer. `NULL` is every default and is exactly what these calls did in ABI 4, so a binding that has nothing to say passes `NULL` and is done.

```c
PioWriteOptions opts;
memset(&opts, 0, sizeof opts);
opts.struct_size = sizeof opts;
opts.missing_gen_cost_mode = PIO_MISSING_GEN_COST_FILL;
opts.fill_c2 = 0.011;
opts.fill_c1 = 5.0;
char *text = pio_to_format(net, "matpower", &opts, NULL, errbuf, sizeof errbuf);
```

The struct is extensible in the convention `openat2` and `clone3` use. Zero it, set `struct_size` to `sizeof`, fill what you need. The library reads only the first `min(struct_size, its own sizeof)` bytes, so an older caller's shorter struct is read correctly and a newer caller's longer one is too as long as the fields past the library's size are zero. A **nonzero** field beyond that point fails the call with `BIND.CAPI.INVALID_OPTIONS`: you asked for something this build does not implement, and honoring the rest would drop your request in silence. Fields are appended, never reordered or removed, so a later write option costs a field rather than a symbol.

`gen_cost_csv` carries the patch table as CSV **text**, never a path. A write entry point does not open a file you name — that is the behavior 0.7.3 removed from the string entry points — so read the CSV yourself and hand over its bytes, which is what the CLI and the Python binding already do internally.

The three `pio_dist_*` write entry points take no options struct. The multiconductor writers carry their own per format options and none of them is reachable from this policy set; giving them a parameter with nothing to feed it would be the speculative version.

## Normalize options

`pio_normalize` takes `const PioNormalizeOptions *opts` and `pio_normalize_with_options` is gone. `NULL` is every default and is exactly what ABI 4's `pio_normalize` did, so the two flat arguments become fields under the same struct rules as `PioWriteOptions`.

```c
/* ABI 4 */
PioNetwork *n = pio_normalize_with_options(net, 1, 1.0472, errbuf, sizeof errbuf);

/* ABI 5 */
PioNormalizeOptions opts;
memset(&opts, 0, sizeof opts);
opts.struct_size = sizeof opts;
opts.clamp_angle_bounds = 1;
PioNetwork *n = pio_normalize(net, &opts, errbuf, sizeof errbuf);
```

`angle_bound_pad` must be in `(0, pi/2)` when the clamp is on, so `0` was never a value you could pass. It means the default 1.0472 radians, which is what makes a zero filled struct the default options with no sentinel to encode. Any other out of range pad still fails with the reader's own `InvalidNormalizeOption`.

A caller that passed `pio_normalize_with_options` through a runtime symbol lookup has to re-key that lookup: the symbol resolves to nothing now, and a lookup that silently falls back to its own repair will do that repair instead of this one with no error to show for it.

## Geo layer notes

`pio_geo_parse` reads a tolerant sidecar and skips what it cannot use: a buscoords row with too few columns, a Point feature with unusable coordinates, a route with no way to name its branch. Through ABI 4 those skips were dropped on the floor and the header said so, which left a C caller unable to tell a layer that read whole from one that read half.

```c
char *diagnostics = NULL;
char *canonical = pio_geo_parse(text, "buscoords.csv", &diagnostics, errbuf, sizeof errbuf);
if (diagnostics) {
    /* the same record shape the conversion channel returns; the string is yours */
    pio_string_free(diagnostics);
}
```

Same discipline as the conversion channel: written before the call does any work, `NULL` when the reader used every record, `NULL` for the parameter itself discards, and an error return leaves it `NULL` so there is nothing to free. `pio_geo_apply` is unchanged; it already appends the reader's notes to the handle it returns, where `pio_warnings` reads them.

## Diagnostic codes

Every `errbuf` message and every warning line reads `CODE: message`:

```text
REQUEST.FORMAT.UNKNOWN: unknown format `bogus`
BIND.CAPI.NULL_HANDLE: network handle is NULL
EMIT.PSSE.FIELD_DROPPED: generator cost curves dropped: PSS/E .raw has no cost data
```

Split at the first `": "`. The left side is `NAMESPACE.SCOPE.SPECIFIC` and contains no colon; the right side is prose, covered by no stability promise, so branch on the code and never on the message. The first segment names the stage and is the only segment to parse; treat the rest as opaque identity, and read more than three segments without complaint.

`pio_build_info` reports `diagnostic_namespaces`, the ten first segments powerio emits, and `error_categories`, the coarse five bucket projection each fatal code is published under. A code whose first segment is outside the ten came from somewhere else, which is data rather than a failure.

A published code keeps its identity forever. It may be retired, which stops it being emitted and never reassigns it, and a family may be refined by adding narrower codes beside it. A default severity may move in a minor release, so a consumer that needs a fixed policy pins by code.

## The `powerio-json` token, and what classification answers instead

`pio_parse_str` and `pio_to_format` accepted `powerio-json`, `powerio` and `json` for bare balanced model JSON. ABI 5 removes all three. Model JSON is powerio's own document rather than a case format, and `pio_to_json` and `pio_from_json` have carried it since ABI 4, so the token routed one thing through two entry points and made powerio look like the author of a case format it never wrote. A call passing any of the three now fails with `REQUEST.FORMAT.UNKNOWN`; the bare `json` alias goes with them, so a caller that wrote `"json"` meaning "let it sniff" must name a format or use `pio_parse_file`.

`pio_classify_str` answers `model-json` for such a document, where ABI 4 answered `transmission:powerio-json`. The label's family — everything up to the first `:` — is one of a closed six: `transmission`, `distribution`, `package`, `model-json`, `ambiguous`, `unknown`. Spellings are permanent, a family is never removed or redefined, and a new one is an addition with a changelog line, so a file picker that dispatches on the family keeps working. `pio_build_info` reports the same set under `json_classes`, which is where to read it rather than hardcoding it.

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

`pio_build_info` returns one owned JSON document, shaped after `curl_version_info`, holding `powerio_version`, `abi`, `features` (`arrow`, `matrix`, `gridfm`, `dist`, `pkg`, `prob`), `foreign_schemas` (`bmopf`), `diagnostic_namespaces`, `error_categories` and `json_classes`. The last two are the ones worth binding early: a caller that wants to branch on the kind of failure reads the code every message leads with, and these two report the sets a code decodes into.

`pio_parse_bytes` accepts every format name `pio_parse_str` takes plus `pwb`. PowerWorld binary has no text form and a NUL truncates it, so before this the only route to one was `pio_parse_file`, and a consumer holding an upload or an archive member had to stage a temporary file. It opens nothing, which is a security property rather than a convenience: it is the entry point for input you do not control, and it is why the 0.7.3 advisory fix works.

## The JSON documents, which is why the integer moved

Six documents changed shape while their symbols kept their signatures. This is the part that a compiler cannot catch, and on its own it is the reason ABI 5 exists: a binding built against 4 would pass the handshake and then read `null` for keys it mirrors.

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

Arrow table ids and column order. Every case format token except the three retired above. The opaque handle design. The panic guard on every entry point. `errbuf` and `errlen` last. `pio_abi_version`, `pio_version` and `pio_has_feature`.
