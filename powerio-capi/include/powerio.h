/* powerio C ABI, version 4: parse any power system case format, query it,
 * convert it, and extract the numeric tables for matrix assembly. Check
 * pio_abi_version() against PIO_ABI_VERSION at load; the integer is the
 * compatibility check, the version string is informational.
 *
 * Naming grammar (fixed; the surface evolves additively from here):
 * - Verb-led names are operations and the verb fixes the return family:
 *   parse/read/normalize return a new handle, write has a filesystem effect,
 *   convert transcodes without keeping a handle, free destroys.
 * - to_ marks a representation change of the same network (the strtol/htons
 *   lineage). The target is a format string unless the output type differs:
 *   pio_to_format returns text for every named format, pio_to_arrow fills
 *   Arrow C Data Interface structs.
 * - Noun phrases are queries (no get_); n_ prefixes counts, is_ predicates.
 * - Format names never appear in symbols. Formats are strings ("matpower",
 *   "psse", "powerio-json", ...), so a new format never changes this ABI.
 *
 * Vocabulary (one meaning per word, transmission and distribution alike):
 * - bus: a named connection point, any number of phases. This surface is bus
 *   granular (pio_n_buses, pio_bus_ids, pio_bus_demand, ...).
 * - node: one conductor's point at a bus (OpenDSS bus.1.2.3). Reserved for
 *   the multiconductor surface; never a synonym for bus here.
 * - branch: any two-terminal series element, lines and transformers alike
 *   (circuit theory; MATPOWER mpc.branch; the Branch Flow Model). "line" is
 *   the transformer-excluding subset and never names the umbrella table.
 *
 * Conventions:
 * - Array extractors write up to `cap` values per output array and return the
 *   total available; NULL out (or cap 0) is a pure count query, so a short
 *   read is detectable and a caller buffer can never silently overflow.
 * - Bus ids are int64 in the range 1..2^63-1 (a v4 invariant). pio_bus_ids and
 *   every per-bus column keyed to its ordering are int64; a source whose ids are
 *   strings or exceed 2^63-1 has no representation on this surface and is mapped
 *   to dense int64 at read (with a warning) or routed through the multiconductor
 *   surface. Never hand a raw oversized id to this surface.
 * - errbuf/errlen caller buffers (the libpcap/curl idiom: allocation-free
 *   across the boundary, no thread-local state). NULL or length 0 discards
 *   the message; a long message truncates on a UTF-8 character boundary and
 *   is always NUL-terminated. PIO_ERRBUF_MIN is a comfortable size. The ABI
 *   reports errors as messages and defines no error codes.
 * - Warnings attach to the network handle; query them with pio_warnings,
 *   which returns the byte length needed (call with NULL/0 to size). Only
 *   functions returning no handle (pio_to_format, pio_convert_*,
 *   pio_write_dir) take a warnbuf.
 * - Strings returned by pio_to_format / pio_convert_file / pio_convert_str
 *   are owned by the library; free them with pio_string_free. Handles from
 *   pio_parse_file / pio_parse_str / pio_read_dir / pio_normalize are freed
 *   with pio_network_free. Arrow buffers are freed through their own release
 *   callbacks (the C Data Interface release rule).
 * - A handle is immutable after construction: concurrent reads from any
 *   number of threads are safe; pio_network_free is not, free exactly once.
 * - Every entry point catches Rust panics at the boundary and returns the
 *   documented failure value (NULL, 0, -1, 0.0) rather than unwinding across
 *   the ABI (requires the default panic = "unwind"; a panic = "abort" build
 *   aborts the process instead).
 *
 * Evolution policy: existing signatures are frozen. New data means new
 * symbols; rich or multiconductor data rides the Arrow C Data Interface
 * (pio_to_arrow) and the powerio-json snapshot, whose schemas carry their own
 * structure and grow without touching a C signature. The dense extractors are
 * the frozen balanced positive-sequence projection, complete as-is. The Arrow
 * tables and the powerio-json schema are append-only: the existing
 * PIO_ARROW_TABLE_* ids and each table's column order are frozen, a new table
 * takes the next id and new columns append (nullable) at the end, and a
 * consumer addresses columns by name, never by position. The powerio-json
 * snapshot adds only optional or defaulted fields; existing field names, types,
 * and wire forms stay fixed. This is what PIO_ABI_VERSION freezes.
 *
 * Optional: build with `--features arrow` for pio_to_arrow (guarded by
 * PIO_ARROW), `--features gridfm` for pio_read_dir / pio_scenario_ids
 * (guarded by PIO_GRIDFM), `--features dist` for the pio_dist_* entry
 * points (guarded by PIO_DIST): multiconductor distribution cases (OpenDSS,
 * PMD ENGINEERING JSON, BMOPF JSON) behind their own PioDistNetwork handle,
 * freed with pio_dist_network_free, string outputs freed with pio_string_free,
 * and `--features pkg` for the pio_package_* entry points (guarded by
 * PIO_PKG): `.pio.json` compiler packages behind their own PioPackage handle,
 * freed with pio_package_free.
 * The distribution surface is EXPERIMENTAL while the IEEE BMOPF schema is a
 * draft: supported dist C usage starts at PIO_DIST_ABI_VERSION = 1, with
 * pio_dist_convert_*(input, from, to, ...). Dist C signature changes bump
 * PIO_DIST_ABI_VERSION, not PIO_ABI_VERSION. Its JSON payloads (bmopf-json,
 * powerio-dist-json) carry their own meta.version and may evolve; pin a
 * vintage from the payload meta.
 * Probe optional surfaces at runtime with
 * pio_has_feature("arrow"|"gridfm"|"dist"|"pkg").
 *
 * Checked in and generated; regenerate from the Rust source with
 *   cbindgen --config cbindgen.toml --crate powerio-capi --output include/powerio.h
 */

