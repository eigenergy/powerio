# C ABI v5

This is the C surface powerio freezes. v4 set a grammar. v5 applies it to every symbol v4
left alone. This is the last breaking change to the C ABI.

One question decides everything here: what does a caller in 2029 need this to be.

This document replaces `abi-v5-audit.md`, `abi-v5-review.md`, and `abi-v5-followups.md`. Those
were three rounds of design. They disagreed with each other. This is the settled result.

## The grammar

Every symbol has this form:

```
pio_<subject>_<operation>[_<qualifier>]
```

**Subjects.** The subject is the handle the function takes or returns. An empty subject means
the library itself.

| subject | handle |
|---|---|
| *(empty)* | the shared object |
| `balanced` | `PioBalancedNetwork` |
| `multiconductor` | `PioMulticonductorNetwork` |
| `package` | `PioPackage` |
| `scopf` | `PioScopfInstance` |
| `conversion` | `PioConversion` |
| `source` | `PioSource` |
| `geo` | a geographic layer; no handle |

**Operations.** There are six forms. Only four use a verb, and those verbs are a closed list.

| form | shape |
|---|---|
| constructor | returns a new handle. Verbs: `parse`, `from_json`, `from_<subject>`, `from_source`, `normalize`, `lower_to_<subject>`, `apply_<noun>`, `open` |
| destructor | `free`. One per subject. Returns void. Accepts NULL |
| emitter | `size_t f(handle, out, cap, errbuf, errlen)`. Payload names: `to_json`, `summary`, `warnings`, `graph`, `validation`, `diagnostics`, `operating_points`, `study`, `geo`, `text`, `file`, `catalog`, `build_info` |
| accessor | no verb. `n_<plural>` returns a count. `<plural>` fills an array. `<singular>` returns one value. `is_<adj>` and `has_<noun>` return `int32_t` |
| mutator | `validate`, `set_<noun>`, `materialize_<noun>`. `PioPackage` only |
| out-param | `int32_t f(handle, …, <out structs>, errbuf, errlen)`. For payloads that are not bytes: `to_arrow` |

**Qualifiers.** Two members: `_check`, a preflight for the operation it attaches to, and
`_bytes`, which says the input is memory rather than a path. Nothing else may use the slot
without being added to this list.

**The rule that makes the grammar predictable:**

> There is one ingest verb, `parse`. It takes a path. A suffix appears only when the bytes
> come from somewhere other than a path.

So `pio_balanced_parse(path, …)` and `pio_balanced_parse_bytes(data, len, …)`, and nothing
else. `_file`, `_str`, `_dir`, `_dataset` and `_scenario` all disappear.

The suffix does not name the storage shape, which rule 7 rejects. It names **who touches the
filesystem**. A path argument means the library opens files and may follow an OpenDSS
`Redirect` tree. A buffer argument means it opens nothing, which is a security property and
not a convenience: `parse_bytes` is the entry point for untrusted text, and it is why the
0.7.3 advisory fix works.

`_bytes` rather than `_str`, because a PowerWorld `.pwb` is binary and a NUL truncates it. v4
called this `_str` and could not accept half the formats it named.

The precedent is libxml2: one verb, and a suffix for where the bytes came from —
`xmlReadFile`, `xmlReadMemory`, `xmlReadDoc`.

## The seven rules

**1. Every symbol names its subject.** v4 left the balanced model unnamed. `pio_parse_str` was
balanced, `pio_dist_parse_str` was multiconductor. The reader had to know which omission meant
what. Both models are peers. Both are named.

Names are spelled out. The reason is not that `mc` reads as Monte Carlo; that claim does not
survive checking, and psspy has no Monte Carlo entry points. The reason is that
`balanced_network` and `multiconductor_network` are already the `.pio.json` payload keys, the
Rust type names, the Python class names and the Julia exported types. A C header you cannot
grep with the word the file format uses is the one surface out of step. Spelling them out
costs 3.9 characters of mean symbol length. v5 averages 22.7 characters. cairo averages 24.0.

