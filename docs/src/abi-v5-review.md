# A. The universal grammar

```
pio_<subject>_<operation>[_<qualifier>]
```

**Subjects** — closed. The subject is the handle type the function takes or returns. Empty means the library itself or a free function over bytes.

```
(empty)          the shared object, or a free function over bytes
balanced         PioBalancedNetwork
multiconductor   PioMulticonductorNetwork
package          PioPackage
scopf            PioScopfInstance
conversion       PioConversion
source           PioSource
geo              a geographic layer (no handle; free function)
```

**Operations** — exactly one of six forms. Only forms 1, 2, 5 and 6 use verbs, and those verbs are closed.

```
1 CONSTRUCTOR   returns a new handle; errbuf/errlen last; NULL on failure
                verbs: read | parse | from_json | from_<subject> | from_source
                     | normalize | lower_to_<subject> | apply_<noun> | write | open

2 DESTRUCTOR    free    one per subject; void; idempotent on NULL

3 EMITTER       size_t f(handle, out, cap, errbuf, errlen)
                NULL/0 = size query; returns the total excluding the NUL;
                truncates on a UTF-8 boundary
                to_json      the subject itself, serialized (an inverse exists)
                <payload>    a derived report, no inverse: summary warnings graph
                             validation diagnostics operating_points study geo
                             text file catalog build_info

4 ACCESSOR      spelled by what comes back, no verb:
                  n_<plural>            -> size_t count
                  <plural>              -> typed array extractor, (out.., cap) -> total
                  <singular>            -> one scalar, or one fixed tuple of arrays
                  is_<adj> / has_<noun> -> int32_t, 0 false, nonzero true

5 MUTATOR       verbs: validate | set_<noun> | materialize_<noun>
                PioPackage only; invalidates nothing because PioPackage never caches

6 OUT-PARAM     int32_t f(handle, ..., <out struct pointers>, errbuf, errlen)
                for payloads C cannot express as bytes in a caller buffer:
                to_arrow (Arrow C Data Interface), find (index lookup)
```

**Qualifier** — closed, currently one member: `_check`, a preflight for the operation named by the stem (`pio_package_lower_to_balanced` / `pio_package_lower_to_balanced_check`). Nothing else may occupy the slot without an entry in this list.

**Two invariants that are not name slots.** `errbuf, errlen` are the last two parameters of every fallible entry point, and `errbuf[0]` is set to `'\0'` on entry. Every entry point is panic-guarded and returns its documented failure value.

**The rule that makes the grammar predictable, and the two abbreviation disputes it settles.** `read` takes a path; `parse` takes bytes in memory. That one distinction retires `_file`, `_str`, `_bytes`, `_dir`, `_dataset` and `_scenario` from the surface — the storage shape belongs to the format, and the argument type already says whether input is a path or memory. Where the designers disagreed on `pio_bal_`/`pio_mc_` versus the long forms, take the long forms, but not on the "no abbreviations" argument as written: the Monte Carlo claim does not survive checking (psspy has zero Monte Carlo entry points; `mc` appears only in `mcre`), and the NERC BAL restatement is weaker than the original. The argument that holds is namespace reservation plus greppability: `balanced_network` and `multiconductor_network` are the `.pio.json` payload keys (`powerio-pkg/src/model.rs:35-42`), the only Rust spellings since `1bf7c630` deleted the shims, the Python class names and the Julia exported types. A C header that cannot be grepped by the word the wire format uses is the one surface out of step. The measured cost is 3.9 characters of mean symbol length and zero characters of maximum; v5 at mean 22.7 sits under cairo's 24.0. Separately, `dist` is spent: psspy ships 23 `dist_*` functions and every one means *disturbance* (`PowerMCP/PSSE/psspy_command_json/dist_bus_fault_3.json`, category "Set Disturbance").

# B. The revised symbol table

Disposition: `=` unchanged · `R` renamed · `S` re-signatured · `X` removed · `N` new.

## Handshake — 7 → 4

| v4 | v5 | disp | reason |
|---|---|---|---|
| `pio_abi_version` | `pio_abi_version` | = | the gate; callable before you trust anything else |
| `pio_version` | `pio_version` | = | build identity; feeds PowerIO.jl's repin lineage gate, which an ABI integer cannot |
| `pio_has_feature` | `pio_has_feature` | = | cheap path for a caller with no JSON parser |
| `pio_schema_versions_json` | — | X | three keys, none unique: `abi`, `powerio_version` and `bmopf_schema` all duplicate another symbol |
| `pio_dist_capabilities_json` | — | X | a dist-gated symbol is the wrong home for the fact rule 6 depends on |
| `pio_dist_abi_version` | — | X | rule 6: a second integer cannot express foreign-schema drift |
| `pio_matrix_available` | — | X | the `arrow ∧ matrix` conjunction belongs to the caller; no other feature pair gets a symbol |
| — | `pio_build_info` | N | one report: `powerio_version`, `abi`, `features[]`, `capabilities[]`, `schemas{}` |

`pio_build_info`'s `schemas` is keyed by format token on both models — `bmopf`, `surge`, `pandapower`, `powermodels` — each with `{id, writes, reads}`, `reads: null` meaning version-agnostic. That is what makes rule 6's claim true; a scalar does not. `capabilities` is a string array in curl's `feature_names` shape, so PowerIO.jl's documented cross-release probe (`docs/src/distribution.md:24-31`) survives as `"bmopf.typed_capacitors" in build_info().capabilities`, and a new capability costs one array element rather than a key or a symbol.

## Balanced model — 35 v4 rows → 31 symbols