#ifndef POWERIO_H
#define POWERIO_H

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#if defined(PIO_ARROW)
struct ArrowArray;
struct ArrowSchema;
#endif

/**
 * ABI version of this C interface. Bump on any breaking change to an existing
 * `pio_*` signature or to the `powerio-json` snapshot schema (new additive
 * symbols don't require a bump). A consumer compares [`pio_abi_version`]
 * against the value it was built against (the `PIO_ABI_VERSION` macro in
 * `powerio.h`) and refuses a mismatched library instead of calling in blind.
 *
 * v4 froze the naming grammar and conventions (see the header preamble); the
 * surface evolves additively from here: new data means new symbols, and rich
 * or multiconductor data rides the Arrow and `powerio-json` schemas, which
 * carry their own structure and never force a signature change.
 */
#define PIO_ABI_VERSION 4

#if defined(PIO_DIST)
/**
 * ABI version of the optional `pio_dist_*` C surface. This is separate from
 * [`PIO_ABI_VERSION`] so distribution C entry points can evolve without forcing
 * a core ABI bump. Version 1 is the supported dist surface with conversion
 * order `(input, from, to, ...)`.
 */
#define PIO_DIST_ABI_VERSION 1
#endif

/**
 * A comfortable error-buffer size: pass a `char[PIO_ERRBUF_MIN]` to any
 * `errbuf`/`warnbuf` parameter and a message always fits without truncation.
 */
#define PIO_ERRBUF_MIN 256

#if defined(PIO_ARROW)
/**
 * Table selectors for [`pio_to_arrow`](crate::pio_to_arrow); the C
 * header mirrors these as `PIO_ARROW_TABLE_*`.
 */
#define PIO_ARROW_TABLE_BUS 0
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_BRANCH 1
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_GEN 2
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_LOAD 3
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SHUNT 4
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SWITCH 5
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SOLVER_BUS 6
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SOLVER_LOAD 7
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SOLVER_SHUNT 8
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SOLVER_BRANCH 9
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SOLVER_SWITCH 10
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SOLVER_ARC 11
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SOLVER_GEN 12
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SOLVER_STORAGE 13
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_SOLVER_HVDC 14
#endif