**2. `dist` is gone as a word.** v4 used `dist` in 15 symbols and `multiconductor` in 4, in one
header, for one model. The crate keeps the name `powerio-dist`, and that is fine: a package
name is not a symbol prefix, and libcurl exports `curl_easy_*`. The word is also spent
elsewhere. psspy ships 23 `dist_*` functions and every one means *disturbance*.

**3. One buffer idiom. No raw pointer to library memory crosses the boundary.** v4 had three
idioms. 27 symbols returned an owned `char *` that the caller freed with `pio_string_free`. 5
filled a caller buffer. 9 filled caller arrays. v5 keeps the last two:

```c
size_t pio_x(const PioHandle *h, char *out, size_t cap, char *errbuf, size_t errlen);
```

The return is the total available. `NULL` or `0` is a size query. `pio_string_free` is deleted
with the class it freed.

Everything that crosses is either bytes copied into memory the caller owns, or an opaque
handle with exactly one free function. The library still allocates handles. It no longer hands
out pointers into its own memory.

These invariants hold for every buffer symbol:

- the return excludes the NUL
- the buffer is always NUL terminated
- a short buffer truncates on a UTF-8 boundary, so a C caller never receives a split codepoint
- a fallible symbol writes `errbuf[0] = '\0'` on entry, and a message only on failure

That last one matters. Several of these symbols can legitimately return nothing. Without it, a
`0` return means both "empty" and "failed", and the caller cannot tell.

**Caching.** A size query followed by a fill would run the serialization twice. Handles with no
mutator may cache each payload in a `OnceLock<Result<String, String>>`. That is
`PioBalancedNetwork`, `PioMulticonductorNetwork`, `PioScopfInstance`, `PioConversion` and
`PioSource`. `OnceLock<T>` is `Sync` when `T` is, so the header's promise that concurrent reads
are safe still holds.

`PioPackage` is excluded. It has two mutators, `pio_package_validate` and
`pio_package_set_operating_points`, and they rewrite the fields its accessors read. A cached
read taken before `validate` and served after it would return pre-validation text from a
`const` accessor. Its four document accessors recompute per call.

**4. A conversion is a handle, because it has two outputs.** v4's seven `warnbuf` symbols
truncated silently. `finish_conversion` discarded the needed length. There was no size query.
A long fidelity-loss list was lost with no signal, and the header told callers that 256 bytes
always sufficed.

A conversion has text and warnings. Both need a size query. Neither can attach to the network
handle, because write-time warnings are produced at write time.

A `PioConversion` owns its text and warnings. It does not borrow the network. It stays valid
after that handle is freed, so the two have no ordering requirement.

It also reports the files a write produced. `pio_conversion_n_files` and
`pio_conversion_file` replace a newline-joined list, because a newline is a legal byte in a
POSIX filename. This closes a live defect: OpenDSS sidecars are dropped today, so a written
`.dss` can name a coordinates file the user does not have.

**5. One extensible options struct.** With no `repr(C)` structs, every new option needs a new
symbol. Over three years that bloats the surface or forces a v6.

```c
typedef struct PioNormalizeOptions {
  size_t  struct_size;
  int32_t clamp_angle_bounds;   /* 0 false, any nonzero true */
  double  angle_bound_pad;      /* radians; 0 is a valid explicit value */
} PioNormalizeOptions;
```

The precedent is the Linux extensible syscall convention: `openat2`, `clone3`,
`sched_setattr`. Win32 `cbSize` is the older and coarser form. Vulkan is not a precedent; its
`sType` and `pNext` chain solves a different problem.

The rules, all of which must be stated or the mechanism does not work:

- The library reads a field only if its offset and size fall within
  `min(opts->struct_size, sizeof(PioNormalizeOptions))` as the library was compiled.
- A newer caller against an older library is safe by that same clause.
- If the caller's tail beyond the library's own size is **nonzero**, the call fails. The caller
  asked for something this build does not do. Silence would be wrong.
