# C ABI v5: every symbol, and why

v4 froze a grammar. v5 applies it to the symbols v4 left alone, and it is the
last C break powerio takes. Everything here is decided against one question:
what does a caller in 2029 need this to be.

The v4 surface is 84 functions, 5 opaque handles, zero `#[repr(C)]` structs,
and 24 macros. This document lists every one of the 84 as kept, renamed,
re-signatured, or removed.

## The six rules

**1. Every symbol names its subject.** v4 left the balanced model unnamed, so
`pio_parse_str` was balanced, `pio_dist_parse_str` was multiconductor, and
`pio_package_parse_str` was a package. The reader had to know which omission
meant what. Both models are peers: `pio_balanced_*` and
`pio_multiconductor_*`. The precedent is `curl_easy_*` / `curl_multi_*` /
`curl_url_*`.

No abbreviations. `mc` means Monte Carlo to a power engineer and `bal` invites
balancing area; both are common, different concepts in this domain, so a short
form lands in a crowded namespace. The longest symbol in v5 is about 31
characters, against 52 in v4.

**2. `dist` is gone as a word.** v4 used `dist` in 15 symbols and
`multiconductor` in 4, in the same header, for the same model, and its own
comments called the handle name historical. One name per concept.

**3. One buffer idiom, and the library never allocates.** v4 had three: 27
symbols allocated and returned an owned `char *`, 5 filled a caller buffer and
returned the needed length, 9 filled caller arrays and returned a count. v5
keeps only the second and third: `size_t pio_x(…, out, cap)` returns the total
available, and `NULL`/`0` is a size query. `pio_string_free` is deleted with
the whole owned-`char *` class, which removes an entire ownership rule and lets
a caller use its own allocator.

**4. A conversion is a handle, because it has two outputs.** v4's seven
`warnbuf` symbols silently truncated: `finish_conversion` discarded the needed
length and there was no size query, so a long fidelity-loss list was lost with
no signal. The header made it worse by telling callers `PIO_ERRBUF_MIN` (256
bytes) always sufficed for a `warnbuf`.

A conversion has text *and* warnings, both needing a size query, and neither can
attach to the network handle because write-time fidelity warnings are produced
at write time. `PioConversion` is not new design: `Conversion` already exists in
Rust and in Python; the C ABI is the one surface that flattened it and lost the
warnings.

**5. One extensible options struct is what buys the freeze.** With zero
`repr(C)` structs, every new option needs a new symbol, and over three years
that either bloats the surface or forces a v6. A leading `struct_size` lets the
library treat absent tail fields as defaults, so a caller compiled against the
0.9.0 header keeps working against a 2029 library that added three fields. This
is the Win32 / Vulkan / `sqlite3_config` idiom.

**6. One ABI integer.** `PIO_DIST_ABI_VERSION` existed to absorb distribution
volatility, but that volatility is in the BMOPF schema, which the IEEE task
force owns and powerio only reproduces. A BMOPF revision changes a reader, a
writer, and an emitted token; it changes no C signature. So the second integer
never fires for its stated reason, and what it does instead is give one shared
object two compatibility promises, which no mature C library does. Foreign
schema drift is reported at runtime by `pio_multiconductor_capabilities` and
`pio_document_versions`, which is the right tool: an integer checked once at
load cannot express "I speak BMOPF 0.2 but not 0.3".

## The table

`R` renamed · `S` re-signatured · `X` removed · `=` kept unchanged

### Handshake and capability (7 → 5)

| v4 | v5 | | why |
|---|---|---|---|
| `pio_abi_version` | same | = | returns 5 |
| `pio_dist_abi_version` | — | X | rule 6 |
| `pio_dist_capabilities_json` | `pio_multiconductor_capabilities` | R S | rules 2, 3 |
| `pio_schema_versions_json` | `pio_document_versions` | R S | "schema versions" became one powerio version plus the foreign ones |
| `pio_matrix_available` | — | X | `pio_has_feature("matrix")` says the same thing, and the two disagreed on meaning |
| `pio_has_feature` | same | = | |
| `pio_version` | same | = | the one non-owned `const char *`; documented as such |