#if defined(PIO_DIST)
/**
 * Opaque parsed distribution network handle (the multiconductor wire-coordinate
 * model). Distinct from [`PioNetwork`] (the positive-sequence transmission
 * model); none of the `pio_n_*`/extractor functions accept it. Only built with
 * the `dist` cargo feature.
 */
typedef struct PioDistNetwork PioDistNetwork;
#endif

/**
 * Opaque parsed network handle. Carries the parsed [`Network`], the
 * [`IndexCore`] derived from it once at parse time (so every indexed query
 * reuses the same bus-id map and per-bus aggregates instead of rebuilding
 * them), and the reader's fidelity warnings ([`pio_warnings`]).
 */
typedef struct PioNetwork PioNetwork;

#if defined(PIO_PKG)
/**
 * Opaque `.pio.json` compiler package handle. A package owns one
 * [`powerio_pkg::CompilerPackage`], which wraps either a balanced
 * [`PioNetwork`] payload or a multiconductor [`PioDistNetwork`] payload.
 */
typedef struct PioPackage PioPackage;
#endif

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * The ABI version the library was built with (see [`PIO_ABI_VERSION`]). Lets a
 * consumer detect a stale or incompatible library at load time. Infallible.
 */
uint32_t pio_abi_version(void);

#if defined(PIO_DIST)
/**
 * The ABI version of the optional `pio_dist_*` surface. Only linked when the
 * `dist` feature is compiled in; probe that first with `pio_has_feature("dist")`
 * if loading dynamically.
 */
uint32_t pio_dist_abi_version(void);
#endif

/**
 * Whether an optional build feature is compiled in: pass `"arrow"`, `"gridfm"`,
 * `"dist"`, or `"pkg"`. Returns 1 if present, 0 otherwise (and 0 for a NULL or
 * unknown name). The optional surfaces (`pio_to_arrow`, the `pio_read_dir`/
 * gridfm path, the `pio_dist_*` block, and the `pio_package_*` block) are only
 * linked when their feature is built, so a consumer that loaded the library at
 * runtime probes for them here instead of resolving symbols blind. Feature
 * names are strings like format names, so a new feature never changes this
 * signature. Infallible.
 */
int32_t pio_has_feature(const char *feature);

/**
 * The crate version string (a semver string), `'static` and NUL-terminated. Do
 * NOT free it. Informational; pair it with [`pio_abi_version`] for the actual
 * compatibility check.
 */
const char *pio_version(void);

/**
 * Parse `path` (format from extension, or `from` if non-NULL) into a network
 * handle. `from` accepts the [`pio_parse_str`] format names plus
 * `pypsa-csv`/`pypsa` and `pwb`; that includes `pslf`/`epc`, and `.epc` is
 * inferred by extension. A PyPSA CSV folder is a directory, so it can only
 * enter through this function, with `from = "pypsa-csv"` (or NULL when the
 * directory holds a `network.csv`). Read fidelity warnings attach to the
 * handle ([`pio_warnings`]). Returns `NULL` on error and writes the message
 * into `errbuf`. Free the handle with [`pio_network_free`].
 */
PioNetwork *pio_parse_file(const char *path, const char *from, char *errbuf, size_t errlen);

/**
 * Parse in-memory case `text` of the named `format` into a network handle.
 * Unlike [`pio_parse_file`] there is no path to infer from, so `format` is
 * required: one of `matpower`/`m`, `powermodels`/`pm`, `egret`,
 * `pandapower-json`/`pandapower`/`pp`, `psse`/`raw`, `powerworld`/`aux`,
 * `pslf`/`epc`, or `powerio-json`/`json` (the canonical snapshot
 * [`pio_to_format`] writes, validated on read). PyPSA CSV folders are
 * directories, not text; parse them with [`pio_parse_file`] and
 * `from = "pypsa-csv"`. Read fidelity warnings attach to the handle
 * ([`pio_warnings`]). Returns `NULL` on error and writes the message into
 * `errbuf`. Free the handle with [`pio_network_free`].
 */
