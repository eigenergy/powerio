/* powerio C ABI: parse any power system case format, query it, convert
 * it, and extract the numeric tables for matrix assembly.
 *
 * Memory: strings returned by pio_to_matpower / pio_to_format /
 * pio_convert_file / pio_to_json are owned by the library; free them with
 * pio_string_free. Network handles from pio_parse_file / pio_parse_str /
 * pio_from_json / pio_to_normalized are freed with pio_network_free. Array
 * extractors fill caller-allocated buffers whose length is the matching pio_n_*
 * count; pass NULL to skip an output.
 *
 * Message buffers: errbuf/warnbuf may be NULL (or length 0) to discard the
 * message. A message longer than the buffer is truncated to fit and is always
 * NUL-terminated. PIO_ERRBUF_MIN is a comfortable size for any error string.
 *
 * Every entry point catches Rust panics at the boundary and returns the documented
 * failure value (NULL, 0, -1, 0.0, or unchanged output) rather than unwinding
 * across the ABI.
 *
 * Optional: build with `--features arrow` to get pio_export_arrow, a raw
 * network export over the Arrow C Data Interface (guarded by PIO_ARROW).
 * Build with `--features dist` to get the pio_dist_* entry points (guarded by
 * PIO_DIST): multiconductor distribution cases (OpenDSS, PMD ENGINEERING JSON,
 * BMOPF JSON) behind their own PioDistNetwork handle, freed with
 * pio_dist_network_free; their string outputs are freed with pio_string_free.
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
 * `pio_*` signature or to the JSON transport schema (new additive symbols don't
 * require a bump). A consumer compares [`pio_abi_version`] against the value it
 * was built against (the `PIO_ABI_VERSION` macro in `powerio.h`) and refuses a
 * mismatched library instead of calling in blind.
 */
#define PIO_ABI_VERSION 3

/**
 * A comfortable error-buffer size: pass a `char[PIO_ERRBUF_MIN]` to any
 * `errbuf`/`warnbuf` parameter and a message always fits without truncation.
 */
#define PIO_ERRBUF_MIN 256

#if defined(PIO_ARROW)
/**
 * Table selectors for [`pio_export_arrow`](crate::pio_export_arrow); the C
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

#if defined(PIO_DIST)
/**
 * Opaque multiconductor distribution network handle: a parsed OpenDSS, PMD
 * ENGINEERING JSON, or BMOPF JSON case in wire coordinates. Distinct from
 * [`PioNetwork`] (the positive sequence transmission model); none of the
 * `pio_n_*`/extractor functions accept it. Only built with the `dist` cargo
 * feature.
 */
typedef struct PioDistNetwork PioDistNetwork;
#endif

/**
 * Opaque parsed network handle. Carries the parsed [`Network`] plus the
 * [`IndexCore`] derived from it once at parse time, so every indexed query
 * reuses the same bus-id map and nodal aggregates instead of rebuilding them.
 */
typedef struct PioNetwork PioNetwork;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * The ABI version the library was built with (see [`PIO_ABI_VERSION`]). Lets a
 * consumer detect a stale or incompatible library at load time. Infallible.
 */
uint32_t pio_abi_version(void);

/**
 * The crate version string (e.g. `"0.0.1"`), `'static` and NUL-terminated. Do
 * NOT free it. Informational; pair it with [`pio_abi_version`] for the actual
 * compatibility check.
 */
const char *pio_version(void);

/**
 * Parse `path` (format from extension, or `from` if non-NULL) into a case
 * handle. Returns `NULL` on error and writes the message into `errbuf`.
 */
PioNetwork *pio_parse_file(const char *path, const char *from, char *errbuf, size_t errlen);

/**
 * Parse in-memory case `text` of the named `format` into a network handle. Unlike
 * [`pio_parse_file`] there is no path to infer from, so `format` is required: one of
 * `matpower`/`m`, `powermodels`/`pm`, `egret`, `psse`/`raw`, `powerworld`/`aux`
 * (see `TargetFormat::from_str`). Returns `NULL` on error and writes the
 * message into `errbuf`. Free the handle with [`pio_network_free`].
 */
PioNetwork *pio_parse_str(const char *text, const char *format, char *errbuf, size_t errlen);

/**
 * Free a network handle from [`pio_parse_file`], [`pio_parse_str`],
 * [`pio_to_normalized`], or [`pio_from_json`].
 */
void pio_network_free(PioNetwork *net);

/**
 * Normalize `net` into a NEW per-unit network handle: per unit, radians,
 * out-of-service filtered, densely reindexed, bus types canonicalized (see
 * `Network::to_normalized`). The result is independent of `net`; free both
 * with [`pio_network_free`]. Every extractor and [`pio_to_json`] works on it
 * unchanged (the handle is per unit, not MW). Returns `NULL` on error (no
 * reference bus can be chosen, or a non-positive base MVA) and writes the
 * message into `errbuf`.
 */
