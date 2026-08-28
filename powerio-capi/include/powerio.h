/* powerio C ABI, version 6. Parse power system sources into module handles,
 * inspect and transform them through typed accessors, and write supported
 * formats. Check pio_abi_version() against PIO_ABI_VERSION at load; the
 * integer is the compatibility check, the version string is informational.
 *
 * The surface in one paragraph: pio_parse_file / pio_parse_str /
 * pio_parse_bytes compile one source into a PioModule of whichever built in
 * value family claims it. pio_module_kind names the value; the typed
 * accessors (pio_module_balanced_network, pio_module_multiconductor_network)
 * hand back independently owned network handles; pio_module_diagnostics is
 * the structured findings list; pio_module_write_str / pio_module_write_file
 * write a supported target, echoing the retained source bytes exactly for an
 * unchanged same format write; and pio_module_read_json / write_json carry
 * the stored .pio.json document.
 *
 * Naming grammar:
 * - pio_<handle noun>_<operation or field>: module, balanced_network,
 *   multiconductor_network, diagnostics/diagnostic, dc_data, error. Verb-led
 *   tails are operations; noun tails are queries; n_ prefixes counts, is_
 *   predicates.
 * - Case format names never appear in symbols. Formats are strings
 *   ("matpower", "psse", ...), so a new format never changes this ABI.
 * - bus: a named connection point, any number of phases. node: one
 *   conductor's point at a bus (OpenDSS bus.1.2.3), reserved for the
 *   multiconductor API. branch: any two-terminal series element, lines and
 *   transformers alike.
 *
 * Ownership grammar (one rule set for every handle):
 * - Every handle type has a retain/release pair. retain mints an independent
 *   handle over the same immutable value; release drops one handle;
 *   release(NULL) is a no-op. Releasing a parent never invalidates a child
 *   or a retained sibling.
 * - Accessors on a handle return borrowed pointers and spans valid until
 *   that handle's last release. Owned strings (type char *) are released
 *   with pio_string_release. Caller-fill buffer functions exist only where
 *   the caller's layout, subset, or unit view differs from the stored data
 *   (the dense element extractors); each writes at most `cap` entries and
 *   returns the total available, so NULL/0 is a count query.
 * - A handle is immutable after construction: concurrent reads from any
 *   number of threads are safe; releasing the same raw handle concurrently
 *   with any call on it is caller error.
 *
 * Error grammar (one channel):
 * - Every fallible entry point takes a PioError **error out parameter (NULL
 *   to ignore) and returns NULL, -1, or false on failure with a new error
 *   handle stored for the caller to release. pio_error_code is the stable
 *   dotted diagnostic identity, pio_error_message the rendered text, and
 *   pio_error_diagnostics the structured findings behind the failure. Branch
 *   on the code, never on the prose.
 * - Every entry point catches Rust panics at the boundary and reports
 *   BIND.CAPI.PANIC rather than unwinding across the ABI (requires the
 *   default panic = "unwind"; a panic = "abort" build aborts the process).
 *
 * Diagnostics: pio_module_diagnostics and pio_error_diagnostics return a
 * PioDiagnostics list handle whose rows expose code, severity name, message,
 * optional identity, target, suggested action, details JSON, byte spans into
 * the named source, and related record identities. The *_json twins remain
 * as explicit serialization helpers. A published code keeps its identity
 * forever and is never reused; an unknown code is data, never a failure.
 *
 * Options structs (PioNormalizeOptions, PioWriteOptions) are extensible, in
 * the Linux syscall convention of openat2/clone3: size_t struct_size is the
 * first field, the caller zero fills the struct and sets struct_size to
 * sizeof, and NULL for the parameter means every default. A nonzero field
 * beyond the library's own size fails the call. Fields are appended, never
 * reordered or removed.
 *
 * Compatibility policy: changing an existing signature or documented
 * behavior requires a PIO_ABI_VERSION increment. New data uses new symbols
 * or versioned Arrow, `.pio.json`, and format-specific JSON schemas. Arrow
 * tables are append only: existing PIO_ARROW_TABLE_* ids and column order do
 * not change; a consumer addresses columns by name, never by position.
 *
 * Optional features, probed at runtime with pio_has_feature("arrow" |
 * "matrix" | "gridfm" | "dist" | "prob"): `arrow` adds
 * pio_balanced_network_to_arrow (guarded by PIO_ARROW), `matrix` the
 * balanced matrix Arrow tables, `gridfm` the GridFM Parquet parse routing
 * (guarded by PIO_GRIDFM), `dist` the multiconductor value family (guarded
 * by PIO_DIST), and `prob` the problem instance and solution families
 * (guarded by PIO_PROB). Each symbol's own #if states what it needs.
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
/*
 * ABI v6 opaque handle types. Declared here because their Rust definitions
 * come from the shared handle macro cbindgen cannot expand. Every one is an
 * independently owned reference with a retain/release pair; release(NULL) is
 * a no-op; releasing a parent never invalidates a retained child; accessors
 * return spans valid until the handle's last release. Concurrent immutable
 * calls on one handle are allowed; releasing a raw handle concurrently with
 * any call on that same raw handle is caller error.
 */
typedef struct PioBalancedNetwork PioBalancedNetwork;
typedef struct PioError PioError;
typedef struct PioModule PioModule;
typedef struct PioDiagnostics PioDiagnostics;
typedef struct PioDcData PioDcData;
#if defined(PIO_DIST)
typedef struct PioMulticonductorNetwork PioMulticonductorNetwork;
#endif
#if defined(PIO_ARROW)
struct ArrowArray;
struct ArrowSchema;
#endif