PioNetwork *pio_parse_str(const char *text, const char *format, char *errbuf, size_t errlen);

#if defined(PIO_GRIDFM)
/**
 * Read one scenario of a dataset directory in the named `from` format into a
 * network handle: the directory sibling of [`pio_parse_file`]. `gridfm` (the
 * gridfm-datakit Parquet layout; `dir` resolves leniently: the `raw/` leaf,
 * a `<case>/` directory with a `raw/` child, or a parent holding exactly one
 * such case) is the one dataset format today. `scenario` selects within a
 * multi-scenario dataset ([`pio_scenario_ids`] enumerates them); formats
 * without scenarios take `0`. Read fidelity warnings attach to the handle
 * ([`pio_warnings`]). Returns `NULL` on error and writes the message into
 * `errbuf`. Free the handle with [`pio_network_free`]. Built
 * `--features gridfm`.
 */
PioNetwork *pio_read_dir(const char *dir,
                         const char *from,
                         int64_t scenario,
                         char *errbuf,
                         size_t errlen);
#endif

#if defined(PIO_GRIDFM)
/**
 * Write the distinct scenario ids (ascending) of the dataset directory `dir`
 * in the named `from` format into `out`, up to `cap` entries, and return the
 * total count: the cap/count convention of [`pio_bus_ids`]. `gridfm` is the
 * one dataset format today. Returns `-1` on error and writes the message into
 * `errbuf` (unlike the handle extractors, this reads the filesystem and can
 * fail). Built `--features gridfm`.
 */
ptrdiff_t pio_scenario_ids(const char *dir,
                           const char *from,
                           int64_t *out,
                           size_t cap,
                           char *errbuf,
                           size_t errlen);
#endif

/**
 * The fidelity warnings attached to the handle at construction (by whichever
 * of [`pio_parse_file`], [`pio_parse_str`], [`pio_read_dir`], or
 * [`pio_normalize`] built it), `\n`-joined into `warnbuf` (truncated to fit
 * on a UTF-8 boundary; NULL/0 to skip). Returns the byte length of the full
 * joined text, excluding the NUL; call once with `(NULL, 0)` to size, then
 * pass a `char[len + 1]`. `0` means no warnings (or a NULL handle); readers
 * that are total attach none.
 */
size_t pio_warnings(const PioNetwork *net, char *warnbuf, size_t warnlen);

/**
 * Free a network handle from [`pio_parse_file`], [`pio_parse_str`],
 * [`pio_read_dir`], or [`pio_normalize`].
 */
void pio_network_free(PioNetwork *net);

/**
 * Normalize `net` into a NEW network handle: per unit, radians, out-of-service
 * filtered, source bus ids preserved, bus types canonicalized (see
 * `Network::to_normalized`). A value transform, not a serialization, hence
 * the verb, while the `to_*` family re-encodes unchanged data. The result is
 * independent of `net`; free both with [`pio_network_free`]. Every extractor
 * and serializer works on it unchanged (the handle is per unit, not MW).
 * Returns `NULL` on error (no reference bus can be chosen, or a non-positive
 * base MVA) and writes the message into `errbuf`.
 */
PioNetwork *pio_normalize(const PioNetwork *net, char *errbuf, size_t errlen);

size_t pio_n_buses(const PioNetwork *net);

size_t pio_n_branches(const PioNetwork *net);

size_t pio_n_switches(const PioNetwork *net);

size_t pio_n_gens(const PioNetwork *net);

double pio_base_mva(const PioNetwork *net);

/**
 * Dense `[0, n)` index of the single reference (slack) bus, or `-1` if not
 * exactly one. An INDEX into the [`pio_bus_ids`] ordering, not a bus id;
 * `pio_branches` from/to carry ids, so the unit is in the name. A network may
 * carry several references (one per island, or a normalized case that kept
 * the file's multiple `REF` buses); [`pio_ref_bus_indices`] reads them all,
 * and its count (`NULL` out) tells zero from many.
 */
int64_t pio_ref_bus_index(const PioNetwork *net);

