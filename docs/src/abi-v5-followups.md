# v5 decision document — containers, geo, ExaModelsPower

Verdicts below are final for the freeze. Where a designer and a critic disagreed, the critic's counter is adopted and marked. Line citations are from this worktree unless the path says otherwise.

---

## 1. Containers

### Why only gridfm

Because gridfm is the only container anyone wrote a reader for. It is a fact about the implementation, not about the world. The repo already names N-case input it turns away: `format/mod.rs:476` refuses OPFData releases with "extract and parse an example_N.json source file", and `AGENTS.md:326` vendors `pglib/` as a fixture directory the library cannot open.

**Storage shape and cardinality are independent axes.** Storage shape never reaches the ABI — `read` takes a path and `resolve_path` owns the structural inference. Four shapes are already live behind one entry point: `mod.rs:440-441` dispatches a directory before it looks at an extension, `:454` reads a binary file with `std::fs::read` where `:515` uses `read_to_string`, and `powerio-dist`'s `.dss` loader resolves an arbitrarily deep include tree behind a single path. Zero symbols were spent on any of it. Cardinality is the only axis that got a second symbol (`pio_read_dir`'s `scenario: i64`, `powerio-capi/src/lib.rs:544-550`), which is the tell.

| format | storage shape | cases per open |
|---|---|---|
| MATPOWER `.m`, PSS/E `.raw`, PowerWorld `.aux`, PSLF `.epc`, egret / pandapower / Surge / BMOPF / PMD JSON | one text file | 1 |
| PowerWorld `.pwb` | one binary file | 1 |
| PowerModels JSON | one text file | 1 read; **N under `nw` dropped with a warning** (`format/powermodels.rs:468-473`, "multinetwork=true: only the top-level single snapshot was read") |
| GOC3 JSON | one text file | 1; N time intervals *inside* it, first one mapped |
| OPFData JSON | one text file | 1; the published release is an archive of many, refused at `mod.rs:475-478` |
| PyPSA CSV | directory of files | 1; time series siblings named as unread |
| OpenDSS `.dss` | one file plus a resolved include tree | 1 |
| gridfm Parquet | directory (three accepted layouts) | **N** |
| `.pio.json` | one text file | not a case format; N operating points inside |
| `.pwd`, geo sidecar | one file | 0 (display / sidecar) |

**Cardinality is defined operationally in the header:** the number of entries one open materializes as independent networks with the shipped reader. That excludes GOC3 intervals and SCOPF contingencies (they are within-case multiplicity), and it excludes `.pio.json` operating points by a second rule, stated in one line: **`PioSource` is the path-level container, `PioPackage` is the document-level one, and neither grows into the other.** Without that sentence someone proposes `pio_source_open` over contingencies in 2028.

**A source is never opened over a single document in v5.** PowerModels multinetwork stays a warning. That is a deliberate foreclosure; see §5(a).

### Verdict: universal model, `read` retained as the documented composition

`pio_source_open` accepts every path — that is the direct answer to "why only gridfm". A matpower file is a source with one entry; a PyPSA folder is a source with one entry; a gridfm dataset has N. `read` survives as a spelling of open + entry 0, not as a second route, which is what killed v4's `pio_parse_file`/`pio_read_dir` split. SQLite is the precedent for the shortcut-as-composition (`sqlite3_exec` documented as "a convenience wrapper around sqlite3_prepare_v2(), sqlite3_step(), and sqlite3_finalize()"); GDAL and libarchive are the honest counterprecedent, both making open-then-enumerate the only path. They lose here on the 19-to-1 ratio, not on design. Say that in the document rather than claiming their endorsement.

Three specification fixes over the designer's draft, all adopted from critique:

1. **`from` names the entry format only.** Container selection is structural. Today the two meanings are already separate — `mod.rs:488-494` treats `from` as the entry format, `powerio-matrix/src/io/mod.rs:56-63` treats it as the container ("is not a dataset directory format (dataset formats: gridfm)"). One frozen string cannot carry both. For 0.9.0 the two token namespaces are disjoint and a test enforces it; "corpus of PSS/E files" is inexpressible and lands later as an appended `entry_from` field under rule 5. Nothing is foreclosed because corpus is unsupported in 0.9.0 anyway.
2. **Open is lazy.** It resolves the `Route` and enumerates entries; materialization happens at `from_source`. `read` is *observationally* equivalent to the composition, not literally it — otherwise entry 0 of a matpower source is a deep copy on the 99% path, against the zero-copy retained-source guarantee at `mod.rs:512-515`. Enumeration reads only what enumeration needs (gridfm reads `bus_data.parquet` alone, `io/gridfm.rs:1148-1152`).
3. **Drop the `count >= 1` invariant.** Zero is a legal count for an empty container; `pio_balanced_read` is the symbol that fails.

### Entry keying: by name, not `int64_t`

`pio_source_entry_ids` is deleted. A gridfm scenario id is `Vec<i64>` off a Parquet column (`io/gridfm.rs:1148-1152`) — that is gridfm's key type, not containers'. PGLib entries are `pglib_opf_case14_ieee.m`, GOC3 entries are `scenario_001`, OPFData entries are `example_1471.json`. A frozen `int64_t *` either fails or returns `0..n-1` dressed as an id. `find(src, "3", &i)` is the gridfm lookup and int64 round-trips through decimal losslessly (`PowerIO.jl/src/gridfm.jl:65-88` is the only integer consumer).

Model routing is a discriminator, never an error string or a token table. `PowerIO.jl/src/network.jl:210-221` re-implements that routing in Julia today; `pio_source_entry_model` is what deletes it.

### Final C declarations