| v4 | v5 | disp | reason |
|---|---|---|---|
| `pio_parse_file` | `pio_balanced_read` | R S | rule 1; `_file` is false for PyPSA directories, which already enter through it |
| `pio_parse_str` | `pio_balanced_parse` | R S | `(const void *, size_t)`, so `.pwb` and any future binary format are reachable without a temp file |
| `pio_from_json` | `pio_balanced_from_json` | R S | rule 1 + rule 3 |
| `pio_read_dir` | `pio_balanced_from_source` | R S | one entry out of an opened dataset; see the source family |
| `pio_scenario_ids` | `pio_source_entry_ids` | R S | re-homed onto the handle; keeps `int64_t` typing |
| `pio_classify_str` | `pio_classify` | R S | now takes bytes, so `_str` would be a lie; gains an error channel it never had |
| `pio_normalize` | `pio_balanced_normalize` | R S | takes `const PioNormalizeOptions *`; NULL means defaults |
| `pio_normalize_with_options` | — | X | folded; one door |
| `pio_to_json` | `pio_balanced_to_json` | R S | the model's own JSON, no fidelity channel |
| `pio_to_format` | — | X | folded into `pio_balanced_write` with a NULL path |
| `pio_write_dir` | — | X | folded into `pio_balanced_write`; it wrote exactly one format (PyPSA CSV), which is a case, not a dataset |
| `pio_convert_file` | — | X | three same-typed strings; it shipped with two reversed and still linked |
| `pio_convert_str` | — | X | same |
| — | `pio_balanced_write` | N | one write verb; `path` NULL serializes into the handle, non-NULL writes a file, a directory, or a file plus sidecars |
| `pio_network_free` | `pio_balanced_free` | R | rule 1 |
| `pio_to_arrow` | `pio_balanced_to_arrow` | R | form 6; ownership handling is already correct |
| `pio_n_buses` | `pio_balanced_n_buses` | R S | rule 1; moves to the star-lowered bus space so `length(bus_ids) == n_buses` |
| `pio_n_branches` | `pio_balanced_n_branches` | R | rule 1 |
| `pio_n_gens` | `pio_balanced_n_generators` | R | rule 1; no abbreviation |
| `pio_n_switches` | `pio_balanced_n_switches` | R | rule 1 |
| `pio_n_islands` | `pio_balanced_n_islands` | R | rule 1 |
| `pio_base_mva` | `pio_balanced_base_mva` | R | rule 1 |
| `pio_network_name` | `pio_balanced_name` | R S | the handle is the network; saying it twice adds nothing (the audit's own rule at `:259-262`) |
| `pio_source_format` | `pio_balanced_source_format` | R S | rule 1 + rule 3 |
| `pio_is_radial` | `pio_balanced_is_radial` | R | rule 1 |
| `pio_ref_bus_index` | `pio_balanced_ref_bus_index` | R | rule 1 |
| `pio_ref_bus_indices` | `pio_balanced_ref_bus_indices` | R | rule 1 |
| `pio_bus_ids` | `pio_balanced_bus_ids` | R S | rule 1; star-lowered space, matching `n_buses` |
| `pio_bus_demand` | `pio_balanced_bus_demand` | R | rule 1; documented exception to the accessor rule (singular noun, two arrays) |
| `pio_bus_shunt` | `pio_balanced_bus_shunt` | R | same exception |
| `pio_branches` | `pio_balanced_branches` | R | rule 1 |
| `pio_branch_charging` | `pio_balanced_branch_charging` | R | rule 1; same singular/two-array exception |
| `pio_switches` | `pio_balanced_switches` | R | rule 1 |
| `pio_gens` | `pio_balanced_generators` | R | rule 1; no abbreviation |
| `pio_warnings` | `pio_balanced_warnings` | R | rule 1; already rule-3 shaped |
| `pio_summary_json` | `pio_balanced_summary` | R S | the `_json` suffix names the encoding, not the payload |
| `pio_geo_extract` | `pio_balanced_geo` | R S | form 3 emitter; `extract` is not a verb in any list |
| `pio_geo_apply` | `pio_balanced_apply_geo` | R S | form 1 constructor; gains a `PioGeoApplyReport *` out-param instead of an English sentence in the warnings |

## Source family — new, 5 symbols

| v4 | v5 | disp | reason |
|---|---|---|---|
| — | `pio_source_open` | N | opens a dataset once; the gridfm Parquet columns are read once, not once per scenario |
| — | `pio_source_count` | N | infallible because open is eager |
| — | `pio_source_entry_ids` | N | `int64_t *` out, cap/total idiom; preserves PowerIO.jl's integer-keyed public signature |
| — | `pio_source_find` | N | `(src, int64_t id, size_t *index)`; no binding writes a decimal strcmp loop |
| — | `pio_source_free` | N | one disposal |

`pio_balanced_from_source(src, index, errbuf, errlen)` is counted under the balanced family. A network obtained from a source owns its data outright and outlives the `PioSource`; the two have no free ordering. `PioReadOptions` ships with the directory-walk caps defined but reserved, so corpus ingestion is a `Route` arm later, not a new symbol. `pio_balanced_read` on a multi-entry source fails with a message naming `pio_source_open` — the shortcut can never silently disagree with the container form. This is a departure from GDAL/FFmpeg/Arrow/libgit2, all of which make open-then-enumerate the only path; say so plainly in the document rather than claiming their endorsement.

## Conversion — new, 5 symbols

| v4 | v5 | disp | reason |
|---|---|---|---|
| — | `pio_conversion_text` | N | rule 4: a conversion has text *and* warnings, both needing a size query |
| — | `pio_conversion_warnings` | N | replaces the truncating `warnbuf` that `finish_conversion` never sized |
| — | `pio_conversion_n_files` | N | count/index, not a `\n`-joined list — `\n` is a legal byte in a POSIX filename |
| — | `pio_conversion_file` | N | what actually landed on disk, including OpenDSS sidecars the ABI currently drops |
| — | `pio_conversion_free` | N | one disposal |

## Multiconductor model — 13 → 11

| v4 | v5 | disp | reason |
|---|---|---|---|
| `pio_dist_parse_file` | `pio_multiconductor_read` | R S | rule 2 |
| `pio_dist_parse_str` | `pio_multiconductor_parse` | R S | bytes; also fixes the stale doc at `lib.rs:2345` claiming includes resolve against the cwd |
| `pio_dist_network_free` | `pio_multiconductor_free` | R | rule 2 |
| `pio_dist_warnings` | `pio_multiconductor_warnings` | R | rule 2 |
| `pio_dist_summary_json` | `pio_multiconductor_summary` | R S | drop `_json` |
| `pio_dist_graph_json` | `pio_multiconductor_graph` | R S | drop `_json` |
| `pio_dist_to_json` | `pio_multiconductor_to_json` | R S | the pair was always symmetric; the audit's table layout hid it |
| `pio_dist_from_json` | `pio_multiconductor_from_json` | R S | same |
| `pio_dist_to_format` | — | X | folded into `pio_multiconductor_write` |
| `pio_dist_convert_file` | — | X | see `pio_convert_file` |
| `pio_dist_convert_str` | — | X | same |
| `pio_dist_geo_extract` | `pio_multiconductor_geo` | R S | form 3 |
| `pio_dist_geo_apply` | `pio_multiconductor_apply_geo` | R S | form 1 |
| — | `pio_multiconductor_write` | N | one write verb, sidecars included |

## Package — 18 → 18

| v4 | v5 | disp | reason |
|---|---|---|---|
| `pio_package_parse_file` | `pio_package_read` | R S | one ingest verb pair |
| `pio_package_parse_str` | `pio_package_parse` | R S | bytes; the `_str` suffix goes with the argument type |
| `pio_package_free` | `pio_package_free` | = | no buffer, no errbuf, nothing to change |
| `pio_package_to_json` | `pio_package_to_json` | S | rule 3 only |
| `pio_package_from_balanced_network` | `pio_package_from_balanced` | R S | the handle is the network |
| `pio_package_from_multiconductor_network` | `pio_package_from_multiconductor` | R S | same |
| `pio_package_to_balanced_network` | `pio_package_to_balanced` | R S | same |
| `pio_package_to_multiconductor_network` | `pio_package_to_multiconductor` | R S | same |
| `pio_package_lower_multiconductor_to_balanced` | `pio_package_lower_to_balanced` | R S | 44 → 29; keeps `lower`, which `.pio.json` publishes as `lowering_history` |
| `pio_package_multiconductor_to_balanced_preflight_json` | `pio_package_lower_to_balanced_check` | R S | 53 → 35; `_check` attaches to the stem it preflights |
| `pio_package_validate` | `pio_package_validate` | S | rule 3 errbuf clause |
| `pio_package_validation_json` | `pio_package_validation` | R S | drop `_json` |
| `pio_package_diagnostics_json` | `pio_package_diagnostics` | R S | drop `_json` |
| `pio_package_operating_points_json` | `pio_package_operating_points` | R S | drop `_json` |
| `pio_package_study_json` | `pio_package_study` | R S | drop `_json` |
| `pio_package_set_operating_points` | `pio_package_set_operating_points` | S | rule 3 errbuf clause |
| `pio_package_materialize_operating_point` | `pio_package_materialize_operating_point` | S | rule 3; still the longest symbol in the ABI at 39 |
| `pio_package_materialize_study_commit` | `pio_package_materialize_study_commit` | S | rule 3 |

`pio_package_to_balanced` and `pio_package_lower_to_balanced` must not be `to_balanced` and `to_balanced_model`: they return different handle types under mutually exclusive preconditions (`lib.rs:1629` vs `:1977`, precondition split at `:1643-1646`), and a one-suffix difference between them is the worst confusion in the drafted table.

## Problem instances — 6 → 3

| v4 | v5 | disp | reason |
|---|---|---|---|
| `pio_scopf_parse_str` | `pio_scopf_parse` | R S | one ingest verb pair; 0-based indices, PowerIO.jl converts back |
| `pio_scopf_to_json` | `pio_scopf_to_json` | S | rule 3; the handle's `OnceLock` is what keeps the two-call idiom from re-parsing a GOC3 document |
| `pio_scopf_instance_free` | `pio_scopf_free` | R | the type is the subject |
| `pio_acopf_from_network` | — | X | its objective is quadratic-only (`powerio-prob/src/ac.rs:101-107`) and it carries no storage (`:155-169`), so it cannot serve the one consumer it was cut for; PowerIO.jl routed around it into 836 lines of `exa.jl` |
| `pio_acopf_to_json` | — | X | same; a JSON-only door nobody walked through |
| `pio_acopf_instance_free` | — | X | goes with its handle |

The freeze forbids breaking changes, not additions. Delete now, re-cut additively when a C consumer exists and can say what shape it needs.

## Geographic and Arrow — 4 → 3

| v4 | v5 | disp | reason |
|---|---|---|---|
| `pio_geo_parse` | `pio_geo_parse` | S | keep the verb; return `PioConversion *` so the tolerant reader's notes stop being thrown away (`lib.rs:2005-2006` admits it). Do **not** rename to `geo_normalize`: `normalize` is already taken by `pio_balanced_normalize`, and in GEOS `GEOSNormalize_r` canonicalizes a parsed geometry rather than reading a file |
| `pio_arrow_catalog_json` | `pio_arrow_catalog` | R S | drop `_json` |
| `pio_string_free` | — | X | the owned-`char *` idiom goes with it |

## Handles, structs, macros

| v4 | v5 | disp | reason |
|---|---|---|---|
| `PioNetwork` | `PioBalancedNetwork` | R | rule 1 |
| `PioDistNetwork` | `PioMulticonductorNetwork` | R | rule 2 |
| `PioPackage` | `PioPackage` | = | |
| `PioScopfInstance` | `PioScopfInstance` | = | |
| `PioAcopfInstance` | — | X | with the acopf trio |
| — | `PioConversion` | N | rule 4 |
| — | `PioSource` | N | one dataset read, many entries |
| — | `PioNormalizeOptions` | N | rule 5 |
| — | `PioReadOptions` | N | `struct_size`, `name_hint`, walk caps (reserved) |
| — | `PioWriteOptions` | N | `struct_size` only today; the slot exists so a write knob is not a v6 |
| — | `PioGeoApplyReport` | N | `struct_size` + five counts; tellegen's report has five, three bare out-params could not grow |
| `PIO_ABI_VERSION 4` | `PIO_ABI_VERSION 5` | S | |
| `PIO_DIST_ABI_VERSION` | — | X | rule 6 |
| `PIO_ERRBUF_MIN` | `PIO_ERRBUF_MIN` | = | |
| `PIO_ARROW_TABLE_*` (21) | unchanged (21) | = | append-only ids, frozen column order |
| — | `PIO_NORMALIZE_OPTIONS_MIN_SIZE` | — | **not shipped.** `offsetof` is 16 on LP64 and 8 on ILP32, so a cbindgen integer is wrong on one; and the only value a caller legitimately writes is `sizeof`. Delete rather than fix |

**Counts.** Functions 84 → 80 (12 removed, 8 added). Handles 5 → 6. Structs 0 → 4. Macros 24 → 23. Total names 113 → 113. The audit's current arithmetic has three errors to fix while you are in there: only four `*_free` functions are double-recorded, not five (`pio_network_free` appears only in the Memory table); v4's longest symbol is 53 characters, not 52; and v5's longest is `pio_package_materialize_operating_point` at 39, not "about 31" — it is on the keep list, which is why the measurement missed it.

# C. The owner's questions

**`mc` / `bal`.** You are right about `mc` and the document is wrong: nobody reads `pio_mc_parse` as Monte Carlo, and psspy — 2184 API functions, the most-used power systems scripting surface in industry — has zero Monte Carlo entry points and zero `bal*` functions. Do not keep the claim as written. The argument that survives is reservation, not misreading: `powerio-prob` already ships scenario machinery and the table already has `pio_source_entry_ids` over scenario sets, so `mc_` is a prefix powerio itself may want, and a balancing area is a live network-partition concept (interchange, area control error) that a power API could plausibly expose. A prefix spent on a model family cannot be spent twice over a five year freeze. The NERC BAL-001…BAL-005 restatement is a weaker claim than "balancing area" and should be at most a subordinate clause.

**`dist` as a word versus the `powerio-dist` crate name.** No tension worth a paragraph. A package name is not a symbol prefix — libcurl exports `curl_easy_*` and nobody expects `libcurl_*`. `powerio-dist` names the package that implements the multiconductor model, which is true and costs nothing. But correct one fact before you write it down: the C ABI is not the last surface speaking `dist`. `powerio_dist`'s own Rust API is, and tellegen imports it by name (`tellegen-wasm/src/dist.rs:17`, `use powerio_dist::{parse_str, CoordinateSpace, DistGraphEdgeKind, GeoMeta, MulticonductorNetwork}`, pinned at `powerio-dist = "0.7.1"`). Of the 20 `Dist*` public types, 12 are element structs with no external consumer, 6 are the `DistGraph*` projection tellegen imports, and 2 are format enums. File the element structs as a 1.0 rename; leave the graph types alone or do them with a companion tellegen PR. None of it blocks v5.

**Rule 3 in plain language.** A C function returning a string of unknown length has two choices. It can allocate inside the library and hand you the pointer, which means you must call its free function — that is `pio_string_free`, and 27 v4 symbols worked that way. Or it can ask you for the buffer, which means calling twice: once with `(NULL, 0)` to learn the length, once with a buffer that big. v4 shipped both, plus a third variant for arrays, so a caller had to remember which of three rules applied to each of 84 functions. v5 keeps the second and third and deletes the first. The headline "the library never allocates" is false — every handle is library-allocated, and `PioConversion` is freed by `pio_conversion_free`. Say what is actually promised: **no raw pointer to library memory crosses the boundary.** Everything that crosses is either bytes copied into memory the caller already owns or an opaque handle with exactly one disposal function. Do not use the Windows CRT-mismatch story as the rationale; it does not describe v4, which pairs its allocation with `pio_string_free`, the textbook mitigation. The real payoff is that an ownership rule and a binding leak path disappear: PowerIO.jl's `_take_string` (`capi.jl:420-424`) does `unsafe_string` then a `pio_string_free` ccall, and any error between them leaks.

**Rule 5's precedents.** Half accurate. Win32 `NOTIFYICONDATA` really is a leading-size struct used for version selection, verbatim: "you must initialize the structure with its size. If you use the size of the currently defined structure, the application might not run with earlier versions of Shell32.dll." But that page also tells the caller to call `DllGetVersion` and pick a matching size — Shell32 dispatches on a set of known exact sizes rather than clamping, which is the defect your clamp fixes. Lead rule 5 with the Linux extensible-syscall convention instead (`openat2`'s `struct open_how`, `clone3`, `sched_setattr`), which specifies both directions in exactly your terms and also requires the caller to zero-fill, matching `:123`. Keep Win32 as the older, coarser citation. The Vulkan and `sqlite3_config` rejections at `:107-109` are correct; Vulkan has no size-led struct. Three things are missing and all three matter: an append-only clause (fields may only be appended — no reorder, no removal, no type change, no field whose alignment exceeds the struct's current maximum), a no-implicit-padding rule with explicit named reserved fields, and `openat2`'s E2BIG behavior so a 2029 caller against a 2027 library gets a loud error instead of silently unapplied options. Adopt E2BIG only *with* the padding rule — `size_t / int32_t / double` as sketched carries four bytes of implicit padding, and Julia zeroes fields, not padding, so an unqualified tail scan would reject correct callers.

**Rule 7, and whether one auto-routing entry point is safe.** Rule 7 says: do not name a read symbol after the storage shape of its input, because the shape is not stable across formats. `_file` asserts one file per case and that is already false — a PyPSA case is a directory and enters through `parse_file` today (`powerio/src/format/mod.rs:440`), while `pio_read_dir` is named for a directory but reads gridfm. Yes, one entry point can route safely, and the routing already exists in three places: `powerio::parse_file`, `powerio_dist::parse_file`, `powerio_matrix::read_dataset_dir`. Nothing sits above all three, which is why the C ABI is where the unified entry belongs. Put `powerio::routing::resolve_path(path, from) -> Route` in the `powerio` crate and let the C ABI, the CLI and the bindings share it. `from` stays optional with declared-wins semantics — cite FFmpeg for that ("If non-NULL, this parameter forces a specific input format. Otherwise the format is autodetected"), not GDAL, whose `papszAllowedDrivers` only restricts the candidate set and still sniffs. Inference must stay structural and cheap: extension for files, one marker file for a directory, content markers only for `.json`. Never probe readers against a file to identify it — that is the amplification pattern GDAL documents as its own worst security property. Specify the two-stage failure explicitly: `resolve_path` answers "what is this", and each model's read symbol rejects a Route that is not its own with the existing guidance messages, so pointing the balanced reader at a `.dss` says "use the multiconductor entry point" rather than "unknown format".

**`pio_abi_version` versus `pio_version`.** Keep both; neither derives from the other. The integer is the gate, checked once before any other ccall (`PowerIO.jl/src/capi.jl:175-192`), and it changes only on a break. The string is identity and changes every release. Without the integer, every binding independently encodes powerio's compatibility rule out of semver and every patch looks like a possible break. Without the string, the artifact repin gate has nothing to compare: 0.8.3 and 0.9.0 binaries both legitimately report ABI 4, and one of them writes documents the binding cannot read. Drop the SQLite and curl citations that were offered for this — `sqlite3_libversion`/`libversion_number` and curl's `version`/`version_num` are one fact in two spellings, which is the redundancy you are deleting elsewhere. Cite Vulkan's `apiVersion`/`driverVersion`, and rest the argument on powerio's own distribution: PowerIO.jl resolves everything by `dlsym` out of a pinned artifact with no soname discipline, so `pio_abi_version` *is* powerio's soname.

**`pio_document_versions`.** It is the rename of `pio_schema_versions_json`, and it should not exist. Measured from a built cdylib, that function returns `{"abi":4,"bmopf_schema":"0.1.0","powerio_version":"0.8.3"}`. `abi` duplicates `pio_abi_version()`, `powerio_version` duplicates `pio_version()` byte for byte (shared workspace version), and `bmopf_schema` duplicates the capabilities document. It earned its keep when it reported a table of per-document lineages; the `.pio.json` version collapse killed the table, and the capi's own test now asserts the per-document keys are gone rather than renamed (`lib.rs:4900`). A report of a one-element table is the element. Delete it, do not ship the rename, and move the one irreducible payload — the foreign schema versions — into `pio_build_info`. The word "document" is not invented (`powerio/src/version.rs:32-34` uses it for a powerio-authored artifact), but the name promises a per-document table, and the symbol that actually reports per-document schema versions is `pio_arrow_catalog`.

**Why multiconductor has capabilities.** Because BMOPF is a foreign schema powerio reproduces and whose vintage is fixed at compile time and stamped into every file as `$schema` — curl's `ssl_version`/`libz_version` case. But the fourteen-key document is three things stapled together: ten fidelity booleans, two duplicates of other symbols, and two irreducible strings. And the stated asymmetry is wrong: `surge.rs:24` pins `SCHEMA_VERSION = "0.1.0"` and its reader *hard-refuses* a mismatch, `pandapower.rs:864` writes `"version": "3.0.0"`, `powermodels.rs:132` writes `"source_version": "2"` — three build-fixed foreign vintages on the balanced side, one of them stricter than BMOPF, whose reader ignores schema versions entirely. Delete the symbol, keep the information: `schemas` in `pio_build_info` keyed by format token across both models, and `capabilities` as an enumerable string array. The flags are not dead weight — PowerIO.jl documents them as the cross-release probe that tells "the case has none" from "the library predates the table" (`docs/src/distribution.md:24-31`), and replacing that with semver comparisons is exactly the inference `pio_abi_version` exists to prevent.

**The four parse symbols.** After the read/parse split there are two ingest verbs, not four spellings. `read` takes a path and routes on structure; `parse` takes `(const void *, size_t)` and requires `from`. Per model that is two symbols, plus `pio_package_read`/`pio_package_parse` and `pio_scopf_parse`. `_file`, `_str`, `_dir`, `_dataset` and `_scenario` all disappear, because the argument type already carries the distinction the suffix was trying to carry.

**`parse_str` versus binary formats.** `parse_str` is the no-filesystem entry point, and it is the one with the strongest security posture: the DSS variant disables includes entirely (`powerio-dist/src/dss/read.rs:129-141`), which is the 0.7.3 advisory fix. It survives, but the signature must change. A `.pwb` cannot be a C string — `parse_file` reads it with `std::fs::read`, not `read_to_string` (`mod.rs:454` against `:515`), and a NUL byte truncates it at the boundary. Parquet cannot either, and gridfm needs a directory plus a manifest, so it has no in-memory form at all. Length-delimit it, and give it `const PioReadOptions *` carrying `name_hint` from day one — `parse_str_with_name` (`mod.rs:704`) exists precisely because bytes carry no file stem, and `.pwb` needs it most, since `parse_file` passes the stem into `parse_pwb`.

**`from_json` / `to_json` asymmetry.** There is none. `pio_dist_to_json` (`lib.rs:2461`) and `pio_dist_from_json` (`:2504`) both exist; the audit lists them in prose at `:311-313` instead of in a table. Fix the table layout — the asymmetry you saw is a real documentation defect even though the symbols are symmetric. What the pair *is* deserves one line: the model's own JSON, the same object a `.pio.json` package carries under `model`, no format token, no fidelity warnings because none can arise. That is the bindings' transport, distinct from `to_format(net, "powerio-json")`, which the Rust source already marks `#[doc(hidden)]` as a "Compatibility alias for `BalancedNetwork::to_json` and `from_json`". Delete the `powerio-json` / `powerio` / `json` tokens from what the C read and write symbols accept — but settle all three channels first, not just the explicit one: inference on a `.json` whose content is model JSON must reach the same rejection with the same message (the way `sniff_json`'s Package arm already does at `mod.rs:646-651`), and `pio_classify` must return a distinct token documented as not a case format rather than one the read symbols refuse.

**`write_dataset` and `convert_file`.** `write_dataset` is the worst of the names available and it is factually wrong: the symbol it renames writes exactly one format, PyPSA CSV, and powerio's own source says "PyPSA CSV directories are case inputs, not datasets" (`powerio-matrix/src/io/mod.rs:24`). The one thing that genuinely is a dataset write, `write_gridfm_dataset`, is not in the C ABI at all. `write_dir` is honest but names the storage shape, which rule 7 spent a page arguing against. Use one write verb per model where `path` is where output goes, whatever shape the format wants; `pio_conversion_file` reports what landed. That also closes a live consumer bug: OpenDSS sidecars are currently dropped and converted into a warning string (`lib.rs:2519-2531`), so BMOPFTools emits a `.dss` referring to a Buscoords file the user does not have. `convert_*` should go, all four, but for the right reason. Not "C cannot express a two-field return" — rule 4 introduces `PioConversion` precisely so it can, and writing that sentence into a five year document would mislead the next maintainer. The reason is that a call taking path, from and to as three same-typed strings can be transposed without a link error, which is exactly the v4 `pio_convert_file` bug; read-then-write takes one format token per call against a differently typed handle. Rust and Python keep `convert_*` — their arguments are named and their return is typed, and PowerMCP calls `powerio.convert_file(file_path, "egret-json", source_format)` today.

**The `_json` suffix on `summary`.** Drop it everywhere. The suffix names the encoding of a payload whose identity is "the summary", and no symbol in the ABI returns a summary in any other encoding. Same for `validation`, `diagnostics`, `operating_points`, `study`, `graph`, `catalog`.

**The Arrow asymmetry.** Not a gap and not dead weight: there is nothing to export. `arrow_export.rs` is 1988 lines in which the words `dist` and `multiconductor` appear zero times, all 21 table ids are balanced or matrix, and `powerio-dist` carries no arrow dependency at all. Keep `pio_balanced_to_arrow` alone and add one sentence, but not the raggedness argument — Arrow has `List` for that, and the id space is not what a premature table would spend. The true reason: the balanced model is the one with a settled columnar layout and a consumer, while the multiconductor model's only consumers read a graph (tellegen) or a document (BMOPFTools); a table id is cheap to add later, but a shipped table's *column order* is frozen (`arrow_export.rs:54-60`), so the cost of guessing lands on the columns. Say the same for the absent multiconductor `normalize` and typed extractors.

**`geo_normalize` versus `geo_parse`, and what tellegen needs.** Keep `parse`. The symbol parses (buscoords CSV, aliased CSV/JSON, GeoJSON) and then serializes because there is no geo handle to return; the canonicalization is incidental output encoding, not the operation. Three reasons the rename is wrong: `normalize` is already the per-unit/radian transform in the same header, which breaks rule 2 with rule 1's fix; in the geospatial domain `GEOSNormalize_r` canonicalizes an already-parsed geometry while readers are named `_read`; and "a parse must return a handle" is not a rule anyone follows — curl's URL parser is tolerant-in, canonical-string-out and is still the parser. The real defect is that parse throws the reader's notes away, which the doc comment admits (`lib.rs:2005-2006`) while the apply path keeps them (`:2075`). Return `PioConversion *`: zero new symbols, the notes come back through `pio_conversion_warnings`, and the geo family obeys rule 4 for free. What tellegen needs is on the record because it already built it against the Rust API: parse returns `{layer, warnings, n_points, n_routes}` and apply returns `matched_buses`, `matched_branches`, `unmatched_features`, `notes` (`tellegen-wasm/src/geo.rs:39-44`, `:156-163`), which the frontend renders as "2 of 3 buses placed". The C surface gives neither. Give apply a `PioGeoApplyReport *` under rule 5 rather than bare out-params — the Rust report already has five counts (`lib.rs:2084-2093`), and three frozen `size_t *` could never grow to five. Note that tellegen is not a migration cost: it links the Rust crates and touches no `pio_` symbol. It is the specification.

# D. What changes outside the design document

**powerio, code.**
- `powerio-capi/src/lib.rs` — every signature; delete `pio_string_free`, `finish_string`'s owned path, `finish_conversion`'s `warnbuf`; add `PioConversion`, `PioSource`, the four structs, a sixth `assert_send_sync` pin for `PioConversion` and a seventh for `PioSource`; fix the stale panic rationale at `:146-147` (since Rust 1.81 an escaped panic aborts rather than being UB — the guards buy a documented failure value, not soundness); fix the stale `pio_dist_parse_str` doc at `:2345`; add `const _: () = assert!(offset_of!(...))` pins for each struct in the style of `arrow_export.rs:60-83`; hand-edit the `c_header_abi_manifest_is_pinned` literal array (`:3040`) — it does not pick up new lines automatically.
- `powerio-capi/cbindgen.toml` — header preamble rewrite (`:41-83`): the rule-3 headline, the errbuf zeroing credited to curl's `CURLOPT_ERRORBUFFER`, the NUL/UTF-8-boundary guarantees stated as powerio's own, the panic sentence at `:80-83`, the ownership list (which currently omits `pio_acopf_instance_free`), and `PIO_ERRBUF_MIN`. No `trailer` block — a trailer lands outside the include guard.
- `powerio-capi/include/powerio.h` — regenerated, never hand-edited.
- `powerio-capi/examples/smoke.c` — the renamed calls, plus four `release == NULL` asserts after the existing release pairs at `:350`, `:366`, `:375`. The Arrow round trip is already there; do not add a second block.
- `powerio-capi/examples/header_cpp.cpp` — renamed calls.
- `powerio/src/format/routing.rs` — `resolve_path(path, from) -> Route`, shared by the C ABI, CLI and bindings.
- `powerio/src/format/pypsa.rs:392` — cap the unread-`.csv` warning list (32 plus "N more"); it is a full `read_dir` inside the path the design calls bounded.
- `powerio-dist`, `powerio-cli` — hoist `is_relative_component_path` (`powerio-cli/src/main.rs:1537`) so the CLI and the ABI share one sidecar containment check.

**powerio, CI and tests.**
- `scripts/capi-header-parity.sh`, `scripts/capi-header-regen.sh` — unchanged mechanically; they gate the regeneration.
- New: a grammar check that parses every `extern "C" fn pio_*` and fails on a name that does not decompose into subject + operation + qualifier from the closed lists. Land it after the table is final, or its exception list becomes the grammar.
- New: `scripts/capi-symbol-snapshot.sh` — `nm -D --defined-only ... | grep '^pio_' | sort` against a checked-in per-feature list, wired into the four ubuntu `c-abi` jobs in `.github/workflows/rust.yml` (`:144, :186, :213, :250`). Linux-only by construction; do not copy it into the release matrix.
- New: an exhaustive parameterized buffer-protocol test in the existing `lib.rs` test module — every buffer symbol × cap ∈ {0, 1, n−1, n, n+1} × out ∈ {NULL, buffer}, asserting the return equals the untruncated total, NUL termination within cap, valid UTF-8 prefix, and an untouched canary past cap. This beats a fuzz target: the input space is bounded, and it runs on every feature job with no nightly and no corpus.
- New: a scoped `cargo miri test -p powerio-capi` job (`--no-default-features` plus a filter; miri cannot run the arrow FFI paths or the case-file tests). It is the only proposal here that checks something the current tests cannot — `Box::from_raw` provenance across the five disposal sites and the `ptr::write` ownership move.
- `fuzz/` — add a `pypsa_csv` target. `parse_csv` (`powerio/src/format/pypsa.rs:1046-1080`) is a hand-rolled tokenizer over untrusted bytes, it is absent from both the target list and `fuzz/README.md`'s exclusion rationale, and v5 elevates PyPSA folders to a first-class entry point. Do **not** add `powerio-capi` to the fuzz crate.
- `.github/workflows/release-binaries.yml` — no change; it already builds `--features arrow,matrix,gridfm,dist,pkg,prob`.

**powerio, docs.**
- `docs/src/abi-v5-audit.md` — replaced by the grammar section plus this table.
- `docs/src/capi-arrow.md` — the catalog example is wrong in two ways today: it shows a per-table `powerio_version` that does not exist, while the header doc comment (`lib.rs:744-746`, copied into `powerio.h:744-747`) still claims a per-table `schema_version` that the one-version collapse deleted. Three sources, three shapes, only the code right. Fix both and add a test asserting the table-level field names.
- `docs/src/languages.md` — the name flip missed four rows in one table (`:123-127` still say `DistNetwork`) and `:31` still says `Network::from_json`.
- `docs/src/geo-and-display.md`, `docs/src/performance.md`, `docs/src/study-block.md`, `docs/src/pio-json-schema.md`, `README.md`, `AGENTS.md` — symbol references.
- `CHANGELOG.md` — no 0.9.0 section exists yet, and this release's user-visible surface (bus shunt conductance in DC OPF, five documents changing their version key, `.pio.json` schema, numerical guards altering solver behavior on ill-conditioned cases, the error-architecture split) is larger than any prior entry in the file.
- Prose hygiene while the file is open: this document is hard-wrapped at 78-79 columns (91 lines in that band), against the standing rule; 22% of its sentences exceed 25 words, with a 63-word sentence at `:42-46`; and `shape` appears as vague jargon seven times while `path` is used in two unrelated senses within one section. `format-fidelity.md` is worse by rate (45% over 25 words) and carries the banned word "contract" twice.
- `scripts/deprecated-inventory.sh` exists, is invoked by nothing, and would report zero — while `AGENTS.md:305` and `docs/src/dcopf-bundle.md:36` both claim `DcConvention::ReactanceOnly` is "deprecated in 0.9.0". Either add the `#[deprecated]` attribute or drop the claim, and wire the script into CI.
- The literal string `"0.9.0"` is baked into a runtime error at `powerio/src/version.rs:38` and asserted verbatim in four tests. If the release ships as anything else, all of them are silently wrong.

**PowerIO.jl** — carries the entire migration. 95 `ccall`s across 12 files; roughly 103 symbol reference sites over 73 distinct symbols once you count both spellings (`:pio_x)` misses the `ccall((:pio_x, _lib()), ...)` form, 14 sites); about 98 of them edited. 22 `_take_string` sites become `_string_from`; `_take_string`, `_WARNLEN`, `_warnbuf` and the `capped` truncation guess all delete. 7 `_warnbuf` sites become a new `ConversionHandle` with a finalizer. Four removed C symbols get Julia reimplementations over read + write, keeping the exported names `convert_file`/`convert_str` so BMOPFTools and ExaModelsPower see nothing. `PIO_ABI_VERSION = UInt32(5)` at `src/capi.jl:148`, and `PIO_DIST_ABI_VERSION` must be **deleted** from `src/` — `gen/update_artifacts.jl:159-167` parks the repin forever if the binding declares a dist ABI the binaries no longer expose. `schema_versions()`, `dist_capabilities()` and `matrix_available()` collapse into one `build_info()`. `gen/update_artifacts.jl:105-137` switches from a regex over `pio_schema_versions_json` to `ccall((:pio_version, lib), Cstring, ())`, keeping its `dlsym(...; throw_error=false)` guard. `_string_from` (`capi.jl:409-415`) must start checking the second call's return against its cap — it currently discards it, so a size/fill disagreement is silent truncation, and rule 3 takes buffer symbols from five to seventy-nine. Two risk concentrations, neither of them the renames: the conversion handle lifetimes, and the star-lowered bus space, which changes numbers rather than symbols. Budget 4 to 6 focused days.

**tellegen** — zero C ABI impact (grep for `pio_`/`libpowerio`/`powerio_capi` across `crates/` and `packages/` returns nothing). It links the Rust crates and would only be touched by a future `powerio_dist` type rename, which is a separate 1.0 item.

**PowerMCP** — zero. It imports the Python wheel, which is PyO3 over the Rust crates. Its one real duplication (`_powerio_case_to_ppc`, byte-identical in `pandapower/panda_mcp.py:276` and `PyPSA/pypsa_mcp.py:585`) wants a `Network.to_ppc()` in powerio's Python package, filed separately.

**BMOPFTools.jl, ExaModelsPower.jl** — zero, provided PowerIO.jl keeps `parse_file(T, path)`, `convert_file`, `convert_str`, `to_format`, `LoadSeries` and `goc3_*` as they are. Do not rename PowerIO.jl's public surface to `read`/`open`/`write`: those collide with `Base`, and `parse_file(MulticonductorNetwork, path)`'s type argument is an assertion BMOPFTools depends on (`src/io/from_dss.jl:79`).

# E. The ordered action list to ship v0.9.0

**Stage 0 — settle the design (blocks everything).**
1. [claude] Rewrite `docs/src/abi-v5-audit.md`: the grammar section above the rules, the corrected symbol table, and the fixes to rules 1, 3, 5, 6 and 7 named in §C. Correct the three arithmetic errors (`:27`, `:263`, `:264`) and the four-versus-five `*_free` double count (`:437-438`).
2. [claude] Fix `:62-64`: storing the `Result` in a `OnceLock` freezes a failed first attempt, and that is correct because the payload is a pure function of immutable handle state. Add one sentence declaring caching an unobservable implementation choice, changeable without an ABI bump. Do not delete the caching subsection.
3. [owner] Decide §F. Every open question there changes the frozen table.
4. [claude] Add the read/parse examples in C to the document. If any reads badly the design is wrong, and it is cheap now and expensive after the freeze.

**Stage 1 — land the eleven open PRs.** All green. Merge bottom-up, each retargeted to `main` as the one below merges, with a `main` merge-back after each so the stack does not drift: #310 `numerical-guards` → #311 `dc-convention` → #312 `dc-shunt-conductance` → #313 `opf-prep` → #314 `scopf-instance-parity` → #315 `geo-absorb` → #316 `review-fixes` → #318 `one-version` → #319 `error-architecture` → #320 `name-flip` → #321 `review-pass-2`.
5. [owner] Merge in that order. Batch the pushes — rapid pushes cancel in-flight Pages deploys.
6. [claude] Before #321 merges: fix the three-way `pio_arrow_catalog_json` disagreement (code is right, header doc comment and `capi-arrow.md` are both wrong, in different ways) and add the test asserting per-table field names. This is a shipped-header defect and should not wait for v5.
7. [claude] Fix `docs/src/languages.md:31` and `:123-127`, missed by the name flip.
8. [owner] Confirm `main` is green before calling anything a release.

**Stage 2 — implement v5 in powerio (`v09/abi-v5`, rebased onto `main`).**
9. [claude] Rule 5 machinery first: the four structs, the append-only + padding-free + E2BIG clauses, the `offset_of!` pins, no `PIO_NORMALIZE_OPTIONS_MIN_SIZE`.
10. [claude] Rule 4: `PioConversion` and the five conversion symbols; delete `finish_conversion`'s `warnbuf`.
11. [claude] Rule 3: every string-returning symbol to the two-call idiom; delete `pio_string_free`; `errbuf[0] = '\0'` on entry everywhere.
12. [claude] The read/parse/write consolidation, `resolve_path`, and `PioSource`.
13. [claude] The bulk rename, last — `dlsym` turns every miss into a load-time failure, so this is the cheap part.
14. [claude] Deletions: the acopf trio and `PioAcopfInstance`, the four `convert_*`, `pio_dist_abi_version`, `pio_matrix_available`, `pio_dist_capabilities_json`, `pio_schema_versions_json`, `pio_normalize_with_options`, `PIO_DIST_ABI_VERSION`.
15. [claude] `pio_build_info` and the `schemas`/`capabilities` payload.
16. [claude] Header preamble, regenerate `powerio.h`, hand-edit the manifest array, `smoke.c`, `header_cpp.cpp`.
17. [claude] CI: symbol snapshot, buffer-protocol test, grammar check, scoped miri, `pypsa_csv` fuzz target.
18. [claude] Docs sweep, `CHANGELOG.md` 0.9.0 section, `AGENTS.md`.
19. [owner] Review and merge to `main`. Confirm green.

**Stage 3 — the PowerIO.jl companion (the ordering constraint lives here).**
20. [claude] Create `v09/abi-v5` in PowerIO.jl with the same branch name, so `julia-binding.yml` tests the powerio PR against it. Do this *during* Stage 2, not after — the two must be green together, and that is the whole point of the companion-branch trick.
21. [claude] Migrate the binding in this order: `ConversionHandle` and the 7 conversion sites; the star-lowered bus space, with `@test length(bus_ids(net)) == n_buses(net)` written *before* the rename lands so it fails against v4 and passes against v5; the 22 `_string_from` conversions; the bulk rename; then delete `_take_string`, `_WARNLEN`, `_warnbuf` and `capped`.
22. [claude] `PIO_ABI_VERSION = UInt32(5)`; **delete `PIO_DIST_ABI_VERSION`**, or `gen/update_artifacts.jl:159-167` parks the repin permanently. Point the repin lineage gate at `pio_version()`.
23. [claude] Fix `_string_from` to check the second return against its cap.

**Stage 4 — release.**
24. [owner] Tag `v0.9.0` on powerio `main`. `release-binaries.yml` builds five platforms into a **draft** release.
25. [owner] **Publish the draft release. This is the only manual step across both repos.** Publishing fires `notify-powerio-jl.yml`.
26. [expected] PowerIO.jl's `update-artifacts.yml` runs and **parks**: it downloads the host binary, reads `pio_abi_version() == 5` against a binding still declaring 4, and leaves `Artifacts.toml` untouched with "skipping v0.9.0: its binaries report ABI 5, this binding targets ABI 4". This is the guard rail working, not a failure. Do not hunt for release state that cannot exist yet.
27. [claude] Open the PowerIO.jl PR off the companion branch carrying: the migration, `PIO_ABI_VERSION = 5`, `PIO_DIST_ABI_VERSION` deleted, `Artifacts.toml` repinned by hand to the published v0.9.0 artifacts, the CHANGELOG section, and the `Project.toml` version bump — all in one PR, because `test_release.jl` gates changelog/version consistency.
28. [owner] Review and merge it. Merging is the release on the Julia side.
29. [expected] The next dispatched or scheduled `update-artifacts.yml` run sees ABI 5 == 5, no dist ABI declared, and lineage 0.9.0 matching, then dispatches Register Package itself. General AutoMerge takes ~15 minutes; TagBot tags PowerIO.jl.
30. [owner] Verify by observation, not by assumption: the release commit on PowerIO.jl `main`, and the version in Julia General's `Versions.toml`. If the self-heal has not fired within a day, re-dispatch.

**Stage 5 — additive, does not block the tag.**
31. [claude] The Arrow generator-cost tables, as two primitive non-null tables rather than columns on `gen`: `PIO_ARROW_TABLE_GEN_COST` (`gen_index`, `model`, `startup`, `shutdown`, `ncost`, `coeff_offset`, `coeff_count`) and `PIO_ARROW_TABLE_GEN_COST_COEFF` (`value`), plus solver-space and HVDC twins. `List<Float64>` and nullable columns both fail PowerIO.jl's decoder outright (`arrow.jl:89-95` and `:285`), so a costless generator is an absent row rather than a null. This is the largest duplication in the consumer tree — cost exists on no numeric surface, which is why 836 lines of `exa.jl` rebuild the ExaModelsPower payload from JSON and never touch the fast path.
32. [claude] File as issues, do not rush into the freeze: extractor coverage for `qg`/`qmax`/`qmin`/`rate_a`/`vmin`/`vmax`; the `OperatingPointSeries` construct/attach surface (#236); the 12 `powerio_dist` element-struct renames for 1.0; publish-or-delete the stale GHSA-vg4v-rm4h-3rmg advisory (the pwb/pwd fuzz targets it describes as missing were added a month before it was filed); a `Network.to_ppc()` in powerio's Python package.
33. [claude] State in the document, in one line, that the freeze forbids breaking changes and not additions — "last breaking change" reads to a reader as "last change".

# F. Decisions that still need you

**1. The version register.** ABI 5 is the last C break, and 1.0 was slated for late August. Options: (a) ship ABI 5 as v0.9.0 and keep 1.0 for the freeze declaration after consumers have run on it for a cycle; (b) ship it as 1.0.0 directly. Recommend (a). The star-lowered bus space and the SCOPF index-base flip change numbers rather than symbols, and you want one release of consumer exposure before the freeze is a promise. It also removes the risk that the literal `"0.9.0"` baked into `version.rs:38` and four tests is wrong.

**2. `PioWriteOptions` with no fields.** The struct would ship carrying only `struct_size`, so a 2027 write knob (precision, sidecar policy, overwrite) is an appended field rather than a second symbol. Options: (a) ship it empty; (b) omit it, and accept that `write` can never gain an option without `write_ex`. Recommend (a). It costs one typedef and is exactly what rule 5's mechanism is for. If you would rather not freeze a fourth layout, (b) is defensible and I will not argue.

**3. Does `PioSource` ship in v0.9.0?** It fixes a real O(N) defect — PowerIO.jl's `read_gridfm_scenarios` re-reads the whole Parquet dataset once per scenario (`gridfm.jl:59`) against a Rust API whose comment says it exists to avoid exactly that. But it is six new frozen symbols serving one implemented format, and the corpus reader they were also designed for does not exist in Rust. Options: (a) ship the gridfm-only six now; (b) keep one `pio_balanced_from_dataset(path, from, scenario, ...)` and add the handle when a corpus reader exists. Recommend (a): the efficiency fix is real today, and adding the container form later while `parse_scenario` is frozen leaves two doors to one idea.

**4. Do the Arrow cost tables land in 0.9.0?** They are additive and cannot break anything, so they are not freeze-critical. But they are the change that would let PowerIO.jl retire most of `exa.jl`, and nothing else in this pass makes the fast path usable for the workload people actually run. Options: (a) Stage 5 of this release; (b) 0.9.1. Recommend (a) if Stage 2 lands with time to spare, (b) without hesitation if it does not.

**5. Does geo ship at all?** Five symbols, zero consumers on any surface, and the reshape costs a new `PioGeoApplyReport` struct. Options: (a) ship it reshaped; (b) delete the geo family from the C ABI and re-add additively when PowerIO.jl or another C consumer asks. Recommend (a) — tellegen's Rust surface is a validated specification good enough to build against, and an ABI meant as bedrock shipping with no coordinate story reads as an omission. But (b) is genuinely cheaper and the freeze permits it.

**6. How far does the PowerIO.jl public rename go?** The binding can absorb all of v5 without changing a single exported Julia name, which means BMOPFTools and ExaModelsPower see nothing. Or it can take the opportunity for a breaking Julia release. Recommend keeping the public surface fixed: `parse_file(T, path)`'s type argument is an assertion a live consumer relies on, and `read`/`open`/`write` collide with `Base`. The one addition worth making is `open_source(dir; from)` returning an iterable, so `read_gridfm_scenarios` keeps its name and gets the single-read fix underneath.