/**
 * Write the dense `[0, n)` indices of the reference (slack) buses, ascending,
 * into `out`, up to `cap` entries, and return the total count: the cap/count
 * convention of [`pio_bus_ids`]. `0` means none; `> 1` means one reference
 * per island or several fixed references in one island (a normalized case
 * always reports `>= 1`).
 */
size_t pio_ref_bus_indices(const PioNetwork *net, int64_t *out, size_t cap);

/**
 * Number of islands: connected components of the in-service topology.
 */
size_t pio_n_islands(const PioNetwork *net);

/**
 * `1` if the in-service topology is radial (every island a tree), else `0`.
 */
int32_t pio_is_radial(const PioNetwork *net);

/**
 * Serialize `net` to the named format `to`: the one text serializer; every
 * format is named by a string. Accepts the [`pio_parse_str`] names:
 * `matpower` is a byte-exact echo when the handle was parsed from MATPOWER,
 * and `powerio-json` is the canonical snapshot (validated by [`pio_parse_str`]
 * on the way back; the retained source text is the one field it omits). The
 * snapshot is lossless except for a non-finite `f64` (`Inf`/`NaN`), which JSON
 * cannot represent: it is written as `null`, named in a fidelity warning, and
 * then fails to read back; pass `warnbuf` to detect it.
 *
 * Returns the text as an owned C string (free with [`pio_string_free`]),
 * `NULL` on error (message into `errbuf`). Fidelity warnings, if any, are
 * written `\n`-joined into `warnbuf`; a returned string has no handle to
 * attach them to.
 */
char *pio_to_format(const PioNetwork *net,
                    const char *to,
                    char *warnbuf,
                    size_t warnlen,
                    char *errbuf,
                    size_t errlen);

/**
 * Convert the case file at `path` from format `from` (NULL to infer from the
 * path, as [`pio_parse_file`]) to format `to`, without keeping a handle.
 * Returns the converted text as an owned C string (free with
 * [`pio_string_free`]), `NULL` on error. Fidelity warnings, read side first,
 * are written `\n`-joined into `warnbuf`.
 */
char *pio_convert_file(const char *path,
                       const char *from,
                       const char *to,
                       char *warnbuf,
                       size_t warnlen,
                       char *errbuf,
                       size_t errlen);

/**
 * Convert in-memory case `text` from format `from` (required; there is no
 * path to infer from) to format `to`, without keeping a handle: the in-memory
 * sibling of [`pio_convert_file`]. Returns the converted text as an owned C
 * string (free with [`pio_string_free`]), `NULL` on error. Fidelity warnings,
 * read side first, are written `\n`-joined into `warnbuf`.
 */
char *pio_convert_str(const char *text,
                      const char *from,
                      const char *to,
                      char *warnbuf,
                      size_t warnlen,
                      char *errbuf,
                      size_t errlen);

/**
 * Write `net` into the directory `out_dir` as the named directory-shaped
 * format `to`: the directory sibling of [`pio_to_format`]. PyPSA CSV
 * (`pypsa-csv`/`pypsa`) is the one such format today; a text format name is
 * an error pointing back at [`pio_to_format`]. Returns `0` on success and
 * `-1` on error (message into `errbuf`). Fidelity warnings, if any, are
 * written `\n`-joined into `warnbuf`.
 */
int32_t pio_write_dir(const PioNetwork *net,
                      const char *to,
                      const char *out_dir,
                      char *warnbuf,
                      size_t warnlen,
                      char *errbuf,
                      size_t errlen);

/**
 * Free a string returned by [`pio_to_format`], [`pio_convert_file`], or
 * [`pio_convert_str`].
 */
void pio_string_free(char *s);

/**
 * Write the 1-based external bus ids, in dense order, into `out`, up to `cap`
 * entries, and return the total bus count. This ordering DEFINES the dense
 * index space every other per-bus array shares. Call once with `(NULL, 0)` to
 * size, allocate, then call again to fill. Ids are int64 in `1..2^63-1` (a v4
 * invariant); a source id that is a string or exceeds that range is mapped to
 * dense int64 at read, never passed through raw.
 */