PioNetwork *pio_to_normalized(const PioNetwork *net, char *errbuf, size_t errlen);

size_t pio_n_buses(const PioNetwork *net);

size_t pio_n_branches(const PioNetwork *net);

size_t pio_n_gens(const PioNetwork *net);

double pio_base_mva(const PioNetwork *net);

/**
 * Dense `[0, n)` index of the single reference bus, or `-1` if not exactly one
 * (also `-1` if the index is too large for `isize`). A network may carry
 * several references (one per island, or a normalized case that kept the file's
 * multiple `REF` buses); use [`pio_n_reference_buses`] to tell zero from many,
 * and [`pio_reference_buses`] to read them all.
 */
ptrdiff_t pio_reference_bus(const PioNetwork *net);

/**
 * Number of reference (slack) buses. `0` means none; `> 1` means one reference
 * per island or several fixed reference buses in one island. A normalized case
 * always reports `>= 1`.
 */
size_t pio_n_reference_buses(const PioNetwork *net);

/**
 * Fill `out` (length [`pio_n_reference_buses`]) with the dense `[0, n)` indices
 * of the reference buses, ascending.
 */
void pio_reference_buses(const PioNetwork *net, int64_t *out);

size_t pio_n_components(const PioNetwork *net);

/**
 * `1` if the in-service topology is a forest, else `0`.
 */
int32_t pio_is_radial(const PioNetwork *net);

/**
 * Serialize `net` to MATPOWER `.m` text (byte-exact echo when parsed from
 * MATPOWER). Returns an owned C string; free with [`pio_string_free`]. Returns
 * `NULL` on error and writes the message into `errbuf`.
 */
char *pio_to_matpower(const PioNetwork *net, char *errbuf, size_t errlen);

/**
 * Serialize `net` to format `to`.
 *
 * Returns the converted text as an owned C string (free with
 * [`pio_string_free`]), `NULL` on error. Fidelity warnings, if any, are written
 * `\n`-joined into `warnbuf`.
 */
char *pio_to_format(const PioNetwork *net,
                    const char *to,
                    char *warnbuf,
                    size_t warnlen,
                    char *errbuf,
                    size_t errlen);

/**
 * Convert `path` to format `to` (optionally forcing the source via `from`).
 * Returns the converted text as an owned C string (free with
 * [`pio_string_free`]), `NULL` on error. Fidelity warnings, if any, are written
 * `\n`-joined into `warnbuf`.
 */
char *pio_convert_file(const char *path,
                       const char *to,
                       const char *from,
                       char *warnbuf,
                       size_t warnlen,
                       char *errbuf,
                       size_t errlen);

/**
 * Free a string returned by [`pio_to_matpower`], [`pio_to_format`],
 * [`pio_convert_file`], [`pio_to_json`], or any `pio_dist_*` converter.
 */
void pio_string_free(char *s);

/**
 * Serialize the case to JSON: the structured-table transport every Julia
 * bridge consumes. Carries the whole [`Network`] (buses, loads, shunts,
 * branches, generators, storage, HVDC, extras) but not the retained source
 * text, so it is structured data, not the byte-exact echo. Returns an owned C
 * string (free with [`pio_string_free`]), `NULL` on error (message into
 * `errbuf`).
 */
char *pio_to_json(const PioNetwork *net, char *errbuf, size_t errlen);

/**
 * Rebuild a network handle from JSON produced by [`pio_to_json`]. Returns a new
 * handle (free with [`pio_network_free`]), or `NULL` on error (message into
 * `errbuf`). The handle has no retained source, so [`pio_to_matpower`]
 * reformats it rather than echoing a byte-exact original.
 */
PioNetwork *pio_from_json(const char *json, char *errbuf, size_t errlen);

/**
 * Fill `out` (length `pio_n_buses`) with the 1-based bus ids in dense order.
 */
void pio_bus_ids(const PioNetwork *net, int64_t *out);

/**
 * Fill the branch tables (each length `pio_n_branches`). `from`/`to` are the
 * 1-based bus ids (the same id space as [`pio_bus_ids`], not dense indices);
 * map them to dense matrix rows with the [`pio_bus_ids`] ordering. Any pointer
 * may be `NULL` to skip.
 */
void pio_branches(const PioNetwork *net,
                  int64_t *from,
                  int64_t *to,
                  double *r,
                  double *x,
                  double *b,
                  double *tap,
                  double *shift,
                  uint8_t *in_service);

/**
 * Fill the generator tables (each length `pio_n_gens`; `bus` is the 1-based bus
 * id, the same id space as [`pio_bus_ids`]). Any pointer may be `NULL` to skip.
 */