### Balanced model: read, normalize, write (13 → 10)

| v4 | v5 | | why |
|---|---|---|---|
| `pio_parse_file` | `pio_balanced_parse_file` | R | rule 1 |
| `pio_parse_str` | `pio_balanced_parse_str` | R | rule 1 |
| `pio_from_json` | `pio_balanced_from_json` | R | rule 1 |
| `pio_read_dir` | `pio_balanced_read_dataset` | R | `read_dir` named the mechanism; the concept is a dataset |
| `pio_scenario_ids` | `pio_dataset_scenario_ids` | R S | `ptrdiff_t` was the only one in the ABI, invented by the `-1` sentinel; `size_t` + `errbuf` |
| `pio_classify_str` | `pio_classify` | R S | gains an error channel; it had none, so 0 meant every failure |
| `pio_normalize` | `pio_balanced_normalize` | R S | takes `const PioNormalizeOptions *`, `NULL` for defaults |
| `pio_normalize_with_options` | — | X | folded into the options struct, rule 5 |
| `pio_to_json` | `pio_balanced_to_json` | R S | rule 3 |
| `pio_to_format` | `pio_balanced_to_format` | R S | returns `PioConversion *`, rule 4 |
| `pio_convert_file` | — | X | parse + to_format; it existed only to skip a handle and cost the warnings |
| `pio_convert_str` | — | X | same |
| `pio_write_dir` | `pio_balanced_write_dataset` | R S | warnings through the conversion handle |

### Balanced model: queries and extractors (19 → 19)

Every one is `R` for rule 1, and the four marked `S` change more:

`pio_n_buses`, `pio_n_branches`, `pio_n_switches`, `pio_base_mva`,
`pio_network_name`, `pio_ref_bus_index`, `pio_ref_bus_indices`,
`pio_n_islands`, `pio_is_radial`, `pio_branches`, `pio_branch_charging`,
`pio_switches`, `pio_bus_demand`, `pio_bus_shunt` → `pio_balanced_*`.

| v4 | v5 | | why |
|---|---|---|---|
| `pio_n_gens` | `pio_balanced_n_generators` | R | the only abbreviated noun; its own summary JSON already said `generators` |
| `pio_gens` | `pio_balanced_generators` | R | same |
| `pio_bus_ids` | `pio_balanced_bus_ids` | R S | **reports the star-lowered space**, so every per-bus array has one length and every row has an id |
| `pio_n_buses` | `pio_balanced_n_buses` | R S | same space as the extractors; closes the trap `powerio.h:32-38` documented |
| `pio_source_format` | `pio_balanced_source_format` | R S | returned the Rust enum `Debug` spelling (`PowerModelsJson`), a different alphabet from the format tokens every other symbol takes; returns the token |
| `pio_summary_json` | `pio_balanced_summary` | R S | rule 3; and `source_format` inside it agrees with the above |

### Arrow (2 → 2)

| v4 | v5 | | why |
|---|---|---|---|
| `pio_to_arrow` | `pio_balanced_to_arrow` | R | rule 1 |
| `pio_arrow_catalog_json` | `pio_arrow_catalog` | R S | rule 3 |

Arrow table ids stay append-only and unchanged. That promise predates v5 and
survives it.

### Package (18 → 18)

`pio_package_*` already named its subject. Every one is `S` for rule 3; four
change more:

| v4 | v5 | | why |
|---|---|---|---|
| `pio_package_from_balanced_network` | `pio_package_from_balanced` | R S | the handle is the network; saying it twice added nothing |
| `pio_package_from_multiconductor_network` | `pio_package_from_multiconductor` | R S | same |
| `pio_package_to_balanced_network` | `pio_package_to_balanced` | R S | same |
| `pio_package_to_multiconductor_network` | `pio_package_to_multiconductor` | R S | same |
| `pio_package_lower_multiconductor_to_balanced` | `pio_package_to_balanced_model` | R S | "lower" is compiler-IR vocabulary, not user vocabulary; 44 chars → 27 |
| `pio_package_multiconductor_to_balanced_preflight_json` | `pio_package_to_balanced_check` | R S | "preflight" is an internal stage name; 52 chars → 29, the longest symbol in the ABI |