size_t pio_bus_ids(const PioNetwork *net, int64_t *out, size_t cap);

/**
 * Write the branch table as parallel arrays, each up to `cap` entries, and
 * return the total branch count. A branch is any two-terminal series element
 * lines and transformers alike (a transformer has `tap != 0`). `from`/`to`
 * are 1-based bus IDS (the [`pio_bus_ids`] id space, not dense indices); map
 * them to dense matrix rows with the [`pio_bus_ids`] ordering. Any output
 * pointer may be NULL to skip that column; all NULL is the count query.
 */
size_t pio_branches(const PioNetwork *net,
                    int64_t *from,
                    int64_t *to,
                    double *r,
                    double *x,
                    double *b,
                    double *tap,
                    double *shift,
                    uint8_t *in_service,
                    size_t cap);

/**
 * Write the branch terminal charging table as parallel arrays, each up to
 * `cap` entries, and return the total branch count. Columns are p.u.
 */
size_t pio_branch_charging(const PioNetwork *net,
                           double *g_fr,
                           double *b_fr,
                           double *g_to,
                           double *b_to,
                           size_t cap);

/**
 * Write the switch table as parallel arrays, each up to `cap` entries, and
 * return the total switch count. `from`/`to` are external bus ids.
 */
size_t pio_switches(const PioNetwork *net,
                    int64_t *from,
                    int64_t *to,
                    uint8_t *closed,
                    double *thermal_rating,
                    double *current_rating,
                    double *pf,
                    double *qf,
                    double *pt,
                    double *qt,
                    size_t cap);

/**
 * Write the generator table as parallel arrays, each up to `cap` entries, and
 * return the total generator count. `bus` is the 1-based bus id (the
 * [`pio_bus_ids`] id space). Any output pointer may be NULL to skip.
 */
size_t pio_gens(const PioNetwork *net,
                int64_t *bus,
                double *pg,
                double *pmax,
                double *pmin,
                uint8_t *in_service,
                size_t cap);

/**
 * Write the per-bus demand aggregates (active `pd`, reactive `qd`, summed
 * over each bus's loads, dense [`pio_bus_ids`] order), each up to `cap`
 * entries, and return the total bus count. Either pointer may be NULL.
 */
size_t pio_bus_demand(const PioNetwork *net, double *pd, double *qd, size_t cap);

/**
 * Write the per-bus shunt aggregates (conductance `gs`, susceptance `bs`,
 * dense [`pio_bus_ids`] order), each up to `cap` entries, and return the
 * total bus count. Either pointer may be NULL.
 */
size_t pio_bus_shunt(const PioNetwork *net, double *gs, double *bs, size_t cap);

#if defined(PIO_ARROW)
/**
 * Export one network table over the Arrow C Data Interface: the `to_`
 * conversion whose output type is Arrow structs rather than a string, and the
 * bulk plane this ABI evolves on. Tables 0..5 are raw network tables; tables 6
 * and up are normalized solver tables with per unit/radian values and dense
 * zero based row ids. New or richer columns arrive in the Arrow schema, leaving
 * the C signatures fixed.
 *
 * `table` is one of the `PIO_ARROW_TABLE_*` selectors. Raw table columns use
 * EXTERNAL bus ids (the `pio_bus_ids` id space), not the gridfm schema. On
 * success (returns `0`),
 * `out_array` and `out_schema` are populated with owned C Data Interface
 * structs: ownership of the Arrow buffers transfers to the caller, both
 * `release` callbacks are non-NULL, and the caller MUST invoke each exactly
 * once when done (skipping one leaks; the structs outlive `pio_network_free`).
 * On error (returns `-1`) the message is written into `errbuf` and the
 * out-params are left untouched. Only built with the `arrow` cargo feature.
 */
int32_t pio_to_arrow(const PioNetwork *net,
                     int32_t table,
                     struct ArrowArray *out_array,
                     struct ArrowSchema *out_schema,
                     char *errbuf,
                     size_t errlen);