void pio_gens(const PioNetwork *net,
              int64_t *bus,
              double *pg,
              double *pmax,
              double *pmin,
              uint8_t *in_service);

/**
 * Fill nodal aggregates (each length `pio_n_buses`, dense order): active and
 * reactive demand summed per bus. Any pointer may be `NULL`.
 */
void pio_nodal_demand(const PioNetwork *net, double *pd, double *qd);

/**
 * Fill nodal shunt aggregates (each length `pio_n_buses`, dense order).
 */
void pio_nodal_shunt(const PioNetwork *net, double *gs, double *bs);

#if defined(PIO_ARROW)
/**
 * Export one raw network table over the Arrow C Data Interface.
 *
 * `table` is one of the `PIO_ARROW_TABLE_*` selectors (bus/branch/gen/load/
 * shunt); the columns are the parsed network fields with EXTERNAL bus ids (the
 * `pio_bus_ids` id space), not the gridfm schema. On success (returns `0`),
 * `out_array` and `out_schema` are populated with owned C Data Interface
 * structs: ownership of the Arrow buffers transfers to the caller, both
 * `release` callbacks are non-NULL, and the caller MUST invoke each exactly
 * once when done (skipping one leaks; the structs outlive `pio_network_free`).
 * On error (returns `-1`) the message is written into `errbuf` and the
 * out-params are left untouched. Only built with the `arrow` cargo feature.
 */
int32_t pio_export_arrow(const PioNetwork *net,
                         int32_t table,
                         struct ArrowArray *out_array,
                         struct ArrowSchema *out_schema,
                         char *errbuf,
                         size_t errlen);
#endif

#if defined(PIO_DIST)
/**
 * Parse a distribution case file into a [`PioDistNetwork`] handle. The format
 * comes from `from` if non-NULL (`dss`, `pmd`, or `bmopf`), else from the
 * file itself: `.dss` is OpenDSS, and `.json` holding the ENGINEERING
 * `data_model` key is PMD JSON, otherwise BMOPF JSON. Returns `NULL` on error
 * and writes the message into `errbuf`. Free the handle with
 * [`pio_dist_network_free`].
 */
PioDistNetwork *pio_dist_parse_file(const char *path,
                                    const char *from,
                                    char *errbuf,
                                    size_t errlen);
#endif

#if defined(PIO_DIST)
/**
 * Parse in-memory distribution case `text` of the named `format` (`dss`,
 * `pmd`, or `bmopf`; required, since there is no path to infer from). An
 * OpenDSS `Redirect`/`Compile` in `text` resolves against the current working
 * directory. Returns `NULL` on error and writes the message into `errbuf`.
 * Free the handle with [`pio_dist_network_free`].
 */
PioDistNetwork *pio_dist_parse_str(const char *text,
                                   const char *format,
                                   char *errbuf,
                                   size_t errlen);
#endif

#if defined(PIO_DIST)
/**
 * Free a distribution network handle from [`pio_dist_parse_file`] or
 * [`pio_dist_parse_str`].
 */
void pio_dist_network_free(PioDistNetwork *net);
#endif

#if defined(PIO_DIST)
/**
 * Parse warnings retained on the handle: everything the reader could not
 * represent or had to assume (the loud half of the fidelity contract).
 * Writes them `\n`-joined into `warnbuf` (NULL/0 to skip) and returns the
 * warning count, or `-1` if `net` is NULL.
 */
ptrdiff_t pio_dist_warnings(const PioDistNetwork *net, char *warnbuf, size_t warnlen);
#endif

#if defined(PIO_DIST)
/**
 * Serialize `net` to distribution format `to` (`dss`, `pmd`, or `bmopf`).
 * Writing back to the format the handle was parsed from echoes the source
 * text byte for byte; a cross format write reports every fidelity loss in
 * `warnbuf` (`\n`-joined). Returns the text as an owned C string (free with
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
 * Convert distribution case `path` to format `to` (optionally forcing the
 * source via `from`; see [`pio_dist_parse_file`] for the inference rules).
 * Returns the converted text as an owned C string (free with
 * [`pio_string_free`]), `NULL` on error. The warnings written `\n`-joined
 * into `warnbuf` carry both the parse warnings and the writer's fidelity
 * losses (there is no handle to query them from).
 */
char *pio_dist_convert_file(const char *path,
                            const char *to,
                            const char *from,
                            char *warnbuf,
                            size_t warnlen,
                            char *errbuf,
                            size_t errlen);
#endif

#if defined(PIO_DIST)
/**
 * Convert in-memory distribution case `text` from format `from` to format
 * `to` (both required; `dss`, `pmd`, or `bmopf`). Returns the converted text
 * as an owned C string (free with [`pio_string_free`]), `NULL` on error. The
 * warnings written `\n`-joined into `warnbuf` carry both the parse warnings
 * and the writer's fidelity losses (there is no handle to query them from).
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