- Fields are append only. No reorder, no removal, no type change.
- The struct has no implicit padding. Reserved fields are named.
- The caller zero-fills before setting fields.
- `NULL` means all defaults. That is the only way to ask for defaults without stating a size.

Booleans cross as `int32_t`. The header contains no `bool` today, and
`pio_normalize_with_options` already converts explicitly.

**6. One ABI integer.** `PIO_DIST_ABI_VERSION` existed to absorb distribution volatility. That
volatility is in the BMOPF schema, which the IEEE task force owns and powerio reproduces. A
BMOPF revision changes a reader, a writer and an emitted token. It changes no C signature.

So the second integer never fires for its stated reason. What it does instead is give one
shared object two compatibility promises. No mature C library does that.

Foreign schema drift is reported at runtime by `pio_build_info`. An integer checked once at
load cannot express "I speak BMOPF 0.2 but not 0.3".

Removing the symbol costs the binding more than removing the constant. PowerIO.jl gates every
distribution entry point on `_ensure_dist_compatible`, which resolves `pio_dist_abi_version`
and reports a missing symbol as "use powerio-capi v0.3.1". Thirteen call sites reach it. The
gate has to be rebuilt on `pio_has_feature("dist")` in the same change that repins the
artifact, or a v5 library that fully supports distribution refuses every distribution call.

**7. Cardinality is the axis, not storage.** v4 split reading by storage: `pio_parse_file` for
documents, `pio_read_dir` for directories. That split does not survive the formats.

An OpenDSS `.dss` is one file that pulls in a tree. A PyPSA case is a directory. powerio's own
`parse_file` already dispatches a PyPSA directory before it looks at any extension. One file is
not one case for much of the software powerio reads.

What does survive is how many cases a path yields. Almost every format yields one. A gridfm
dataset directory yields N over one shared topology. A PowerModels JSON with
`multinetwork=true` also yields N, and powerio reads the first and warns about the rest today.

So every path opens to a source:

```c
PioSource *pio_source_open(const char *path, const char *from,
                           const PioReadOptions *opts, char *errbuf, size_t errlen);
size_t     pio_source_count(const PioSource *);
size_t     pio_source_entry_names(const PioSource *, char *out, size_t cap,
                                  char *errbuf, size_t errlen);   /* NUL separated */
size_t     pio_source_entry_name(const PioSource *, size_t index, char *out, size_t cap,
                                 char *errbuf, size_t errlen);
void       pio_source_free(PioSource *);

PioBalancedNetwork *pio_balanced_from_source(const PioSource *, size_t index,
                                             char *errbuf, size_t errlen);
```

A matpower file is a source with one entry. A PyPSA folder is a source with one entry. A
gridfm dataset has N. `pio_source_count` may return 0 for an empty container.

Entries are named, not numbered. `int64_t` is gridfm's Parquet key type, not a property of
containers; PGLib and GOC3 entries have no integer key.

`pio_balanced_parse` survives as a documented composition of open plus entry 0, not as a second
route. SQLite documents `sqlite3_exec` the same way. GDAL and libarchive make open-then-
enumerate the only path, and they are the honest counterexample; the shortcut wins here on the
ratio, since almost every case is a single entry. `pio_balanced_parse` on a multi-entry path
fails and names `pio_source_open`, so the shortcut can never silently disagree with the
container form.

## The symbol table

`=` unchanged · `R` renamed · `S` re-signatured · `X` removed · `N` new

### Handshake, 7 → 4

| v4 | v5 | | why |
|---|---|---|---|
| `pio_abi_version` | same | = | the gate; call it before you trust anything else |
| `pio_version` | same | = | build identity; the artifact repin compares it, and an ABI integer cannot |
| `pio_has_feature` | same | = | the cheap path for a caller with no JSON parser |
| `pio_schema_versions_json` | — | X | three keys, none unique; all duplicate another symbol |
| `pio_dist_capabilities_json` | — | X | a dist-gated symbol is the wrong home for the fact rule 6 needs |
| `pio_dist_abi_version` | — | X | rule 6 |
| `pio_matrix_available` | — | X | `pio_has_feature("matrix")` says it |
| — | `pio_build_info` | N | one report: version, abi, features, capabilities, foreign schemas |