#endif

#if defined(PIO_PKG)
/**
 * Parse a `.pio.json` package file into an opaque package handle. This reads
 * only the package envelope; case format names still enter through
 * [`pio_parse_file`] / [`pio_dist_parse_file`] and package constructors.
 * Returns `NULL` on error and writes the message into `errbuf`. Free the handle
 * with [`pio_package_free`].
 */
PioPackage *pio_package_parse_file(const char *path, char *errbuf, size_t errlen);
#endif

#if defined(PIO_PKG)
/**
 * Parse in-memory `.pio.json` text into an opaque package handle. Returns
 * `NULL` on error and writes the message into `errbuf`. Free the handle with
 * [`pio_package_free`].
 */
PioPackage *pio_package_parse_str(const char *text, char *errbuf, size_t errlen);
#endif

#if defined(PIO_PKG)
/**
 * Free a package handle returned by `pio_package_*`. NULL is a no-op; free
 * exactly once.
 */
void pio_package_free(PioPackage *pkg);
#endif

#if defined(PIO_PKG)
/**
 * Serialize a package handle to compact `.pio.json`. Returns an owned C string
 * (free with [`pio_string_free`]) or `NULL` on error.
 */
char *pio_package_to_json(const PioPackage *pkg, char *errbuf, size_t errlen);
#endif

#if defined(PIO_PKG)
/**
 * Wrap a balanced [`PioNetwork`] handle in a `.pio.json` package. The C handle
 * name is historical; the payload is `powerio::BalancedNetwork`.
 * `include_solver_metadata != 0` attaches compact normalized solver table
 * metadata.
 */
PioPackage *pio_package_from_balanced_network(const PioNetwork *net,
                                              int32_t include_solver_metadata,
                                              char *errbuf,
                                              size_t errlen);
#endif

#if (defined(PIO_PKG) && defined(PIO_DIST))
/**
 * Wrap a multiconductor [`PioDistNetwork`] handle in a `.pio.json` package. The
 * C handle name is historical; the payload is
 * `powerio_dist::MulticonductorNetwork`.
 */
PioPackage *pio_package_from_multiconductor_network(const PioDistNetwork *net,
                                                    char *errbuf,
                                                    size_t errlen);
#endif

#if defined(PIO_PKG)
/**
 * Run the package semantic validation profile in place. Returns `0` on
 * success, `-1` on error.
 */
int32_t pio_package_validate(PioPackage *pkg, char *errbuf, size_t errlen);
#endif

#if defined(PIO_PKG)
/**
 * Return the package validation summary as JSON. The returned string is owned
 * by the library; free it with [`pio_string_free`].
 */
char *pio_package_validation_json(const PioPackage *pkg, char *errbuf, size_t errlen);
#endif

#if defined(PIO_PKG)
/**
 * Return the package structured diagnostics array as JSON. The returned string
 * is owned by the library; free it with [`pio_string_free`].
 */
char *pio_package_diagnostics_json(const PioPackage *pkg, char *errbuf, size_t errlen);
#endif

#if defined(PIO_PKG)
/**
 * Return the multiconductor-to-balanced lowering preflight report as JSON.
 * `base_mva` is the three phase system power base used for the balanced
 * per-unit projection. Returns `NULL` if the package is not multiconductor.
 */
char *pio_package_multiconductor_to_balanced_preflight_json(const PioPackage *pkg,
                                                            double base_mva,
                                                            char *errbuf,
                                                            size_t errlen);
#endif

#if defined(PIO_PKG)
/**
 * Lower a multiconductor package to a new balanced package. Call
 * [`pio_package_multiconductor_to_balanced_preflight_json`] first when the
 * caller needs structured blockers for unsupported inputs. `base_mva` is the
 * three phase system power base used for the balanced per-unit projection.
 */
PioPackage *pio_package_lower_multiconductor_to_balanced(const PioPackage *pkg,
                                                         double base_mva,
                                                         char *errbuf,
                                                         size_t errlen);
