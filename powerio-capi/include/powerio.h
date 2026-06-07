/* powerio C ABI — parse any power-system case format, query it, convert
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

typedef struct PioCase PioCase;

/* Parse `path`; format from the file extension, or forced by `from`
 * ("matpower","powermodels","psse","powerworld") when non-NULL. Returns NULL on
 * error and writes the message into errbuf (a char[errlen]). */
PioCase *pio_parse(const char *path, const char *from, char *errbuf, size_t errlen);
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

/* Numeric table extractors. Each output buffer has the matching pio_n_* length;
 * pass NULL to skip it. `from`/`to`/`bus` are dense bus indices ([0,n), or -1 if
 * a referenced bus is unknown). */
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

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* POWERIO_H */