/**
 * ABI version of this C interface. Bump on any breaking change to an existing
 * `pio_*` signature or documented behavior, including removing a supported
 * format token from the C API. New additive symbols do not require a bump.
 * A consumer compares [`pio_abi_version`] against the value it was built
 * against (the `PIO_ABI_VERSION` macro in `powerio.h`) and refuses a
 * mismatched library before calling another function.
 *
 * New data uses new symbols or versioned Arrow, `.pio.json`, or format
 * specific JSON schemas. Existing signatures do not change without an ABI
 * version increment.
 *
 * 6 is the current version: the replacement surface. The 0.9 package
 * (`pio_package_*` and the `pkg` feature token) and SCOPF (`pio_scopf_*`,
 * plus the solver row Arrow tables) entry points are withdrawn, the
 * network returning parse family and its caller error buffers are gone,
 * and one module surface remains: `pio_parse_*` produce module handles,
 * typed accessors hand back independently owned network handles, every
 * fallible entry point reports through a structured `PioError`, and every
 * handle type carries a `retain`/`release` pair. A binding built against 5
 * would resolve missing symbols; the handshake refuses first. The 4 to 5
 * bump reshaped every ABI visible JSON document and the diagnostic
 * grammar.
 */
#define PIO_ABI_VERSION 6

/**
 * `PioWriteOptions.missing_gen_cost_mode`: leave a missing cost row absent.
 */
#define PIO_MISSING_GEN_COST_PRESERVE 0

/**
 * `PioWriteOptions.missing_gen_cost_mode`: fail when an in-service generator
 * has no cost row.
 */
#define PIO_MISSING_GEN_COST_REQUIRE 1

/**
 * `PioWriteOptions.missing_gen_cost_mode`: synthesize a MATPOWER polynomial
 * row from the five `fill_*` fields.
 */
#define PIO_MISSING_GEN_COST_FILL 2

#if defined(PIO_ARROW)
/**
 * Table selectors for [`pio_balanced_network_to_arrow`](crate::pio_balanced_network_to_arrow); the C
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
#define PIO_ARROW_TABLE_YBUS 15
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_INCIDENCE 16
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_BPRIME 17
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_BDOUBLEPRIME 18
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_MATRIX_BUS 19
#endif

#if defined(PIO_ARROW)
#define PIO_ARROW_TABLE_MATRIX_BRANCH 20
#endif

/**
 * Solver preparation repairs for [`pio_balanced_network_normalize`], in the extensible options
 * struct convention this header states once. Zero the struct, set
 * `struct_size` to `sizeof(PioNormalizeOptions)`, fill what you need; NULL is
 * every default, which is the plain per unit pass with no repair.
 */
typedef struct {
    /**
     * `sizeof(PioNormalizeOptions)` as the caller's build sees it.
     */
    size_t struct_size;
    /**
     * Nonzero applies the same branch angle difference bound repair as
     * PowerModels: `angmin <= -pi/2`, `angmax >= pi/2`, and zero/zero bounds
     * are replaced by `[-angle_bound_pad, angle_bound_pad]`, and a repair that
     * would invert the interval widens to that same window.
     */
    int32_t clamp_angle_bounds;
    /**
     * Named so the layout carries no implicit padding. Must be zero.
     */
    int32_t reserved;
    /**
     * The half width of the replacement window, in radians, which must be in
     * `(0, pi/2)`. `0` means the default 1.0472, so a zero filled struct is
     * every default; any other out of range value fails the call.
     */
    double angle_bound_pad;
} PioNormalizeOptions;

/**
 * Write-time policies for the four transmission write entry points, in the
 * extensible options struct convention this header states once. Zero the
 * struct, set `struct_size` to `sizeof(PioWriteOptions)`, fill what you need;
 * NULL is every default and is what these entry points did before the
 * parameter existed.
 *
 * The three `pio_dist_*` write entry points take no options struct: the
 * multiconductor writers carry their own per format options and none of them
 * is reachable from this policy set.
 */
typedef struct {
    /**
     * `sizeof(PioWriteOptions)` as the caller's build sees it.
     */
    size_t struct_size;
    /**
     * What to do with an in-service generator that has no active power cost
     * row: `PIO_MISSING_GEN_COST_PRESERVE`, `_REQUIRE`, or `_FILL`.
     */
    int32_t missing_gen_cost_mode;
    /**
     * Named so the layout carries no implicit padding. Must be zero.
     */
    int32_t reserved;
    /**
     * The quadratic coefficient of the row `_FILL` synthesizes.
     */
    double fill_c2;
    /**
     * The linear coefficient of the row `_FILL` synthesizes.
     */
    double fill_c1;
    /**
     * The constant coefficient of the row `_FILL` synthesizes.
     */
    double fill_c0;
    /**
     * The startup cost `_FILL` synthesizes.
     */
    double fill_startup;
    /**
     * The shutdown cost `_FILL` synthesizes.
     */
    double fill_shutdown;
    /**
     * Generator cost patches as CSV text, never a path: a header row of
     * `gen_index,bus,c2,c1,c0` and optional `startup,shutdown`, then one row
     * per patched generator. A write entry point never opens a file the caller
     * names, so read the CSV yourself and hand over its bytes. NULL applies no
     * patches.
     */
    const char *gen_cost_csv;
} PioWriteOptions;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * The ABI version the library was built with (see [`PIO_ABI_VERSION`]). Lets a
 * consumer detect a stale or incompatible library at load time. Infallible.
 */
uint32_t pio_abi_version(void);

/**
 * Report the schema version of each document format in this library, as
 * owned JSON. Free the returned string with [`pio_string_release`]. Infallible.
 *
 * [`PIO_ABI_VERSION`] does not cover these versions. A binding that
 * mirrors one of them must read it from here and refuse a library it does
 * not agree with. A key is `null` when the owning feature is not compiled
 * in. Keys are only added over time. `powerio_version` covers every
 * document powerio authors; `bmopf_schema` is the foreign schema this build
 * speaks, whose version belongs to whoever owns it.
 */