#endif

#if defined(PIO_DIST)
/**
 * Parse a distribution case file into a [`PioDistNetwork`] handle. The format
 * comes from `from` if non-NULL (`dss`, `pmd`, or `bmopf`), else from the file
 * itself: `.dss` is OpenDSS, and `.json` holding the ENGINEERING `data_model`
 * key is PMD JSON, otherwise BMOPF JSON. Returns `NULL` on error and writes the
 * message into `errbuf`. Free the handle with [`pio_dist_network_free`].
 */
PioDistNetwork *pio_dist_parse_file(const char *path,
                                    const char *from,
                                    char *errbuf,
                                    size_t errlen);
#endif

#if defined(PIO_DIST)
/**
 * Parse in-memory distribution case `text` of the named `format` (`dss`, `pmd`,
 * or `bmopf`; required, since there is no path to infer from). An OpenDSS
 * `Redirect`/`Compile` in `text` resolves against the current working directory.
 * Returns `NULL` on error and writes the message into `errbuf`. Free the handle
 * with [`pio_dist_network_free`].
 */
PioDistNetwork *pio_dist_parse_str(const char *text,
                                   const char *format,
                                   char *errbuf,
                                   size_t errlen);
#endif

#if defined(PIO_DIST)
/**
 * Free a distribution network handle from [`pio_dist_parse_file`] or
 * [`pio_dist_parse_str`]. NULL is a no-op; free exactly once.
 */
void pio_dist_network_free(PioDistNetwork *net);
#endif

#if defined(PIO_DIST)
/**
 * Parse warnings retained on the handle (everything the reader could not
 * represent or had to assume), `\n`-joined and written into the caller `warnbuf`
 * (truncated to fit, always NUL-terminated). Returns the total byte length of
 * the joined message; call with `NULL`/0 to size first, then fill — the same
 * idiom as [`pio_warnings`]. Returns 0 for a NULL handle.
 */
size_t pio_dist_warnings(const PioDistNetwork *net, char *warnbuf, size_t warnlen);
#endif

#if defined(PIO_DIST)
/**
 * Serialize `net` to distribution format `to` (`dss`, `pmd`, or `bmopf`).
 * Writing back to the format the handle was parsed from echoes the source text
 * byte for byte; a cross-format write reports every fidelity loss in `warnbuf`
 * (`\n`-joined). Returns the text as an owned C string (free with
 * [`pio_string_free`]), `NULL` on error.
 */
char *pio_dist_to_format(const PioDistNetwork *net,
                         const char *to,
                         char *warnbuf,
                         size_t warnlen,
                         char *errbuf,
                         size_t errlen);
#endif

#if defined(PIO_DIST)
/**
 * Convert distribution case `path` from optional source format `from` to format
 * `to`; see [`pio_dist_parse_file`] for the inference rules. Returns the
 * converted text as an owned C string (free with [`pio_string_free`]), `NULL` on
 * error. The warnings written `\n`-joined into `warnbuf` carry both the parse
 * warnings and the writer's fidelity losses (there is no handle to query them).
 */
char *pio_dist_convert_file(const char *path,
                            const char *from,
                            const char *to,
                            char *warnbuf,
                            size_t warnlen,
                            char *errbuf,
                            size_t errlen);
#endif

#if defined(PIO_DIST)
/**
 * Convert in-memory distribution case `text` of format `from` to format `to`
 * (both required; `dss`, `pmd`, or `bmopf`). The parameter order is input,
 * source, target, matching [`pio_dist_convert_file`]. Returns the converted text
 * as an owned C string (free with [`pio_string_free`]), `NULL` on error. The
 * warnings written `\n`-joined into `warnbuf` carry both the parse warnings and
 * the writer's fidelity losses (there is no handle to query them).
 */
char *pio_dist_convert_str(const char *text,
                           const char *from,
                           const char *to,
                           char *warnbuf,
                           size_t warnlen,
                           char *errbuf,
                           size_t errlen);
#endif

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* POWERIO_H */