`pio_abi_version` and `pio_version` are both kept because neither derives from the other. The
integer is the gate and changes only on a break. The string is identity and changes every
release. PowerIO.jl resolves everything by `dlsym` from a pinned artifact, so
`pio_abi_version` is powerio's soname.

### Balanced model

| v4 | v5 | | why |
|---|---|---|---|
| `pio_parse_file` | `pio_balanced_parse` | R S | takes a path of any shape: a file, a directory, or a file that pulls in a tree |
| `pio_parse_str` | `pio_balanced_parse_bytes` | R S | `(const void *, size_t)`, so `.pwb` needs no temp file. Opens nothing, so it is the entry point for untrusted input |
| `pio_from_json` | `pio_balanced_from_json` | R S | rules 1, 3 |
| `pio_read_dir` | `pio_balanced_from_source` | R S | one entry of an opened source |
| `pio_scenario_ids` | `pio_source_entry_names` | R S | re-homed onto the source; names, not integers |
| `pio_classify_str` | `pio_classify_bytes` | R S | takes memory, so it carries the suffix; gains the error channel it never had |
| `pio_normalize` | `pio_balanced_normalize` | R S | takes `const PioNormalizeOptions *` |
| `pio_normalize_with_options` | — | X | folded into the options struct |
| `pio_to_json` | `pio_balanced_to_json` | R S | rules 1, 3 |
| `pio_to_format` | — | X | folded into `pio_balanced_write` with a NULL path |
| `pio_write_dir` | — | X | folded; it wrote one format, and that format is a case |
| `pio_convert_file` | — | X | three same-typed strings; it shipped with two reversed and still linked |
| `pio_convert_str` | — | X | same |
| — | `pio_balanced_write` | N | one write verb. NULL path serializes; a real path writes a file, a directory, or a file plus sidecars |
| `pio_network_free` | `pio_balanced_free` | R | rule 1 |
| `pio_to_arrow` | `pio_balanced_to_arrow` | R | rule 1 |
| `pio_warnings` | `pio_balanced_warnings` | R | rule 1; already rule-3 shaped |
| `pio_summary_json` | `pio_balanced_summary` | R S | `_json` named the encoding, not the payload |
| `pio_source_format` | `pio_balanced_source_format` | R S | returns the format token, not a Rust `Debug` spelling |
| `pio_network_name` | `pio_balanced_name` | R S | the handle is the network |
| `pio_n_buses` | `pio_balanced_n_buses` | R S | **the star-lowered space**, so `length(bus_ids) == n_buses` |
| `pio_bus_ids` | `pio_balanced_bus_ids` | R S | same space |
| `pio_n_gens` | `pio_balanced_n_generators` | R | the one abbreviated noun |
| `pio_gens` | `pio_balanced_generators` | R | same |
| `pio_geo_extract` | `pio_balanced_geo` | R S | emitter; `extract` is not a verb in the list |
| `pio_geo_apply` | `pio_balanced_apply_geo` | R S | constructor; gains a `PioGeoApplyReport *` |

The remaining extractors are plain renames for rule 1: `pio_n_branches`, `pio_n_switches`,
`pio_n_islands`, `pio_base_mva`, `pio_is_radial`, `pio_ref_bus_index`, `pio_ref_bus_indices`,
`pio_branches`, `pio_branch_charging`, `pio_switches`, `pio_bus_demand`, `pio_bus_shunt`.

### Source, new

`pio_source_open`, `pio_source_count`, `pio_source_entry_names`, `pio_source_entry_name`,
`pio_source_free`. See rule 7.

### Conversion, new

`pio_conversion_text`, `pio_conversion_warnings`, `pio_conversion_n_files`,
`pio_conversion_file`, `pio_conversion_free`. See rule 4.

### Multiconductor