char *pio_schema_versions_json(void);

/**
 * Everything a loader needs to decide what this library can do, as one owned
 * JSON document. Free the returned string with [`pio_string_release`]. Infallible.
 *
 * `curl_version_info` is the shape: one call, one report, and new keys arrive
 * without a new symbol. Keys are only added. A caller with no JSON parser
 * keeps using [`pio_has_feature`] and [`pio_abi_version`], which say the same
 * things one answer at a time.
 *
 * Every error and diagnostic carries a stable dotted code identity such as
 * `EMIT.PSSE.FIELD_DROPPED`. `diagnostic_namespaces` lists the first segments
 * powerio emits, so a consumer that merges reports from several producers can
 * tell powerio's findings from its own. `error_categories` lists the coarse
 * projection each fatal code is published under, for a consumer that wants
 * five buckets rather than the full code set. Both sets are stable; a member
 * may be added.
 *
 * `json_classes` lists the classification families [`pio_classify_str`]
 * answers with, so a binding that routes a bare `.json` reads the closed set
 * from the library rather than hardcoding it.
 */
char *pio_build_info(void);

/**
 * Whether an optional build feature is compiled in: pass `"arrow"`, `"matrix"`,
 * `"gridfm"`, `"dist"`, or `"prob"`. Returns 1 if present, 0 otherwise (and 0
 * for a NULL or unknown name). The optional entry points (`pio_balanced_network_to_arrow`, the
 * matrix Arrow tables, the gridfm parse routing, the multiconductor block,
 * so a consumer that loaded the library at runtime probes for them here
 * instead of resolving symbols blind. Feature names are strings like format
 * names, so a new feature never changes this signature. Infallible.
 */
int32_t pio_has_feature(const char *feature);

/**
 * The crate version string (a semver string), `'static` and NUL-terminated. Do
 * NOT free it. Informational; pair it with [`pio_abi_version`] for the actual
 * compatibility check.
 */
const char *pio_version(void);

/**
 * Classify in-memory JSON case `text` by its top level markers, without
 * parsing the case. Writes one of
 *
 * - `transmission:<format>` (e.g. `transmission:powermodels-json`)
 * - `distribution:<format>` (e.g. `distribution:pmd-json`)
 * - `module` (a `.pio.json` stored module; read it with [`pio_parse_str`](v6::pio_parse_str))
 * - `model-json` (bare balanced model JSON; read it with [`pio_balanced_network_from_json`])
 * - `ambiguous` (strong markers from both domains; pass an explicit format)
 * - `unknown` (no recognized marker, or not a JSON object)
 *
 * into the caller `outbuf` (truncated to fit, always NUL-terminated) and
 * returns the total byte length of the classification string (the
 * size-then-fill idiom of the name queries). Returns 0 for NULL `text`. The
 * markers are the same ones the transmission parser's `.json` sniffing uses,
 * so a binding can route a bare `.json` before choosing a parser.
 *
 * That list is closed: the family (the label up to the first `:`) is always
 * one of those six, the spellings are permanent, and a new family is an
 * addition with a changelog line. `pio_build_info` reports the same set under
 * `json_classes`, so a binding need not hardcode it.
 */
size_t pio_classify_str(const char *text, char *outbuf, size_t outlen);

/**
 * Serialize `net` to its model JSON: the network serialization the stored
 * carries under `model.balanced_network`, without the surrounding document.
 * This is the bindings' data transport and the only route to it: model JSON
 * is powerio's own document rather than a case format, so it has no format
 * token. Returns an owned C string (free with [`pio_string_release`]), `NULL` on
 * error.
 */
char *pio_balanced_network_to_json(const PioBalancedNetwork *net, PioError **error);

/**
 * Parse model JSON produced by [`pio_balanced_network_to_json`] (or lifted from a `.pio.json`
 * document's `model.balanced_network`) back into an owned handle, the inverse
 * of [`pio_balanced_network_to_json`]. A bare `.json` file holding this document classifies as
 * `model-json` through [`pio_classify_str`]. Returns `NULL` on error. Free
 * with [`pio_balanced_network_release`].
 */
PioBalancedNetwork *pio_balanced_network_from_json(const char *text, PioError **error);

/**
 * Mint an independent handle to the same network. NULL stays NULL.
 */
PioBalancedNetwork *pio_balanced_network_retain(const PioBalancedNetwork *net);

/**
 * Release one network handle: the drop half of the retain/release pair, with
 * the ABI v6 lifecycle name. NULL is a no-op.
 */
void pio_balanced_network_release(PioBalancedNetwork *net);

/**
 * Normalize `net` into a NEW network handle: per unit, radians, out of service
 * filtered, source bus ids preserved, bus types canonicalized (see
 * `BalancedNetwork::to_normalized`). A value transform, not a serialization, hence
 * the verb, while the `to_*` family re-encodes unchanged data. The result is
 * independent of `net`; release both when done. Every extractor
 * and serializer works on it unchanged (the handle is per unit, not MW).
 *
 * `opts` turns on the solver preparation repairs; see [`PioNormalizeOptions`].
 * NULL is every default and is the plain pass. The findings already on
 * `net` and any repair findings carry over onto the returned handle.
 *
 * Returns `NULL` on error (no reference bus can be chosen, a non-positive base
 * MVA, or an options struct this build cannot honor) and stores a
 * structured error.
 */
PioBalancedNetwork *pio_balanced_network_normalize(const PioBalancedNetwork *net,
                                                   const PioNormalizeOptions *opts,
                                                   PioError **error);

size_t pio_balanced_network_n_buses(const PioBalancedNetwork *net);

