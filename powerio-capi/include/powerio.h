/* powerio C ABI — parse any power system case format, query it, convert
 * losslessly, and extract the numeric tables for matrix assembly.
 *
 * Memory: strings returned by pio_write_matpower / pio_convert are owned by the
 * library; free them with pio_string_free. Case handles from pio_parse are freed
 * with pio_case_free. Array extractors fill caller-allocated buffers whose
 * length is the matching pio_n_* count; pass NULL to skip an output.
 *
 * Message buffers: errbuf/warnbuf may be NULL (or length 0) to discard the
 * message. A message longer than the buffer is truncated to fit and is always
 * NUL-terminated. PIO_ERRBUF_MIN is a comfortable size for any error string.
 *
 * Every entry point catches Rust panics at the boundary and returns the failure
 * default (NULL, 0, -1, 0.0, or no-op) rather than unwinding across the ABI.
 *
 * Optional: build with `--features arrow` to get pio_export_arrow, a zero-copy
 * raw network export over the Arrow C Data Interface (guarded by PIO_ARROW).
 *
 * This header is checked in; regenerate from the Rust source with
 *   cbindgen --config cbindgen.toml --crate powerio-capi --output include/powerio.h
 */
#ifndef POWERIO_H
#define POWERIO_H

#include <stddef.h>
#include <stdint.h>

#define PIO_ERRBUF_MIN 256

#ifdef __cplusplus
extern "C" {
#endif

/* ABI version of this interface. pio_abi_version() returns the value the library
 * was built with; compare it against PIO_ABI_VERSION (the value you compiled
 * against) and refuse a mismatched library. Bump on any breaking change to an
 * existing pio_* signature or the JSON transport schema (additive symbols don't
 * bump it). pio_version() is the informational crate version string ("0.1.0"):
 * 'static and NUL-terminated, do NOT free it. */
#define PIO_ABI_VERSION 1
uint32_t pio_abi_version(void);
const char *pio_version(void);

typedef struct PioCase PioCase;

/* Parse `path`; format from the file extension, or forced by `from`
 * ("matpower","powermodels","psse","powerworld") when non-NULL. Returns NULL on
 * error and writes the message into errbuf (a char[errlen]). */
PioCase *pio_parse(const char *path, const char *from, char *errbuf, size_t errlen);
/* Parse in-memory case `text` of `format` ("matpower"/"m", "powermodels"/"pm",
 * "egret", "psse"/"raw", "powerworld"/"aux"); `format` is required (no path to
 * infer from). Returns NULL on error and writes the message into errbuf. */
PioCase *pio_parse_str(const char *text, const char *format, char *errbuf, size_t errlen);
void pio_case_free(PioCase *c);

size_t pio_n_buses(const PioCase *c);
size_t pio_n_branches(const PioCase *c);
size_t pio_n_gens(const PioCase *c);
double pio_base_mva(const PioCase *c);
/* Dense [0,n) index of the single reference bus, or -1 if not exactly one. */
ptrdiff_t pio_reference_bus(const PioCase *c);
size_t pio_n_components(const PioCase *c);
int pio_is_radial(const PioCase *c);

/* Serialize back to MATPOWER .m (byte-exact echo when parsed from MATPOWER).
 * Owned string; free with pio_string_free. */
char *pio_write_matpower(const PioCase *c);

/* Convert `path` to format `to` (optionally forcing source `from`). Returns the
 * converted text (owned; free with pio_string_free), NULL on error. Fidelity
 * warnings are written '\n'-joined into warnbuf; errors into errbuf. */
char *pio_convert(const char *path, const char *to, const char *from,
                  char *warnbuf, size_t warnlen, char *errbuf, size_t errlen);
void pio_string_free(char *s);

/* Structured JSON transport — what the Julia bridge consumes. pio_to_json
 * serializes the whole network (tables + extras, but not the retained source
 * text) to an owned string (free with pio_string_free), NULL on error.
 * pio_from_json rebuilds a handle from that JSON (free with pio_case_free); the
 * handle has no source, so pio_write_matpower reformats rather than echoing. */
char *pio_to_json(const PioCase *c, char *errbuf, size_t errlen);
PioCase *pio_from_json(const char *json, char *errbuf, size_t errlen);

/* Normalize `c` into a NEW handle: per unit, radians, out-of-service filtered,
 * densely reindexed, bus types canonicalized. Independent of the input handle
 * (free both with pio_case_free); every extractor and pio_to_json works on it.
 * Returns NULL on error (e.g. no reference bus), message into errbuf. */
PioCase *pio_to_normalized(const PioCase *c, char *errbuf, size_t errlen);

/* Numeric table extractors. Each output buffer has the matching pio_n_* length;
 * pass NULL to skip it. `from`/`to`/`bus` are 1-based bus ids in the same id
 * space as pio_bus_ids (NOT dense [0,n) indices); pio_bus_ids gives the id at
 * each dense index, so invert it to map an endpoint id to a matrix row. */
void pio_bus_ids(const PioCase *c, int64_t *out);
void pio_branches(const PioCase *c, int64_t *from, int64_t *to, double *r,
                  double *x, double *b, double *tap, double *shift,
                  uint8_t *in_service);
/* One row per generator (not per bus): `bus` repeats when two generators share
 * a bus, unlike the per-bus pio_nodal_* tables below. */
void pio_gens(const PioCase *c, int64_t *bus, double *pg, double *pmax,
              double *pmin, uint8_t *in_service);
/* Demand / shunt summed per bus, dense order, length pio_n_buses. */
void pio_nodal_demand(const PioCase *c, double *pd, double *qd);
void pio_nodal_shunt(const PioCase *c, double *gs, double *bs);

#ifdef PIO_ARROW
/* Zero-copy raw network export over the Arrow C Data Interface (powerio-capi
 * built with `--features arrow`). ArrowArray / ArrowSchema are the standard C
 * Data Interface structs; include the Arrow ABI header (arrow/c/abi.h, or the
 * vendored examples/arrow_c_data_interface.h) for their definitions — this
 * header only forward-declares them. */
struct ArrowArray;
struct ArrowSchema;

#define PIO_ARROW_TABLE_BUS    0
#define PIO_ARROW_TABLE_BRANCH 1
#define PIO_ARROW_TABLE_GEN    2
#define PIO_ARROW_TABLE_LOAD   3
#define PIO_ARROW_TABLE_SHUNT  4

/* Export raw network table `table` (one of PIO_ARROW_TABLE_*) as a single Arrow
 * array + schema, zero-copy. Columns are the parsed network fields with EXTERNAL
 * bus ids (the pio_bus_ids id space), not the gridfm schema. On success (0)
 * *out_array and *out_schema are populated and owned by the caller — release
 * each via its `release` callback. On error (-1) the message is written into
 * errbuf and the out-params are left untouched. */
int pio_export_arrow(const PioCase *c, int table,
                     struct ArrowArray *out_array, struct ArrowSchema *out_schema,
                     char *errbuf, size_t errlen);
#endif /* PIO_ARROW */

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* POWERIO_H */