```c
typedef struct PioSource PioSource;

typedef enum PioEntryModel {
  PIO_ENTRY_MODEL_NONE          = 0,   /* display / sidecar / package */
  PIO_ENTRY_MODEL_BALANCED      = 1,
  PIO_ENTRY_MODEL_MULTICONDUCTOR= 2
} PioEntryModel;

PioSource *pio_source_open(const char *path, const char *from,
                           const PioReadOptions *opts,
                           char *errbuf, size_t errlen);
size_t   pio_source_count(const PioSource *src);        /* 0 iff NULL or empty container */
size_t   pio_source_entry_names(const PioSource *src, char *out, size_t cap,
                                char *errbuf, size_t errlen);  /* NUL-separated; total bytes */
size_t   pio_source_entry_name(const PioSource *src, size_t index, char *out, size_t cap,
                               char *errbuf, size_t errlen);   /* the i-th field of the above */
int32_t  pio_source_entry_model(const PioSource *src, size_t index); /* PioEntryModel; -1 on bad index */
int32_t  pio_source_find(const PioSource *src, const char *name, size_t *index,
                         char *errbuf, size_t errlen);
void     pio_source_free(PioSource *src);

PioBalancedNetwork       *pio_balanced_read(const char *path, const char *from,
                                            const PioReadOptions *opts,
                                            char *errbuf, size_t errlen);
PioMulticonductorNetwork *pio_multiconductor_read(const char *path, const char *from,
                                                  const PioReadOptions *opts,
                                                  char *errbuf, size_t errlen);
PioBalancedNetwork       *pio_balanced_from_source(const PioSource *src, size_t index,
                                                   char *errbuf, size_t errlen);
PioMulticonductorNetwork *pio_multiconductor_from_source(const PioSource *src, size_t index,
                                                         char *errbuf, size_t errlen);
```

`pio_source_entry_format` is **not** shipped. `entry_model` is what routing needs; the format token is reachable after materialization via `pio_balanced_source_format`, and the freeze permits adding it later.

Frozen invariants for the header: a network from a source owns its data outright, outlives the `PioSource`, and the two have no free ordering. `from_source` takes no options — they were fixed at open. `pio_balanced_read` on a multi-entry source returns NULL with a message naming `pio_source_open` and the count.

### Safety caps

```c
typedef struct PioReadOptions {
  size_t      struct_size;        /* sizeof(PioReadOptions) */
  const char *name_hint;          /* NULL: derive from the path or entry name */
  int32_t     refuse_symlinks;    /* 0: resolve then require containment; nonzero: refuse outright */
  int32_t     reserved0;          /* must be 0; nonzero is E2BIG */
  uint64_t    max_walk_depth;     /* 0 = default 8;      UINT64_MAX = uncapped */
  uint64_t    max_walk_entries;   /* 0 = default 65536;  UINT64_MAX = uncapped */
  uint64_t    max_walk_bytes;     /* 0 = default 4294967296; UINT64_MAX = uncapped */
} PioReadOptions;
```

Offsets LP64 `0,8,16,20,24,32,40`, size 48. ILP32 `0,4,8,12,16,24,32`, size 40. No implicit padding on either — required, because Julia zeroes fields, not padding. Pin both with `offset_of!` asserts. `0 = default` is a deliberate carve-out from rule 5's "defaults are per version, never per field value"; write the exception down, with `UINT64_MAX` as the encoding for "no cap".