The rest — `parse_file`, `parse_str`, `free`, `to_json`, `validate`,
`validation_json`, `diagnostics_json`, `operating_points_json`,
`set_operating_points`, `study_json`, `materialize_operating_point`,
`materialize_study_commit` — keep their names and lose the `_json` suffix where
rule 3 makes the return type say it.

### Geographic (3 → 3)

| v4 | v5 | | why |
|---|---|---|---|
| `pio_geo_parse` | `pio_geo_normalize` | R S | the only `parse` in the ABI that returned a string rather than a handle, so it broke the grammar's own verb rule |
| `pio_geo_extract` | `pio_balanced_geo_extract` | R S | rule 1 |
| `pio_geo_apply` | `pio_balanced_geo_apply` | R | rule 1 |

### Problem instances (6 → 6)

| v4 | v5 | | why |
|---|---|---|---|
| `pio_scopf_parse_str` | same | S | rule 3 |
| `pio_scopf_to_json` | same | S | **0-based**, matching `pio_acopf_to_json`; a C ABI that hands out 1-based indices is surprising, and PowerIO.jl is where the Julia convention belongs |
| `pio_scopf_instance_free` | `pio_scopf_free` | R | `_instance_` appeared in exactly two symbols and repeated the handle type |
| `pio_acopf_from_network` | `pio_acopf_from_balanced` | R | rule 1 |
| `pio_acopf_to_json` | same | S | rule 3 |
| `pio_acopf_instance_free` | `pio_acopf_free` | R | as above |

`pio_acopf_*` is absent from the v4 header preamble, the ownership list, and
`docs/src/capi-arrow.md`. v5 documents it.

### Multiconductor model (15 → 15)

Every `pio_dist_*` becomes `pio_multiconductor_*` (rules 1, 2) and every
string-returning one is `S` for rule 3:

`parse_file`, `parse_str`, `free`, `warnings`, `summary_json` →
`summary`, `to_json`, `graph_json` → `graph`, `from_json`, `to_format`
(returns `PioConversion *`), `geo_extract`, `geo_apply`.

`pio_dist_convert_file` and `pio_dist_convert_str` are removed for the same
reason as their balanced twins.

The EXPERIMENTAL banner is retired. The dist C signatures were never the
unstable part; the BMOPF payload schema was, and it is versioned where it
lives.

### Memory (6 → 6)

| v4 | v5 | | why |
|---|---|---|---|
| `pio_network_free` | `pio_balanced_free` | R | rule 1 |
| `pio_dist_network_free` | `pio_multiconductor_free` | R | rules 1, 2 |
| `pio_package_free` | same | = | |
| `pio_scopf_instance_free` | `pio_scopf_free` | R | above |
| `pio_acopf_instance_free` | `pio_acopf_free` | R | above |
| `pio_string_free` | — | X | rule 3 removes the class it freed |

### New in v5 (3)

| symbol | why |
|---|---|
| `pio_conversion_text` | rule 4 |
| `pio_conversion_warnings` | rule 4; this is the symbol whose absence lost warnings |
| `pio_conversion_free` | rule 4 |

## Counts

84 v4 symbols → 78 v5 symbols. 7 removed, 3 added, 68 renamed, 41
re-signatured, 5 unchanged. Seven disposal paths become five, and the header
documents all of them (v4 documented five of seven and omitted
`pio_acopf_instance_free` entirely).

## What does not change

Arrow table ids and column order. The format tokens, which are strings and were
never symbols. `pio_from_json` / `pio_to_json` as a concept. The opaque-handle
design, the panic guard on every entry point, and `errbuf`/`errlen` last.

## What this costs PowerIO.jl

Every symbol is resolved by `dlsym` at runtime, so a rename is a load-time
failure rather than a silent misread — with one v4 precedent worth remembering:
`pio_convert_file` kept its symbol, arity, and types while reordering two
arguments, which linked fine and read the formats reversed. That is why the
handshake is not optional, and why v5 changes a name whenever it changes
behavior.

A companion PowerIO.jl branch tracks this one so `julia-binding.yml` keeps both
sides green.