Every `pio_dist_*` becomes `pio_multiconductor_*`. `parse_file` becomes `parse`, `parse_str`
becomes `parse_bytes`, `summary_json` becomes `summary`, `graph_json` becomes `graph`. `to_format`
folds into `pio_multiconductor_write`. `pio_dist_convert_file` and `pio_dist_convert_str` are
removed, as their balanced twins are. `warnings`, `free`, `to_json`, `from_json`, `geo_extract`
and `geo_apply` follow the balanced pattern.

`parse` is where rule 7 is clearest. An OpenDSS `.dss` is the format most associated with a file
extension, and it is the one least likely to be a single file.

The EXPERIMENTAL banner is retired. The dist C signatures were never the unstable part. The
BMOPF payload schema was, and it is versioned where it lives.

### Package, 18 → 18

`pio_package_parse_file` becomes `pio_package_parse`, and `pio_package_parse_str` becomes
`pio_package_parse_bytes`.

Four lose a repeated noun: `from_balanced_network` → `from_balanced`,
`from_multiconductor_network` → `from_multiconductor`, and the two `to_*` twins. The handle is
the network; saying it twice adds nothing.

Two get shorter: `pio_package_lower_multiconductor_to_balanced` → `pio_package_lower_to_balanced`
(44 → 29), and `pio_package_multiconductor_to_balanced_preflight_json` →
`pio_package_lower_to_balanced_check` (53 → 35). `lower` stays because `.pio.json` publishes
`lowering_history`. `preflight` goes because it is an internal stage name.

`pio_package_lower_to_balanced` and `pio_package_to_balanced` must not differ by one suffix.
They return different handle types under mutually exclusive preconditions.

Five lose `_json`: `to_json`, `validation_json`, `diagnostics_json`, `operating_points_json`,
`study_json`. The suffix named an encoding that no sibling symbol varies.

`free`, `validate`, `set_operating_points`, `materialize_operating_point` and
`materialize_study_commit` keep their names.

### Problem instances, 6 → 3

| v4 | v5 | | why |
|---|---|---|---|
| `pio_scopf_parse_str` | `pio_scopf_parse_bytes` | R S | takes memory, so it carries the suffix; a SCOPF document arrives as bytes, never as a path |
| `pio_scopf_to_json` | same | S | **0-based**; see the movers |
| `pio_scopf_instance_free` | `pio_scopf_free` | R | the type carries the noun |
| `pio_acopf_from_network` | — | X | no consumer; `acopf` appears nowhere in PowerIO.jl |
| `pio_acopf_to_json` | — | X | same |
| `pio_acopf_instance_free` | — | X | goes with its handle |

The freeze forbids breaking changes, not additions. Delete now. Re-cut additively when a C
consumer exists and can say what shape it needs.

### Geographic and Arrow

| v4 | v5 | | why |
|---|---|---|---|
| `pio_geo_parse` | same | S | returns `PioConversion *`, so the tolerant reader's notes stop being discarded |
| `pio_arrow_catalog_json` | `pio_arrow_catalog` | R S | drop `_json` |
| `pio_string_free` | — | X | rule 3 removes the class |

`pio_geo_parse` keeps its verb. It parses. `normalize` is already the per-unit transform in the
same header, and in the geospatial domain `GEOSNormalize_r` canonicalizes a geometry that is
already parsed.

Arrow stays balanced only. `arrow_export.rs` contains no multiconductor tables, and
`powerio-dist` has no Arrow dependency. A table id is cheap to add later; a shipped table's
column order is frozen, so the cost of guessing lands on the columns.

### Handles, structs, macros

New handles: `PioConversion`, `PioSource`. `PioNetwork` becomes `PioBalancedNetwork`.
`PioDistNetwork` becomes `PioMulticonductorNetwork`. `PioAcopfInstance` is removed.

New structs: `PioNormalizeOptions`, `PioReadOptions`, `PioWriteOptions`, `PioGeoApplyReport`.
All follow rule 5. `PioWriteOptions` ships with only `struct_size`, so a later write option is
an appended field rather than a second symbol.