size_t pio_balanced_network_n_branches(const PioBalancedNetwork *net);

size_t pio_balanced_network_n_switches(const PioBalancedNetwork *net);

size_t pio_balanced_network_n_gens(const PioBalancedNetwork *net);

double pio_balanced_network_base_mva(const PioBalancedNetwork *net);

/**
 * Case name. Writes UTF-8 bytes into `out`, up to `cap`, NUL-terminates when
 * possible, and returns the byte length needed excluding the NUL. `NULL` or
 * `cap == 0` is a size query.
 */
size_t pio_balanced_network_name(const PioBalancedNetwork *net, char *out, size_t cap);

/**
 * Source format token used by the JSON snapshot and accepted by every
 * `from` parameter, for example `matpower`, `powermodels-json`, or
 * `normalized`. Uses the same cap/count string convention as
 * [`pio_balanced_network_name`].
 */
size_t pio_balanced_network_source_format(const PioBalancedNetwork *net, char *out, size_t cap);

/**
 * Serialize a compact balanced network summary as JSON for display and scalar
 * queries without serializing [`pio_balanced_network_to_json`]'s full payload.
 *
 * `counts` is the case file's own inventory, so it counts a 3-winding
 * transformer once under `transformers_3w` rather than as the star bus and
 * three branches it lowers to. `topology.n_buses` and `topology.n_branches`
 * are that lowered space, the one [`pio_balanced_network_n_buses`] and [`pio_balanced_network_branches`]
 * report and the one the rest of `topology` is computed over. The two differ
 * only for a case with an in-service 3-winding transformer.
 */
char *pio_balanced_network_summary_json(const PioBalancedNetwork *net,
                                        PioError **error);

/**
 * Dense `[0, n)` index of the single reference (slack) bus, or `-1` if not
 * exactly one. An INDEX into the [`pio_balanced_network_bus_ids`] ordering, not a bus id;
 * `pio_balanced_network_branches` from/to carry ids, so the unit is in the name. A network may
 * carry several references (one per island, or a normalized case that kept
 * the file's multiple `REF` buses); [`pio_balanced_network_ref_bus_indices`] reads them all,
 * and its count (`NULL` out) tells zero from many.
 */
int64_t pio_balanced_network_ref_bus_index(const PioBalancedNetwork *net);

/**
 * Write the dense `[0, n)` indices of the reference (slack) buses, ascending,
 * into `out`, up to `cap` entries, and return the total count: the cap/count
 * convention of [`pio_balanced_network_bus_ids`]. `0` means none; `> 1` means one reference
 * per island or several fixed references in one island (a normalized case
 * always reports `>= 1`).
 */
size_t pio_balanced_network_ref_bus_indices(const PioBalancedNetwork *net,
                                            int64_t *out,
                                            size_t cap);

/**
 * Number of islands: connected components of the in-service topology.
 */
size_t pio_balanced_network_n_islands(const PioBalancedNetwork *net);

/**
 * `1` if the in-service topology is radial (every island a tree), else `0`.
 */
int32_t pio_balanced_network_is_radial(const PioBalancedNetwork *net);

/**
 * Convert the case file at `path` from format `from` (NULL to infer from the
 * path, as [`pio_parse_file`](v6::pio_parse_file)) to format `to`, without keeping a handle.
 * `opts` carries the write-time cost policies (NULL for every default); see
 * [`PioWriteOptions`].
 * Returns the converted text as an owned C string (free with
 * [`pio_string_release`]), `NULL` on error. The findings, read side first, are
 * published through `out_diagnostics_json` as one owned JSON array of
 * diagnostic records (free it with [`pio_string_release`]), NULL when there are
 * none. Pass NULL to discard them. `out_diagnostics_json` is written on every
 * return path and is NULL whenever this returns NULL, so an error return
 * leaves nothing to free.
 */
char *pio_convert_file(const char *path,
                       const char *from,
                       const char *to,
                       const PioWriteOptions *opts,
                       PioDiagnostics **out_diagnostics,
                       PioError **error);

/**
 * Convert in-memory case `text` from format `from` (required; there is no
 * path to infer from) to format `to` without keeping a handle. `opts` carries
 * the write-time cost policies (NULL for every default); see
 * [`PioWriteOptions`]. Returns the converted text as an owned C
 * string (free with [`pio_string_release`]), `NULL` on error. The findings, read
 * side first, are published through `out_diagnostics_json` as one owned JSON
 * array of diagnostic records (free it with [`pio_string_release`]), NULL when
 * there are none. Pass NULL to discard them. `out_diagnostics_json` is written
 * on every return path and is NULL whenever this returns NULL, so an error
 * return leaves nothing to free.
 */
char *pio_convert_str(const char *text,
                      const char *from,
                      const char *to,
                      const PioWriteOptions *opts,
                      PioDiagnostics **out_diagnostics,
                      PioError **error);

/**
 * Free any owned C string returned by this API.
 */
void pio_string_release(char *s);

/**
 * Write the 1-based external bus ids, in dense order, into `out`, up to `cap`
 * entries, and return the total bus count. This ordering DEFINES the dense
 * index space every other per-bus array shares. Call once with `(NULL, 0)` to
 * size, allocate, then call again to fill. Ids are int64 in `1..2^63-1` (a v4
 * invariant): a reader whose source ids are strings assigns dense ids and
 * keeps the source name, and a numeric id past that range is refused at the
 * read boundary rather than passed through.
 */
size_t pio_balanced_network_bus_ids(const PioBalancedNetwork *net, int64_t *out, size_t cap);