Numbers: depth 8 (PyPSA is 1, gridfm's deepest accepted layout `parent/<case>/raw/*.parquet` is 3, a two-level corpus is 2); entries 65536 (gridfm is four files, a PGLib mirror is a few hundred — an OPFData release must raise it explicitly, which is the correct outcome for 300k files); bytes 4 GiB.

**Scope, corrected from the draft:** the caps apply to directory walks **and to OpenDSS include resolution**. The draft exempted single-file routes, which exempts the only shipped format that actually walks a tree. `raw.rs:686` (`if self.dirs.len() > MAX_REDIRECT_DEPTH`) is the sole bound today — depth only, no cap on include count or total bytes. Shipping without this, a directory of `.m` files is capped at 65536 entries and 4 GiB while an untrusted `.dss` keeps unbounded fan-out, which inverts the threat model that 0.7.3 and 0.8.2 exist to address. `pio_multiconductor_read` already takes the same struct, so it costs no symbol. `MAX_REDIRECT_DEPTH = 64` stays the internal nesting bound, distinct from `max_walk_depth`; `MAX_OBJECT_PROPS` and the 0.7.3 phase/winding caps stay content caps in their readers.

Containment reuses `confined_fs_read` (`powerio-dist/src/dss/raw.rs:961-977`: canonicalize, then `starts_with` the canonical root), hoisted so the walk, the DSS loader and the CLI share one check. `..` needs no separate rule — canonicalization resolves it and containment rejects the result. Two rules the walk needs that the include loader did not: only regular files are opened (a fifo makes `max_walk_bytes` unenforceable), and bytes are counted as read, not from a prior stat. If untrusted extracted trees are in scope, specify `openat` against a pinned dirfd rather than canonicalize-then-open, which is check-then-use and blind to hardlinks.

### When a future directory format arrives

**Directory of scripts, cardinality 1** (PSCAD, a PowerFactory export, `.raw` + `.dyr` + `.seq` siblings): **zero new symbols.** One `Route::Case` arm keyed on a marker file, one format token (a string, never a symbol), one reader. PyPSA is the shipped proof — a directory of CSVs entering through the same `parse_file` as a `.m` file (`mod.rs:440-444`).

**Directory of many unrelated cases:** a source with N entries, opened **only under an explicit `from`**. A bare directory with no marker stays the error it is today. This is the correction that matters: PGLib has no manifest, nor will a GOC3 division or an extracted OPFData release, so corpus detection can only be a content scan — and `pio_balanced_read` pointed at any directory must never silently become a filesystem walk. Entries may be different formats; `pio_source_entry_model` is what routes them, and the guidance error at `mod.rs:629-639` is demoted to what it is, the error a caller who ignored the discriminator gets. That message needs rewriting anyway, since it names `pio_dist_parse_file`, renamed in v5.

Resolution stays exactly-one-or-error, already the house rule (`io/gridfm.rs:1509-1554` errors on zero and on more than one). **0.9.0 ships `Route::Container(ContainerKind::Corpus)` as an unsupported variant** — the symbols are already the right shape, so the reader lands later as a `#[non_exhaustive]` variant plus a `CaseSource` impl, zero ABI change. Archives stay out (`Route::Archive` absent); entry-name traversal and decompression ratios do not belong in a first cut, and adding it is one enum variant.

### Rust and PowerIO.jl

`Route`, `SourceEntry` and `CaseSource` live in `powerio`; container impls live with their readers. There is no facade crate (`Cargo.toml` lists eight leaf members), so the ~20 line `match Route` dispatcher is per consumer with `powerio-capi` as the reference copy. Name the Rust entry `powerio::source::open_case` and say plainly that the universal open exists only in the three dispatchers — `powerio-matrix` depends on `powerio`, so a gridfm dataset handed to `powerio::source::open` would error where the C caller succeeds. `read_multiconductor` cannot be a trait method (`powerio` cannot name `MulticonductorNetwork`); `powerio_dist::read_source(&dyn CaseSource, index)` reads through `entry_path`, defined as the entry's **primary** file, with a bundle's siblings the reader's business.

The measured defect this fixes: `PowerIO.jl/src/gridfm.jl:59` maps `read_gridfm` over the scenario ids, each call re-reading all three Parquet files, for 3N+1 reads — against a Rust function whose own comment (`io/gridfm.rs:1126-1128`) says it exists to avoid exactly that. After: 3 reads regardless of N. In the Julia rewrite, bind the network **once** and pull warnings off its handle; the designer's sketch called `src[i]` twice and would have shipped a 2× regression in place of a 3N+1 one.

Every exported PowerIO.jl name survives. New: `open_source`, `entry_names`, `entry_name`. Deleted: `_gridfm_scenario_ids` and its probe-and-fill race guard. `PowerIO.jl/src/network.jl:209`'s untyped `parse_file` loses its duplicated model routing to `entry_model` — count it in the consumer impact, since `ExaModelsPower.jl/src/parser.jl:15` calls that method.

---

## 2. Geo

**No.** powerio's geo family cannot completely relieve tellegen of its geo layer, and should not. It absorbs the file-reading half completely — roughly 400 lines, including a second CSV reader and a second GeoJSON LineString reader living in the same repo as a wasm module that calls powerio's tolerant reader for the same file types — and none of the layout half, which is ~550 lines of Rust and TypeScript that belongs to a renderer.

### The boundary and why it falls there

The test: **does the answer depend on anything outside the case file and its sidecar?** If no, it is powerio's and two implementations are a bug. If it needs a viewport, a zoom level, or an aesthetic judgment, it is tellegen's.

powerio owns coordinate file parsing (every tolerant form, one reader); coordinate space declaration and the CRS string, recorded and carried; matching features to buses and branches, including the uid / external id / name / unordered endpoint ladder and the substation join; the apply report; canonical GeoJSON emission; and promotion of coordinates a source file carries.

tellegen owns layout of a coordinate-free case; jittering co-located buses; the map center, span and target bbox; every projection to screen; its `coords_kind` UI state machine; and its wasm and HTTP payload shapes. The project already wrote this half down at `docs/src/geo-and-display.md:164-167` ("PowerIO stores and transports coordinates; it does not compute them"). tellegen proves it holds an opinion by shipping two layouts with different aesthetics — a force pass in Rust, a tidy tree in TypeScript chosen because "A force pass turns a deep radial feeder into an illegible tangle".

Reprojection is the one thing that looks like canonicalization and is not: it needs a datum database, and the whole geospatial stack keeps it in a dedicated library — GDAL's own tutorial says reprojection "were only available if GDAL had been build against the PROJ library", with PROJ required since 3.0. The rule is therefore **not** "powerio never transforms": it is *powerio ships named, documented, format-specific lifts (today only `pwd_mercator_to_lonlat`, accurate to ~0.02°) whose output is stamped `CoordsKind::Derived` and never claims a CRS it cannot verify.*

**No extent bbox and no simplification.** Both were justified as "someone might"; neither has a caller. Douglas–Peucker needs a zoom level powerio cannot see.

**One rule for space inference, and it is the fix.** Today the same OpenDSS Buscoords values get two spaces depending on which door they enter: `powerio-dist/src/dss/read.rs:300-306` warns and leaves the space unknown "because Buscoords does not declare a CRS", while `powerio/src/geo/layer.rs:758-772` infers `Geographic { crs: None }` from the same ±180/±90 test. The BMOPF writer branches on exactly that (`powerio-dist/src/bmopf/write.rs:466-476`, `EMIT.BMOPF.BUS_LOCATION_DROPPED`). **Decision: inference lives in `powerio::geo::inferred_space` and the DSS reader adopts it.** Consequence, to name in the 0.9.0 notes: a `.dss` case whose buscoords are in lon/lat now emits BMOPF `longitude`/`latitude`, where it previously dropped them; coordinates in metres still stay Unknown and are still dropped, which is the honest answer.

### The Rust API tellegen calls in v0.2.0

```rust
impl GeoLayer {
    /// Build a layer from computed or hand-placed points; routes through
    /// apply_geo_layer, so a stamped layout gets the same matching and report
    /// as an applied file.
    pub fn from_bus_points<I>(points: I, space: CoordinateSpace, kind: Option<CoordsKind>) -> GeoLayer
    where I: IntoIterator<Item = (BusId, [f64; 2])>;

    /// Each feature counted once, by target.
    pub fn counts(&self) -> GeoCounts;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)] #[non_exhaustive]
pub struct GeoCounts { pub bus_points: usize, pub branch_routes: usize, pub substation_points: usize }

#[derive(Debug, Clone, Copy, PartialEq, Eq)] #[non_exhaustive]
pub enum PwdLayerSpace { Diagram, GeographicApprox }

/// GeographicApprox stamps CoordsKind::Derived on the layer and every feature.
pub fn geo_layer_from_pwd(display: &PwdDisplay, space: PwdLayerSpace) -> GeoLayer;

impl BalancedNetwork {
    /// Located buses in id order, with the space they are in. x/y, not lon/lat:
    /// a Diagram or Projected network returns diagram units.
    pub fn bus_points(&self) -> (BTreeMap<BusId, [f64; 2]>, CoordinateSpace);

    /// Explicit, not a parse-time join: the aux file is often a different
    /// document than the case being placed.
    pub fn join_substation_layer(&mut self, aux: &AuxFile) -> GeoApplyReport;
}
```

Three corrections to the designer's draft, all adopted. `bus_positions() -> BTreeMap<_,[f64;2]>` documented as "`[lon, lat]`, in row order" was wrong twice — a BTreeMap iterates in id order, and the model has four spaces, so lon/lat is a lie for exactly the `.pwd` case tellegen hits. `GeoCounts { points, routes, substations }` mixed geometry taxonomy with target taxonomy; every substation feature *is* a Point, so the sum was undefined either way. And the parse-time aux substation join does not serve the case it was cut for: `tellegen-server/src/lib.rs:952-957` reads a **separate** `.aux` and places a MATPOWER case from it, so a join at parse of the aux's own buses does nothing.

`powerio` already ships the substation join (`geo/pwd.rs:75-165`); what is missing is only that nothing calls it.

### The C symbols

```c
typedef struct PioGeoApplyReport {
    size_t struct_size;            /* caller sets sizeof(); library writes at most this many bytes */
    size_t matched_buses;
    size_t matched_branches;
    size_t unmatched_features;     /* a key that resolved to no element */
    size_t substation_features;    /* features seen with a substation target this pass */
    size_t replaced_locations;
    size_t unlocated_buses;        /* model-wide, after the pass */
    size_t unlocated_branches;
} PioGeoApplyReport;

PioConversion *pio_geo_parse(const uint8_t *bytes, size_t len, const char *name_hint,
                             char *errbuf, size_t errlen);

size_t pio_balanced_geo(const PioBalancedNetwork *net, char *out, size_t cap,
                        char *errbuf, size_t errlen);
size_t pio_multiconductor_geo(const PioMulticonductorNetwork *net, char *out, size_t cap,
                              char *errbuf, size_t errlen);

PioBalancedNetwork *pio_balanced_apply_geo(const PioBalancedNetwork *net,
                                           const uint8_t *layer, size_t layer_len,
                                           const char *name_hint,
                                           PioGeoApplyReport *report,
                                           char *errbuf, size_t errlen);
PioMulticonductorNetwork *pio_multiconductor_apply_geo(const PioMulticonductorNetwork *net,
                                                       const uint8_t *layer, size_t layer_len,
                                                       const char *name_hint,
                                                       PioGeoApplyReport *report,
                                                       char *errbuf, size_t errlen);
```

**`pio_geo_parse` keeps its name and gains `PioConversion *`.** `pio_geo_normalize` is disqualified by collision: `pio_balanced_normalize` in the same header is the per-unit/radian/filter/reindex transform (`docs/src/abi-v5-audit.md:208`), and the subject token disambiguates the noun, not the verb. GEOS agrees on the domain meaning — its readers are `GEOSWKTReader_read_r` / `GEOSWKBReader_read_r`, while `GEOSNormalize_r(handle, GEOSGeometry *g)` canonicalizes an already-parsed geometry in place. The return type change also dissolves the audit's original objection (`abi-v5-audit.md:288`, "the only `parse` in the ABI that returned a string rather than a handle") and stops the reader's notes being discarded — `powerio-capi/src/lib.rs:2021` returns the GeoJSON and nothing else, while the apply path keeps the warnings and Python keeps both.

**Bytes, not `const char *`.** The grammar says parse takes bytes, and `required_cstr` (`powerio-capi/src/lib.rs:379-381`) rejects non-UTF8 that `GeoLayer::parse_bytes` reads happily through `from_utf8_lossy`. A cp1252 GIS export the Rust and wasm paths read would be permanently unreachable from C. `GEOSWKBReader_read_r` takes `(const unsigned char *, size_t)`.

**`substation_features`, not `skipped_features`.** Freeze a positive fact. `GeoTarget` is `#[non_exhaustive]`; the day substations become applyable or a Load target lands, a field defined as "not this pass's target" silently goes to zero and every caller's arithmetic shifts. Substation features **stop counting in `unmatched_features`** — today `geo/layer.rs:930-936` does `report.unmatched_features += substations` and then explains itself in English at `:975`, which is why tellegen's message reads "no case elements matched the geographic file (N feature(s) unmatched)" for a file that was simply the wrong kind. That is a semantic change to the already-shipped Python key `unmatched_features`; call it out in the notes. `replaced_locations` is added for the same reason the others are: `geo/pwd.rs:122,152-155` already counts it and pushes it as prose.

**Out-param `struct_size` runs the opposite direction from options structs** and the header must say so: options are caller-filled and library-validated (E2BIG on a nonzero unknown tail); a report is caller-sized and library-written, and the library writes **at most** `struct_size` bytes. Getting this backwards corrupts a Julia stack frame. Pin the offsets with a const assert as rule 5 already requires.

Memoize the emitted GeoJSON on the handle, the way `pio_scopf_to_json`'s `OnceLock` does — otherwise the two-call size-query idiom serializes the whole document twice.

The English apply summary at `powerio-capi/src/lib.rs:2084-2093` goes away; `report.notes` still ride in the handle's warnings, because a coordinate-space change is genuinely prose.

### BMOPFTools

Narrower than the designer claimed, and it is a lockstep bump, not a free win.

It needs **one** symbol: `pio_multiconductor_apply_geo` with the report, which deletes their 38-line buscoords CSV reader on the `from_dss` route (`BMOPFTools.jl/src/io/from_dss.jl:79-80` already holds a multiconductor handle at the right moment).

What does **not** happen: `_strip_derived_bus_fields` does not disappear and `sideload_coordinates` does not become the default. The BMOPF schema declares `additionalProperties: false` on buses, which is why their stripping exists and why powerio says the same thing in its own source (`powerio-dist/src/bmopf/write.rs:126-128`, "The default stays schema strict because the BMOPF schema rejects these fields"). The lossiness note in their writer stays true — it is a fact about the schema, not about where the CSV reader lives. Their `n_skipped` already means `unmatched_features` (their tests assert `n_skipped == 0` for malformed rows); powerio is better on the malformed side because it warns rather than silently dropping, not because the counts were conflated. And their tests drive `parse_bmopf` on a JSON string with no DSS file anywhere, which the geo apply does not serve at all.

Behavior change to coordinate with them: their fixture rows are `650, 100.0, 200.0`. Under the single inference rule that stays Unknown and is dropped on write, where their current reader stored the numbers verbatim. Settle it before they bump.

**The C ABI ships the intersection of the two consumers; the Rust API ships the union.** Five geo symbols plus one struct cover BMOPFTools completely and cover everything of tellegen's that would ever cross a C boundary. No points-array stamping path in C — tellegen gets it in Rust, where it links.

### The tellegen diff

| file | now | after | what goes |
|---|---|---|---|
| `crates/tellegen/src/geo.rs` → `layout.rs` | 434 | ~200 | `network_coords`, `substation_coords`, `extra_f64`, `stamp_layout`, `pwd_lonlat_layer`, `complete_coords_for` (~110 lines) plus ~90 lines of their tests |
| `crates/tellegen-server/src/lib.rs` | 2007 | −~200 / +~25 | `load_bus_csv_coords`, `BranchPaths`…`json_number`, `normalize_key`, `valid_coord` |
| `crates/tellegen-wasm/src/geo.rs` | 436 | ~390 | the counting loop, the coords-to-network plumbing, the hand-pushed approximation note |
| `crates/tellegen-wasm/src/lib.rs` | — | −~10 | `network_coords` → `bus_points`, the per-symbol projection at `:600` |

~400 lines net. The rename is the honest summary: what is left is a layout module and tellegen no longer has a geo layer.

Three things the sketch got wrong and the diff must reflect. The server work **splits by arm**: only `CoordSpec::BusCsv` is "read bytes → `parse_bytes` → `apply_geo_layer` → `require_located`"; `CoordSpec::Aux` needs parse-the-aux → `join_substation_layer` → apply onto the case, which is why the substation join stays an explicit call. `Coords` must be **retyped** to `BTreeMap<BusId,[f64;2]>` in the same change, or every call site gains a remap and the deletion is not a deletion. And two server behaviors are **traded away, not matched**: `valid_coord`'s ±180/±90 vertex rejection (powerio uses those bounds only to infer, never to reject, because Diagram coordinates legitimately exceed them) and hard per-row failure with a `file:line` message (powerio skips the row and pushes a deduplicated warning capped at 16). A server staging fixtures at boot wants the loud failure. Decide that before deleting; §5(g).

`require_located` also loses the first-missing-bus-id diagnostic `complete_coords_for` gives. Add the id to `Error::UnlocatedElements` or accept the loss explicitly.

### Cannot move

`spread_stacks` (28 lines) — it moves buses off their true surveyed positions by a constant chosen for hover targets at street zoom, and a library whose rule is that readers do not invent coordinates must not ship it. The layouts (~120 Rust lines plus `synthetic-layout.ts`, 427) — graph drawing, and tellegen holds two deliberately different ones, so absorbing either freezes one renderer's aesthetic into a five-year ABI. `multiconductor.ts` placement — fits a drawing into a lon/lat box around the current map center. `coords_kind_token` and the `CoordsKind` union, including `synthetic_pending`, which means the user has not clicked the map yet and has no business in a data model. The wasm JSON envelope.

---

## 3. ExaModelsPower

### Branch and PR state

`origin/main` = `5f64c18` (#57, "form is a type, 0.4.0"). Local `feat/powerio-0.9-and-upstream-merge` pins `PowerIO = "0.9"`; merge base with main is `88aaada`, so **#57 is not in the branch**. #50 is OPEN and CONFLICTING, and its PR body still says "require PowerIO v0.6.1" while the branch says 0.9 — the open PR is two revisions behind the local work. #58 is MERGEABLE; #49 is CONFLICTING but touches no parser file.

#50 today: swaps ExaPowerIO for PowerIO as the parser, rewrites multi-period load handling onto `PowerIO.LoadSeries`, cuts `src/sc_parser.jl` 1512 → 542 and deletes `src/goc3_parser.jl` by routing GOC3 through `PowerIO.goc3_scopf_data` / `goc3_interval_bounds` / `goc3_add_status_flags!`. Net −1020/+554.

#57 breaks it four ways. **(a) Signature.** Main's `parse_ac_power_data(filename, ::Type{T})` is deliberate — `src/opf.jl:103-105` says "The positional method is the one a compiled library calls: `::Type{T}` makes `T` a static parameter, where the keyword form leaves it a `Type`-typed value that nothing downstream can specialize on." #50 reintroduces the value form. **(b) Deps:** main has no PowerIO at all. **(c) Files:** #57 deleted `src/dcopf.jl` (94 lines, DC folded into `opf.jl` as `struct DC <: OPFForm`) and renamed `src/scopf.jl` → `src/goc3.jl`; #50 edits both. **(d) Data flow:** #57 narrowed the model-facing NamedTuple to exactly the fields the model bodies read, because `convert_data`'s `map` gives up inference at 32 fields, and added `branch_rate_a`, `vmin2`, `vmax2`, `branch_ninf` in `opf_args`. PowerIO.jl's `parse_ac_power_data` returns the pre-#57 24-field shape, so it must feed `opf_args`, not replace it.

### Merge shape

Move the boundary: **PowerIO.jl provides the row tables, ExaModelsPower keeps the NamedTuple assembly** exactly as #57 wrote it.

PowerIO.jl adds a **positional** method alongside the keyword one — non-breaking, and it belongs in the 0.9.0 companion PR because that is the release ExaModelsPower pins:

```julia
to_powerdata(path::AbstractString, ::Type{T}; from=nothing, filtered::Bool=true) where {T}
to_powerdata(net::BalancedNetwork, ::Type{T}; filtered::Bool=true) where {T}
```

Without it the rebase makes `T` a static parameter in ExaModelsPower and then hands it to PowerIO.jl one frame lower in the exact keyword form #57 deleted (`PowerIO.jl/src/exa.jl:583`). `opf_args(filename, ::Type{T})` is the method #58's compiler extension compiles.

Post-rebase `src/parser.jl`:

```julia
parse_ac_power_data(filename) = parse_ac_power_data(filename, Float64)
function parse_ac_power_data(filename, ::Type{T}; from = nothing) where {T}
    raw = isfile(filename) ? PowerIO.to_powerdata(filename, T; from = from) :
          PowerIO.to_powerdata(joinpath(ExaPowerIO.get_path(:pglib), filename), T; from = from)
    return (; baseMVA = [raw.baseMVA], bus = raw.bus, ... )   # #57's body verbatim
end
```

**Keep `from`.** Dropping it costs #50 its entire headline ("broad non-MATPOWER format support") — under `opf_args`'s positional call there would be no channel for a `.raw` or `.dss` case to reach the AC OPF. It is inference-neutral: #57's comment is explicit that the hazard was a *return type* ("A `library` of `Union{Nothing,Symbol}` leaves `parse_matpower`'s return type uninferable"), and `parse_file` returns `BalancedNetwork` for every value of `from` while the row types are functions of `T` alone.

**Merge order: #58 → rebase #50 → #49.** #58 removes `opf_core`/`ac_opf_core`/`dcopf_core`/`mpopf_core` and takes `Nbus` out of `mpopf_recipe`, the same direction as #50's mpopf change; land it first so #50 rebases once. Budget `src/mpopf.jl` as a **rewrite, not a replay**: #57 moved it 594 lines, #58 another 35, #50 wants 146. Re-derive #50's `LoadSeries` change against #57+#58's `build_mpopf_body` signature, where `Nbus` is already gone. #49 lands independently; rename its new files around `scopf.jl` → `goc3.jl`.

### What ExaModelsPower reads, and the exact Arrow tables

Specification is `PowerIO.jl/src/exa.jl:92-206` — storage row type first at `:92-110`, then bus/gen/branch/arc, with the intent comment at `:113-135` enumerating the fields the C ABI would need. Provenance today is **100% JSON, 0% Arrow**: every accessor resolves to the `JSON3.Object` from `pio_to_json`.

Gaps confirmed against the catalog: `gen` (id 2, `arrow_export.rs:193`) and `solver_gen` (id 12, `:245`) carry **no cost column at all**; `solver_storage` (id 13, `:251`) stops at `q_loss` with no efficiencies; `solver_branch` (id 9) has no arc back-references though Rust already computed them (`solver_tables.rs:83-84`, `branch_from_arc_indices` / `branch_to_arc_indices`); `solver_bus` (id 6, `:210`) omits `area`/`zone`. Both decoder constraints hold: `List<Float64>` is `+l` in the Arrow C Data Interface and `PowerIO.jl/src/arrow.jl:88-92` accepts only `"l"`, `"g"`, `"C"`; `:285` errors on any non-zero `null_count`. So costs are flat primitive columns and a costless generator is never a null row.

**Three new ids, not six.** Column-append carries the scalars on tables that already have the right row axis:

```rust
pub const PIO_ARROW_TABLE_GEN_COST_COEFF: i32 = 21;
pub const PIO_ARROW_TABLE_SOLVER_GEN_COST_COEFF: i32 = 22;
pub const PIO_ARROW_TABLE_SOLVER_HVDC_COST_COEFF: i32 = 23;
```

Each: `(owner_index int64, value float64)`, one row per coefficient, rows grouped by owner and sorted, in curve order — polynomial highest-order-first, piecewise `p0,f0,p1,f1,…`, matching `GenCost`'s own documented layout.

Append to `gen` (2), `solver_gen` (12), `solver_hvdc` (14), in this order, all non-null:
`("cost_model","int64")`, `("cost_startup","float64")`, `("cost_shutdown","float64")`, `("cost_ncost","int64")`, `("cost_coeff_offset","int64")`, `("cost_coeff_count","int64")`.

A costless generator is `cost_model = 0, cost_ncost = 0, cost_coeff_count = 0`, offset at its slice start. No join, no absent rows, no `row_axis` untruth. The parent/child split with `row_axis = Some("solver_gen")` on a table *shorter* than `solver_gen` would have frozen a false axis into the published catalog, which `PowerIO.jl/src/arrow.jl:416-419` validates.

Both `ncost` and `coeff_count` ship — they are independent, because `GenCost::with_ncost` exists so a malformed source row keeps a declared `ncost` that disagrees with the vector, and the Rust readers guard on `coeffs.len() < ncost` before indexing. In solver space only `coeff_count < ncost` can occur, because `normalize.rs:172,181` truncates. `cost_model` is int64: the convention exports true booleans as uint8 and codes as int64.

Further appends, in priority order:
- `solver_branch += ("from_arc_index","int64"), ("to_arc_index","int64")` — Rust computed them; making a consumer invert `solver_arc` in Julia is the duplicated lowering this design exists to delete.
- `solver_storage += ("charge_efficiency","float64"), ("discharge_efficiency","float64")` — the multi-period model cannot be built without them.
- `solver_bus += ("area","int64"), ("zone","int64")` — optional; ship only with a catalog note that these are the Rust `usize` fields widened, so a later signed `area` is not a wire break. No model body reads them; the bridge row type declares them.

`current_rating` stays out — `Option<f64>` with no sentinel convention and no consumer.

New unit blocks `units_solver_cost()` / `units_source_cost()` = the existing block plus `"cost_scalar": "currency"` and `"cost_coefficient": "currency_per_power_to_the_order"`, attached to the coefficient tables. Do **not** edit `units_solver()` — it is shared by nine tables whose frozen unit document must not move. Solver-space coefficients are already per unit (`normalize.rs:169-191` scales polynomial coefficient *i* by `base^(k−1−i)` and divides piecewise MW breakpoints); say so in the new block so nobody rescales twice.

Also fix `arrow_export.rs:55-57` while in the file: "new columns append (nullable) at the end" contradicts `:170` (`"nullable": false` on every column) and the only decoder.

### How much of exa.jl retires

**~200 lines, and the default path stops decoding JSON.** Do not sell it as more.

`_to_powerdata_normalized` (`exa.jl:208-365`, 158 lines) retires: the bus id → dense index dict is `solver_bus.index`; the load and shunt aggregation is `solver_bus.pd/.qd/.gs/.bs`; the kept-gen and kept-branch filters are the normalize pass; `f_idx`/`t_idx` become the appended arc columns; the arc build is `solver_arc`. What survives is ~60 lines of row-constructor comprehensions, because ExaModelsPower wants array-of-structs on a `CuArray` and Arrow is struct-of-arrays. That transpose stays.

Cost helpers (`:16-68`, 53 lines) go to **~35, not 15**. The `base^(k-i)` scaling loop is already dead on the fast path (guarded by `if !normalized`, and `:258` passes `normalized=true`), so Arrow deletes none of it — and it is the only live caller on the `filtered=false` branch, so the function cannot be deleted at all. `_cost_tuple`'s model dispatch, `ncost` trimming, leading-zero stripping, `> 3` rejection and 3-slot pad encode ExaModelsPower's quadratic-only objective, not transport. **Add `_cost_tuple` to the "cannot move" list.**

`_load_alignment` (`:689-695`) currently calls the full `to_powerdata` to obtain three vectors, on the multi-period hot path. **Do not migrate it in the same step.** It feeds `bus_ids` into `LoadSeries`, and `_check_load_matrix` validates a **user-supplied** `nbus × N` matrix against that length — so moving it onto `solver_bus` throws `DimensionMismatch` on a user's own Pd matrix for any case with a 3-winding transformer, and the appended star buses enter the series with `pd = qd = 0`. Migrate it once PowerIO.jl exposes which `solver_bus` rows are synthetic (`solver_tables.rs:86`, `bus_source_rows[i] === nothing`), filtering them out.

Cannot move: `_branch_coeffs` (`c1..c8` encode ExaModelsPower's branch flow formulation, not case data), the five row-type declarations (`:92-206`, the bridge schema), the `filtered=false` branch (`:389-580`, source units, its own REF/PV/PQ reassignment, and the source-space Arrow tables carry no bus-level aggregation — leave it on JSON and consider retiring the keyword, which ExaModelsPower never sets), and `LoadSeries` (`:624-836`).

**One behavior change.** `to_powerdata` walks the normalized network's bus list; `solver_bus` walks the `IndexedNetwork` view, which star-lowers 3-winding transformers, appending a bus, its branches and a magnetizing shunt with no source row (`normalize.rs:77-78`). On any case with a 3-winding transformer the two disagree in bus count. MATPOWER and PGLib have none; PSS/E RAW cases do. Same star-lowered space `pio_balanced_n_buses` already moves to, so land both in one release.

**The Arrow path does not change any number.** The designer's claim that per-unit division moves from Julia to Rust is false — `to_powerdata` calls `to_normalized` first (`exa.jl:384-385`) and does no per-unit division on the filtered path; `base` appears five times and none of them divide. The values are the same f64s through a buffer instead of a JSON round trip, which is exact. That is what makes the field-for-field regression gate writable, and it must not appear in the release notes as "the numbers moved" — that invites consumers to loosen tests that should stay exact.

### Landing order

**powerio v0.9.0** — all additive, merges last, parallel with the rename work:
1. Column appends only: `solver_branch` arc indices, `solver_storage` efficiencies, optionally `solver_bus` area/zone. No new ids.
2. Fix the `:55-57` comment; the `capi-arrow.md` catalog fixes already scheduled before #321.
3. Cost tables (ids 21–23, the six appended cost columns, the two unit blocks) — **gated**: they land in 0.9.0 only after a PowerIO.jl prototype has rebuilt `_to_powerdata_normalized` against the parent/child shape on case14, case118 and case9241_pegase. Ids are the one thing the freeze forbids re-cutting; a wrong column order is permanent. If Stage 2 runs out of room, the ids slip to 0.9.1 and the appends still ship.

**PowerIO.jl 0.9.0 companion PR** (one PR — `test_release.jl` gates changelog/version consistency): the six ids in `_ARROW_TABLE_IDS`; the positional `to_powerdata(path, ::Type{T})` methods; `_to_powerdata_normalized` rewritten against `to_arrow` with the JSON path kept as `filtered=false`; the regression gate asserting Arrow-built and JSON-built agree field for field before the JSON default is deleted.

**ExaModelsPower**, after PowerIO.jl 0.9.0 is in General: merge #58, rebase #50 onto #57+#58 with the parser shape above keeping `PowerIO = "0.9"`, then #49. #50 needs no knowledge of Arrow — the fast path is entirely inside PowerIO.jl.

**Blocker to check before tagging:** `c_active_power_balance_dc(b) = b.pd + b.gs` reads the bus shunt conductance as a power injection, and #312 `dc-shunt-conductance` is in the 0.9.0 stack with "bus shunt conductance in DC OPF" already listed as a user-visible change (`docs/src/abi-v5-review.md:287`). Diff #312 against that reader. No AC form reads `gs` as a power, so every test in the migration list is blind to it.

---

## 4. Delta against the review's §B symbol table

Only rows that change.

**Source family (§B `:119-129`) — 5 symbols → 7.**

| row | change |
|---|---|
| `pio_source_open` | now **universal** (every path, not datasets only) and **lazy** — enumerate at open, materialize at `from_source`. `from` names the entry format only; container selection is structural, container override is a later appended `PioReadOptions` field. |
| `pio_source_count` | drop "infallible because open is eager" and drop the `>= 1` invariant. 0 is a legal count for an empty container. |
| `pio_source_entry_ids` | **deleted.** `int64_t` is gridfm's Parquet key type, not a container property; PGLib / GOC3 / OPFData entries have no integer key. |
| — | **new** `pio_source_entry_names(src, out, cap, errbuf, errlen)` — NUL-separated, cap/total idiom, the one enumeration. |
| — | **new** `pio_source_entry_name(src, index, out, cap, errbuf, errlen)` — defined as the i-th field of the above. |
| — | **new** `pio_source_entry_model(src, index) -> int32_t` (`PioEntryModel`). Model choice is a discriminator, never a format-token table or an error string; this is what deletes `PowerIO.jl/src/network.jl:213-221`. |
| `pio_source_find` | signature changes to `(src, const char *name, size_t *index, errbuf, errlen)`. |

**Multiconductor family (§B `:141-158`).**

| row | change |
|---|---|
| — | **new** `pio_multiconductor_from_source(src, index, errbuf, errlen)`. |
| `pio_multiconductor_apply_geo` | takes `(const uint8_t *layer, size_t layer_len, ...)` and a `PioGeoApplyReport *`. |

**Balanced family (§B `:76-117`).**

| row | change |
|---|---|
| `pio_geo_apply` → `pio_balanced_apply_geo` | takes bytes, not `const char *`. |
| `pio_geo_extract` → `pio_balanced_geo` | unchanged name; the emitted document is memoized on the handle so the two-call idiom does not serialize twice. |

**Geographic and Arrow (§B `:198-204`).**

| row | change |
|---|---|
| `pio_geo_parse` | signature becomes `(const uint8_t *bytes, size_t len, const char *name_hint, errbuf, errlen)`. Name and `PioConversion *` return stand. |

**Structs and macros (§B `:206-232`).**

| row | change |
|---|---|
| `PioReadOptions` | walk caps are **active, not reserved**, and apply to OpenDSS include resolution as well as directory walks. Final layout and defaults in §1. Add `reserved0` ("must be 0"), `UINT64_MAX` = uncapped, and the `0 = default` carve-out from rule 5 written down. |
| `PioGeoApplyReport` | `struct_size` + **seven** counts, not five: `matched_buses`, `matched_branches`, `unmatched_features`, `substation_features`, `replaced_locations`, `unlocated_buses`, `unlocated_branches`. Substation features leave `unmatched_features`. Header states the **out-param** `struct_size` rule (library writes at most `struct_size` bytes) as distinct from the options rule. |
| — | **new** `PioEntryModel` enum. |
| `PIO_ARROW_TABLE_*` (21) | → **24**: `GEN_COST_COEFF = 21`, `SOLVER_GEN_COST_COEFF = 22`, `SOLVER_HVDC_COST_COEFF = 23`, plus appended columns on ids 2, 6, 9, 12, 13, 14. |

**Counts.** Functions 80 → 84 (`entry_ids` out; `entry_names`, `entry_name`, `entry_model`, `multiconductor_from_source` in) — the same 84 as v4, by coincidence. Handles 6, structs 4, macros 23 → 26. Update §B's closing arithmetic paragraph along with its three existing errors.

**Also to fold in:** `docs/src/abi-v5-audit.md:288-290` still lists the superseded `pio_geo_normalize`, `pio_balanced_geo_extract`, `pio_balanced_geo_apply`. Two documents must not disagree in a frozen release.

---

## 5. Still needs you

**(a) May a source ever open over a single document?** PowerModels multinetwork is N networks in one text file, dropped today with a warning (`powermodels.rs:468-473`), and GOC3 could be read per interval instead of first-only. If yes, the "one text file = 1" row needs its exception written now. **Recommend no for v5**: `PioSource` is the path-level container, multinetwork stays a warning, and a document-level series is `PioPackage`'s problem. Say it in the header or 2028 relitigates it.

**(b) One space-inference rule, and the DSS reader adopts it.** Today `dss/read.rs:300-306` keeps Buscoords Unknown while `geo/layer.rs:758-772` infers Geographic from the identical ±180/±90 test, and the BMOPF writer branches on the difference. **Recommend: inference lives in `powerio::geo` and both readers call it.** Value change: `.dss` cases with lon/lat buscoords start emitting BMOPF `longitude`/`latitude`. Coordinate with BMOPFTools, whose fixture coordinates are metres and will stay dropped.

**(c) Cost-table ids in 0.9.0, gated.** You decided the Arrow cost tables ship. **Recommend gating the three new ids on a PowerIO.jl prototype** against case14/case118/case9241_pegase before the id assert lands; the column appends ship unconditionally in 0.9.0. Ids are the only unrecuttable artifact in the set. If the gate slips, ids go to 0.9.1 and nothing else moves.

**(d) `sideload_coordinates` through `to_format`.** It would need a field in `PioWriteOptions` (which ships with `struct_size` only), and a BMOPF-specific boolean as the first field of the generic write struct is the field four more format flags will imitate. **Recommend deferring to 1.x**; keep it Rust and Python only for 0.9.0, and let BMOPFTools keep its strip-and-reattach, which the schema requires regardless.

**(e) Corpus requires an explicit `from`.** A bare directory with no marker file stays the error it is today, so no path silently becomes a filesystem walk. **Recommend yes**; `Route::Container(ContainerKind::Corpus)` ships as an unsupported variant.

**(f) The PowerIO.jl `v09/abi-v5` companion branch does not exist.** `git branch -a` shows only `v09/one-version` and `v09/goc3-instance-completeness`; `v09/one-version` is the companion for powerio #318, not for the ABI work, and `julia-binding.yml` matches by branch **name**. **Recommend creating it now, during Stage 2**, per step 20 — the two must be green together, which is the whole point of the trick.

**(g) The tellegen server strictness trade.** Deleting `load_bus_csv_coords` gives up ±180/±90 vertex rejection and hard per-row failure with a `file:line` message, in exchange for tolerant-with-warnings capped at 16. **Recommend accepting the trade** (a fixture that fails at boot fails just as loudly on `require_located`) **and adding the first missing bus id to `Error::UnlocatedElements`**, which is the one diagnostic `complete_coords_for` has and `require_located` does not.

**(h) PowerIO.jl gains positional `to_powerdata(path, ::Type{T})`.** Non-breaking addition, but it is API surface in the release ExaModelsPower pins, and without it the #50 rebase reintroduces the exact inference hazard #57 removed. **Recommend shipping it in the 0.9.0 companion PR.**