`PIO_ABI_VERSION` becomes 5. `PIO_DIST_ABI_VERSION` is removed. `PIO_ERRBUF_MIN` stays. The 21
`PIO_ARROW_TABLE_*` ids stay, and stay append only.

## The symbols whose meaning moves

A rename is safe: the old name stops resolving. A re-signature is safe: the old call stops
compiling. Neither protects a symbol whose payload changes while the call still works.

**`pio_balanced_n_buses` and `pio_balanced_bus_ids`** move to the star-lowered space. v4
returned fewer ids than the extractors had rows, so trailing rows had no id. v5 returns one id
per row. A binding that asserts `length(ids) == n` fails on v4 and passes on v5. That assert is
the migration test. The handle rename forces every C declaration to be edited, so no caller
reaches the new behavior without touching the line.

**`pio_scopf_to_json`** keeps its name and changes 1-based indices to 0-based. The document
carries `index_base`, but a field is only a mechanism if something reads it, and nothing does
today. A 0-based index is still a valid 1-based index, so a missed conversion reads the wrong
element rather than failing.

So v5 requires the binding to normalize: **PowerIO.jl converts to 1-based at the boundary.**
The wire value is 0. The value a Julia caller sees is 1. Julia arrays are 1-based, and a
binding should speak its own language. Python, whose lists are 0-based, passes it through.

## What does not change

Arrow table ids and column order. The format tokens, which are strings and were never symbols.
The opaque handle design. The panic guard on every entry point. `errbuf` and `errlen` last.

## What this costs PowerIO.jl

Every symbol resolves by `dlsym`, so a rename fails at load rather than reading wrong. One v4
precedent is worth remembering: `pio_convert_file` kept its symbol, arity and types while two
arguments were reordered. It linked, and it read the formats reversed.

Julia is where the C type system does not help. PowerIO.jl holds handles as `Ptr{Cvoid}`, so
renaming a handle costs it nothing and protects it from nothing. The `ccall` signature changes
carry the migration, and rule 3 changes most of the surface.

Roughly 98 symbol reference sites across 12 files. 22 `_take_string` sites become the
size-query helper, and `_take_string`, `_WARNLEN` and the truncation guess are deleted. The two
risk concentrations are the conversion handle lifetimes and the star-lowered bus space, which
changes numbers rather than symbols. `PIO_DIST_ABI_VERSION` must be deleted from the binding,
or the artifact repin parks forever.

Deleting that constant is not the whole of it. `_ensure_dist_compatible`, `schema_versions`,
`dist_capabilities` and `matrix_available` all resolve symbols v5 removes. The first throws on
a missing symbol, so every distribution call fails; the other three are guarded by
`_exports_symbol`, so they report "unavailable" on a library that has the feature. All four
move to `pio_has_feature` and `pio_build_info` in the same change as the repin.

Every exported PowerIO.jl name survives. `open_source`, `entry_names` and `entry_name` are
added. BMOPFTools and ExaModelsPower see no API change, but `BMOPFTools.from_dss` reaches
`pio_dist_abi_version` through `parse_file(MulticonductorNetwork, …)`, so it breaks if the
gate above is not rebuilt.

tellegen and PowerMCP are not affected. tellegen links the Rust crates. PowerMCP imports the
Python wheel. Neither calls a `pio_` symbol.

## Open decisions

These change the table. Settle them before implementation starts.

1. **`PioWriteOptions` with no fields.** Ship it empty so a later write option is an appended
   field, or omit it and accept that `write` gains options only through a second symbol.
2. **Arrow generator cost tables.** Additive, so not freeze-critical. They are what lets
   PowerIO.jl retire most of `exa.jl`, which rebuilds the ExaModelsPower payload from JSON
   because Arrow carries no cost.
3. **The geo family.** Five symbols, no C consumer today. tellegen's Rust usage is a validated
   specification to build against, but shipping and deleting are both defensible.