/**
 * Write the branch table as parallel arrays, each up to `cap` entries, and
 * return the total branch count. A branch is any two-terminal series element
 * lines and transformers alike (a transformer has `tap != 0`). `from`/`to`
 * are 1-based bus IDS (the [`pio_balanced_network_bus_ids`] id space, not dense indices); map
 * them to dense matrix rows with the [`pio_balanced_network_bus_ids`] ordering. Any output
 * pointer may be NULL to skip that column; all NULL is the count query.
 */
size_t pio_balanced_network_branches(const PioBalancedNetwork *net,
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
size_t pio_balanced_network_branch_charging(const PioBalancedNetwork *net,
                                            double *g_fr,
                                            double *b_fr,
                                            double *g_to,
                                            double *b_to,
                                            size_t cap);

/**
 * Write the switch table as parallel arrays, each up to `cap` entries, and
 * return the total switch count. `from`/`to` are external bus ids.
 */
size_t pio_balanced_network_switches(const PioBalancedNetwork *net,
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
 * [`pio_balanced_network_bus_ids`] id space). Any output pointer may be NULL to skip.
 */
size_t pio_balanced_network_gens(const PioBalancedNetwork *net,
                                 int64_t *bus,
                                 double *pg,
                                 double *pmax,
                                 double *pmin,
                                 uint8_t *in_service,
                                 size_t cap);

/**
 * Write the per-bus demand aggregates (active `pd`, reactive `qd`, summed
 * over each bus's loads, dense [`pio_balanced_network_bus_ids`] order), each up to `cap`
 * entries, and return the total bus count. Either pointer may be NULL.
 */
size_t pio_balanced_network_bus_demand(const PioBalancedNetwork *net,
                                       double *pd,
                                       double *qd,
                                       size_t cap);

/**
 * Write the per-bus shunt aggregates (conductance `gs`, susceptance `bs`,
 * dense [`pio_balanced_network_bus_ids`] order), each up to `cap` entries, and return the
 * total bus count. Either pointer may be NULL.
 */
size_t pio_balanced_network_bus_shunt(const PioBalancedNetwork *net,
                                      double *gs,
                                      double *bs,
                                      size_t cap);

#if defined(PIO_ARROW)
/**
 * Export one network table over the Arrow C Data Interface: the `to_`
 * conversion whose output type is Arrow structs rather than a string, and the
 * bulk table surface of this ABI. Tables 0..5 are raw network tables; tables
 * 6..14 are normalized solver tables with per unit/radian values and dense
 * zero based row ids; tables 15..18 carry COO triplets in that dense index
 * space with dimensions in schema metadata; tables 19 and 20 are the axis maps
 * naming what each row and column of those triplets is (`matrix_bus` carries
 * the bus id, source row, reference flag and island per index, `matrix_branch`
 * the source row and endpoint ids). Tables 21 and 22 carry normalized
 * generator cost: one header row per solver generator, in `solver_gen` order,
 * slicing `[coeff_offset, coeff_offset + coeff_count)` of the flattened
 * coefficient table. `model` 2 reads position `i` of a `coeff_count` long
 * slice as the coefficient of `p^(coeff_count - 1 - i)`; `model` 1 reads even
 * positions as per unit active power at a breakpoint and odd positions as the
 * curve value there; `model` 0 means the generator carries no cost row, and
 * its `coeff_offset` is `-1`. Every table entry in
 * [`pio_arrow_catalog_json`] states which of the three it is under `format`.
 * New columns extend the Arrow schema without changing an existing C
 * signature.
 *
 * `table` is one of the `PIO_ARROW_TABLE_*` selectors. Raw table columns use
 * EXTERNAL bus ids (the `pio_balanced_network_bus_ids` id space), not the gridfm schema. On
 * success (returns `0`),
 * `out_array` and `out_schema` are populated with owned C Data Interface
 * structs: ownership of the Arrow buffers transfers to the caller, both
 * `release` callbacks are non-NULL, and the caller MUST invoke each exactly
 * once when done (skipping one leaks; the structs outlive the handle's release).
 * On error (returns `-1`) a structured error is stored and the
 * out-params are left untouched. Only built with the `arrow` cargo feature.
 */
int32_t pio_balanced_network_to_arrow(const PioBalancedNetwork *net,
                                      int32_t table,
                                      struct ArrowArray *out_array,
                                      struct ArrowSchema *out_schema,
                                      PioError **error);
#endif

#if defined(PIO_ARROW)
/**
 * Return the Arrow table catalog as owned compact JSON.
 *
 * The catalog is feature based rather than handle based: it describes what
 * this library build can export, not what a particular network contains. Top
 * level fields are `powerio_version`, `producer`, and `tables`. Each table
 * entry includes `id`, `name`, `format`, `feature_requirements`, `available`,
 * `row_axis`, `col_axis`, `units`, and `columns`. Each column entry includes
 * `name`, `type`, and `nullable`. Through v4 both levels carried a
 * `schema_version`; one release version now covers every document powerio
 * authors, so the top level names it and the per-table copy is gone.
 *
 * Free the returned string with [`pio_string_release`]. On error this returns
 * NULL and stores a structured error. Only built with the `arrow` cargo
 * feature.
 */
char *pio_arrow_catalog_json(PioError **error);
#endif

/**
 * Normalize a tolerant geographic sidecar (headerless buscoords CSV, aliased
 * CSV/JSON records, GeoJSON Point/LineString) to the canonical GeoJSON form.
 * `name_hint` (a file name, nullable) picks CSV against JSON when given;
 * otherwise the content is sniffed. Free the returned string with
 * [`pio_string_release`]. Returns `NULL` on input that carries no usable
 * coordinates and stores a structured error.
 *
 * The tolerant reader's notes, one per record it read past, are published
 * through `out_diagnostics_json` as one owned JSON array of diagnostic
 * records (free it with [`pio_string_release`]), or NULL when the reader used
 * every record. Pass NULL to discard them. `out_diagnostics_json` is written
 * on every return path and is NULL whenever this returns NULL, so an error
 * return leaves nothing to free.
 */
char *pio_geo_parse(const char *text,
                    const char *name_hint,
                    PioDiagnostics **out_diagnostics,
                    PioError **error);

/**
 * Extract a network's coordinates as the canonical GeoJSON layer: one point
 * per located bus, one route per routed branch. Free the returned string
 * with `pio_string_release`. Returns `NULL` (with a message) when the network
 * carries no coordinates.
 */
char *pio_balanced_network_geo_extract(const PioBalancedNetwork *net, PioError **error);

/**
 * Apply a geographic sidecar (any form [`pio_geo_parse`] accepts) onto a NEW
 * network handle; the input handle is unchanged and both are freed with
 * [`pio_balanced_network_release`]. `name_hint` (a file name, nullable) picks CSV against
 * JSON as in [`pio_geo_parse`]. Matched bus points land in `Bus.location`,
 * matched branch routes in `Branch.route`. The returned handle drops the
 * retained source text, so a same-format write re-serializes the placed case
 * instead of echoing the original. The reader's notes and an apply summary
 * (`geo apply: N bus point(s), ...`) are appended to the handle's warnings
 * on the returned handle's findings. Returns `NULL` on error.
 */
PioBalancedNetwork *pio_balanced_network_geo_apply(const PioBalancedNetwork *net,
                                                   const char *layer,
                                                   const char *name_hint,
                                                   PioError **error);

#if defined(PIO_DIST)
/**
 * One-line apply summary lifted into the returned handle's warnings.
 * Extract a multiconductor network's coordinates as the canonical GeoJSON
 * layer, keyed by the string bus and line names. Free the returned string
 * with `pio_string_release`. Returns `NULL` (with a message) when the network
 * carries no coordinates.
 */
char *pio_multiconductor_network_geo_extract(const PioMulticonductorNetwork *net, PioError **error);
#endif

#if defined(PIO_DIST)
/**
 * Apply a geographic sidecar (any form [`pio_geo_parse`] accepts) onto a NEW
 * distribution network handle; the input handle is unchanged and both are
 * released with [`pio_multiconductor_network_release`]. `name_hint` (a file name, nullable)
 * picks CSV against JSON as in [`pio_geo_parse`]. The returned handle drops
 * the retained source text, so a same-format write re-serializes the placed
 * case. The reader's notes and an apply summary are appended to the handle's
 * the handle's findings. Returns `NULL` on error.
 */
PioMulticonductorNetwork *pio_multiconductor_network_geo_apply(const PioMulticonductorNetwork *net,
                                                               const char *layer,
                                                               const char *name_hint,
                                                               PioError **error);
#endif

#if defined(PIO_DIST)
/**
 * Mint an independent handle to the same multiconductor network. NULL stays
 * NULL.
 */
PioMulticonductorNetwork *pio_multiconductor_network_retain(const PioMulticonductorNetwork *net);
#endif

#if defined(PIO_DIST)
/**
 * Release one multiconductor network handle: identical to
 * the drop half of the multiconductor retain/release pair.
 */
void pio_multiconductor_network_release(PioMulticonductorNetwork *net);
#endif

#if defined(PIO_DIST)
/**
 * Serialize a compact summary of a distribution handle as JSON. This lets
 * bindings answer display and scalar queries without forcing
 * [`pio_multiconductor_network_to_json`]'s full model payload.
 */
char *pio_multiconductor_network_summary_json(const PioMulticonductorNetwork *net,
                                              PioError **error);
#endif

#if defined(PIO_DIST)
/**
 * Serialize `net` to its model JSON: the network serialization the stored
 * carries under `model.multiconductor_network`, without the surrounding
 * document. This is the bindings' data transport, not a case format: the
 * converter, CLI, and format inference do not know it; a distribution case
 * other tools read is BMOPF JSON, written through
 * [`pio_module_write_str`](v6::pio_module_write_str).
 * Returns an owned C string (free with [`pio_string_release`]), `NULL` on error.
 */
char *pio_multiconductor_network_to_json(const PioMulticonductorNetwork *net, PioError **error);
#endif

#if defined(PIO_DIST)
/**
 * Serialize the collapsed bus and terminal graph projection for `net` as JSON.
 * The returned string is owned by the library; free it with
 * [`pio_string_release`].
 */
char *pio_multiconductor_network_graph_json(const PioMulticonductorNetwork *net, PioError **error);
#endif

#if defined(PIO_DIST)
/**
 * Parse model JSON produced by [`pio_multiconductor_network_to_json`] (or lifted from a
 * `.pio.json` document's `model.multiconductor_network`) back into an owned
 * handle: the inverse of [`pio_multiconductor_network_to_json`]. The rebuilt handle retains
 * no source text, so a same format write is a fresh serialization. The handle
 * retains the model JSON `warnings`. Returns `NULL` on error. Free with
 * [`pio_multiconductor_network_release`].
 */
PioMulticonductorNetwork *pio_multiconductor_network_from_json(const char *text, PioError **error);
#endif

/**
 * The failure's stable diagnostic code, valid until the handle's release.
 */
const char *pio_error_code(const PioError *error);

/**
 * The rendered failure message, valid until the handle's release.
 */
const char *pio_error_message(const PioError *error);

/**
 * The structured diagnostics as a JSON array, valid until the handle's
 * release.
 */
const char *pio_error_diagnostics_json(const PioError *error);

/**
 * Mint an independent handle to the same error. NULL stays NULL.
 */
PioError *pio_error_retain(const PioError *error);

/**
 * Release one error handle. NULL is a no-op.
 */
void pio_error_release(PioError *error);

/**
 * Read stored `.pio.json` text: version 1, or a released 0.9 document
 * upgraded one way. Returns a new module handle, or NULL with `error` set.
 */
PioModule *pio_module_read_json(const char *text, PioError **error);

/**
 * Parse a case file into a module of whichever family claims it. `format`
 * may be NULL for detection by name and content.
 */
PioModule *pio_parse_file(const char *path, const char *format, PioError **error);

/**
 * Parse in-memory case text into a module. `name` labels the buffer for
 * diagnostics and format detection; NULL uses `<memory>`.
 */
PioModule *pio_parse_str(const char *name, const char *text, const char *format, PioError **error);

/**
 * Parse in-memory case bytes into a module: the only in-memory way to read
 * a binary format. Text formats must be UTF-8. `name` labels the buffer for
 * diagnostics and format detection; NULL uses `<memory>`.
 */
PioModule *pio_parse_bytes(const char *name,
                           const uint8_t *data,
                           size_t len,
                           const char *format,
                           PioError **error);

/**
 * The module's balanced network value as an owned network handle, provenance
 * included. Any other value kind is refused with the kind named.
 */
PioBalancedNetwork *pio_module_balanced_network(const PioModule *module, PioError **error);

#if defined(PIO_DIST)
/**
 * The module's multiconductor network value as an owned distribution
 * handle, provenance included. Any other value kind is refused.
 */
PioMulticonductorNetwork *pio_module_multiconductor_network(const PioModule *module,
                                                            PioError **error);
#endif

/**
 * A module over one balanced network handle's value, sharing that handle's
 * records: the wrap for semantic writing of a network built in memory (for
 * example through `pio_balanced_network_from_json`).
 */
PioModule *pio_module_of_balanced_network(const PioBalancedNetwork *network, PioError **error);

#if defined(PIO_DIST)
/**
 * A module over one multiconductor network handle's value, sharing that
 * handle's records: the wrap for semantic writing.
 */
PioModule *pio_module_of_multiconductor_network(const PioMulticonductorNetwork *network,
                                                PioError **error);
#endif

/**
 * Write the module as the named target format and return the text: the one
 * write operation over the C surface. Writing an unchanged parsed module
 * back to its source format returns the retained bytes exactly; any other
 * target serializes the typed value. The writer's findings cross through
 * `out_diagnostics` as a structured handle (NULL discards them). Free the
 * text with `pio_string_release`.
 */
char *pio_module_write_str(const PioModule *module,
                           const char *format,
                           PioDiagnostics **out_diagnostics,
                           PioError **error);

/**
 * Write the module as the named target format into `path`: the filesystem
 * form of [`pio_module_write_str`], covering the directory targets (PyPSA
 * CSV) a single text cannot state. The destination must not already exist.
 */
int32_t pio_module_write_file(const PioModule *module,
                              const char *format,
                              const char *path,
                              PioDiagnostics **out_diagnostics,
                              PioError **error);

/**
 * The stored version 1 document. Free with `pio_string_release`.
 */
char *pio_module_write_json(const PioModule *module, PioError **error);

/**
 * The module's diagnostics as a JSON array (stable code, severity, message,
 * optional identity and target per entry). Free with `pio_string_release`.
 */
char *pio_module_diagnostics_json(const PioModule *module, PioError **error);

/**
 * The value's permanent kind identifier, valid until the handle's release.
 */
const char *pio_module_kind(const PioModule *module);

/**
 * Value inspection and supported operation discovery, as JSON. Free with
 * `pio_string_release`.
 */
char *pio_module_inspect_json(const PioModule *module, PioError **error);

/**
 * The typed time or scenario inventory as JSON. Free with `pio_string_release`.
 */
char *pio_module_state_inventory_json(const PioModule *module, PioError **error);

/**
 * Export one selected time point or scenario as an independent static
 * module. `time_position >= 0` selects by position (scenario must be NULL);
 * `scenario` non NULL selects by ID (time_position must be negative).
 */
PioModule *pio_module_export_state(const PioModule *module,
                                   int64_t time_position,
                                   const char *scenario,
                                   PioError **error);

/**
 * Readiness of the multiconductor value for the balanced lowering, as JSON.
 * Free with `pio_string_release`.
 */
char *pio_module_lowering_readiness_json(const PioModule *module,
                                         double base_mva,
                                         PioError **error);

/**
 * Explicitly lower the multiconductor value to a balanced module. Records
 * and source ownership carry over; the pass appends its findings and one
 * Transform history entry.
 */
PioModule *pio_module_lower_to_balanced(const PioModule *module, double base_mva, PioError **error);

/**
 * Mint an independent handle to the same module. NULL stays NULL.
 */
PioModule *pio_module_retain(const PioModule *module);

/**
 * Release one module handle. NULL is a no-op.
 */
void pio_module_release(PioModule *module);

/**
 * The module's diagnostics as a structured list handle. This is the binding
 * inspection path; [`pio_module_diagnostics_json`] stays as the explicit
 * serialization helper.
 */
PioDiagnostics *pio_module_diagnostics(const PioModule *module, PioError **error);

/**
 * The failure's diagnostics as a structured list handle. NULL error yields
 * an empty list.
 */
PioDiagnostics *pio_error_diagnostics(const PioError *error);

/**
 * The number of rows in the list. NULL yields 0.
 */
size_t pio_diagnostics_len(const PioDiagnostics *diagnostics);

/**
 * Mint an independent handle to the same list. NULL stays NULL.
 */
PioDiagnostics *pio_diagnostics_retain(const PioDiagnostics *diagnostics);

/**
 * Release one list handle. NULL is a no-op.
 */
void pio_diagnostics_release(PioDiagnostics *diagnostics);

/**
 * The row's stable diagnostic code. NULL handle or an out of range index yields NULL.
 */
const char *pio_diagnostic_code(const PioDiagnostics *diagnostics, size_t index);

/**
 * The row's severity name: `error`, `warning`, `remark`, or `note`. NULL handle or an out of range index yields NULL.
 */
const char *pio_diagnostic_severity(const PioDiagnostics *diagnostics,
                                    size_t index);

/**
 * The row's rendered message. Explanatory text, not a stable identifier. NULL handle or an out of range index yields NULL.
 */
const char *pio_diagnostic_message(const PioDiagnostics *diagnostics,
                                   size_t index);

/**
 * The row's identifier when one was assigned, else NULL. NULL handle or an out of range index yields NULL.
 */
const char *pio_diagnostic_id(const PioDiagnostics *diagnostics,
                              size_t index);

/**
 * The row's target locator when one exists, else NULL. NULL handle or an out of range index yields NULL.
 */
const char *pio_diagnostic_target(const PioDiagnostics *diagnostics,
                                  size_t index);

/**
 * The row's suggested action when one exists, else NULL. NULL handle or an out of range index yields NULL.
 */
const char *pio_diagnostic_suggested_action(const PioDiagnostics *diagnostics,
                                            size_t index);

/**
 * The row's details as one JSON object, or NULL when it has none. NULL handle or an out of range index yields NULL.
 */
const char *pio_diagnostic_details_json(const PioDiagnostics *diagnostics,
                                        size_t index);

/**
 * The number of source spans on one row. NULL or out of range yields 0.
 */
size_t pio_diagnostic_n_spans(const PioDiagnostics *diagnostics, size_t index);

/**
 * One source span: writes the byte range and returns the span's source
 * identifier. NULL handle or an out of range index yields NULL and leaves
 * the out parameters unwritten.
 */
const char *pio_diagnostic_span(const PioDiagnostics *diagnostics,
                                size_t index,
                                size_t span,
                                uint64_t *byte_start,
                                uint64_t *byte_end);

/**
 * The number of related diagnostic identifiers on one row.
 */
size_t pio_diagnostic_n_related(const PioDiagnostics *diagnostics, size_t index);

/**
 * One related diagnostic identifier. NULL or out of range yields NULL.
 */
const char *pio_diagnostic_related(const PioDiagnostics *diagnostics, size_t index, size_t related);

/**
 * Build the DC branch data of a module's balanced network value under the
 * named branch susceptance formula (`series_susceptance`,
 * `tap_adjusted_reactance`, or `reactance_only`). The result is an
 * independently owned handle: releasing the module never invalidates it.
 */
PioDcData *pio_dc_data_build(const PioModule *module, const char *formula, PioError **error);

/**
 * Included incidence row count (`m`).
 */
size_t pio_dc_data_n_rows(const PioDcData *data);

/**
 * Incidence column count (`n`, the bus count).
 */
size_t pio_dc_data_n_buses(const PioDcData *data);

/**
 * From bus column per included row (`A[e, from] = +1`), length `n_rows`.
 */
const int64_t *pio_dc_data_from_indices(const PioDcData *data);

/**
 * To bus column per included row (`A[e, to] = -1`), length `n_rows`.
 */
const int64_t *pio_dc_data_to_indices(const PioDcData *data);

/**
 * Branch susceptance per included row, PowerModels sign, length `n_rows`.
 */
const double *pio_dc_data_susceptance(const PioDcData *data);

/**
 * Phase shift angle per included row, radians, length `n_rows`. `0` for an
 * unshifted branch or a formula that excludes shifts.
 */
const double *pio_dc_data_shift(const PioDcData *data);

/**
 * Phase shift bus injection `p_shift = A' * (b .* shift)` (the MATPOWER
 * `makeBdc` sign), length `n_buses`.
 */
const double *pio_dc_data_shift_injection(const PioDcData *data);

/**
 * Stable module element ID per included row, length `n_rows`. Both the
 * table and the strings stay valid until the handle's release.
 */
const char *const *pio_dc_data_row_ids(const PioDcData *data);

/**
 * Stable bus element ID per incidence column, length `n_buses`.
 */
const char *const *pio_dc_data_bus_ids(const PioDcData *data);

/**
 * Count of branches the selected formula cannot represent.
 */
size_t pio_dc_data_n_omitted(const PioDcData *data);

/**
 * Stable element IDs of the omitted branches, length `n_omitted`.
 */
const char *const *pio_dc_data_omitted_ids(const PioDcData *data);

/**
 * Diagnostic reason per omitted branch, length `n_omitted`.
 */
const char *const *pio_dc_data_omitted_reasons(const PioDcData *data);

/**
 * The selected branch susceptance formula's stable name.
 */
const char *pio_dc_data_formula(const PioDcData *data);

/**
 * Fill `out` with the complete affine branch flow
 * `p_branch = -b .* (va_from - va_to) + b .* shift`: given bus voltage
 * angles `va` (radians, length `n_buses`), writes
 * `-b[e] * (va[from] - va[to]) + b[e] * shift[e]` per included row into
 * `out` (length `n_rows`), so `A' * p_branch` equals the bus injection
 * including `shift_injection`. Returns false on a NULL argument or a length
 * mismatch. No temporary vector is allocated.
 */
bool pio_dc_data_fill_branch_flow(const PioDcData *data,
                                  const double *va,
                                  size_t va_len,
                                  double *out,
                                  size_t out_len);

/**
 * Mint an independent handle to the same DC data. NULL stays NULL.
 */
PioDcData *pio_dc_data_retain(const PioDcData *data);

/**
 * Release one DC data handle. NULL is a no-op.
 */
void pio_dc_data_release(PioDcData *data);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* POWERIO_H */
