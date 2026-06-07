/* caseio C ABI — parse any power-system case format, query it, convert
 * losslessly, and extract the numeric tables for matrix assembly.
 *
 * Memory: strings returned by cio_write_matpower / cio_convert are owned by the
 * library; free them with cio_string_free. Case handles from cio_parse are freed
 * with cio_case_free. Array extractors fill caller-allocated buffers whose
 * length is the matching cio_n_* count; pass NULL to skip an output.
 *
 * Message buffers: errbuf/warnbuf may be NULL (or length 0) to discard the
 * message. A message longer than the buffer is truncated to fit and is always
 * NUL-terminated. CIO_ERRBUF_MIN is a comfortable size for any error string.
 *
 * Every entry point catches Rust panics at the boundary and returns the failure
 * default (NULL, 0, -1, 0.0, or no-op) rather than unwinding across the ABI.
 *
 * This header is checked in; regenerate from the Rust source with
 *   cbindgen --config cbindgen.toml --crate caseio-capi --output include/caseio.h
 */
#ifndef CASEIO_H
#define CASEIO_H

#include <stddef.h>
#include <stdint.h>

#define CIO_ERRBUF_MIN 256

#ifdef __cplusplus
extern "C" {
#endif

typedef struct CioCase CioCase;

/* Parse `path`; format from the file extension, or forced by `from`
 * ("matpower","powermodels","psse","powerworld") when non-NULL. Returns NULL on
 * error and writes the message into errbuf (a char[errlen]). */
CioCase *cio_parse(const char *path, const char *from, char *errbuf, size_t errlen);
void cio_case_free(CioCase *c);

size_t cio_n_buses(const CioCase *c);
size_t cio_n_branches(const CioCase *c);
size_t cio_n_gens(const CioCase *c);
double cio_base_mva(const CioCase *c);
/* Dense [0,n) index of the single reference bus, or -1 if not exactly one. */
ptrdiff_t cio_reference_bus(const CioCase *c);
size_t cio_n_components(const CioCase *c);
int cio_is_radial(const CioCase *c);

/* Serialize back to MATPOWER .m (byte-exact echo when parsed from MATPOWER).
 * Owned string; free with cio_string_free. */
char *cio_write_matpower(const CioCase *c);

/* Convert `path` to format `to` (optionally forcing source `from`). Returns the
 * converted text (owned; free with cio_string_free), NULL on error. Fidelity
 * warnings are written '\n'-joined into warnbuf; errors into errbuf. */
char *cio_convert(const char *path, const char *to, const char *from,
                  char *warnbuf, size_t warnlen, char *errbuf, size_t errlen);
void cio_string_free(char *s);

/* Numeric table extractors. Each output buffer has the matching cio_n_* length;
 * pass NULL to skip it. `from`/`to`/`bus` are dense bus indices ([0,n), or -1 if
 * a referenced bus is unknown). */
void cio_bus_ids(const CioCase *c, int64_t *out);
void cio_branches(const CioCase *c, int64_t *from, int64_t *to, double *r,
                  double *x, double *b, double *tap, double *shift,
                  uint8_t *in_service);
/* One row per generator (not per bus): `bus` repeats when two generators share
 * a bus, unlike the per-bus cio_nodal_* tables below. */
void cio_gens(const CioCase *c, int64_t *bus, double *pg, double *pmax,
              double *pmin, uint8_t *in_service);
/* Demand / shunt summed per bus, dense order, length cio_n_buses. */
void cio_nodal_demand(const CioCase *c, double *pd, double *qd);
void cio_nodal_shunt(const CioCase *c, double *gs, double *bs);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* CASEIO_H */
