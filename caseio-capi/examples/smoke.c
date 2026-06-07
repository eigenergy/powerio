/* C ABI smoke test: drive caseio-capi from C the way a real consumer would.
 *
 * Built and run in CI against the checked-in static library, so a break in the
 * ABI (or the header drifting from the Rust source) fails the build rather than
 * silently shipping. Not a unit test — it asserts the calls work end to end and
 * returns non-zero on any failure.
 *
 *   cc -I caseio-capi/include caseio-capi/examples/smoke.c \
 *      target/release/libcaseio_capi.a -o smoke   (+ -lpthread -ldl -lm on Linux)
 *   ./smoke tests/data/case9.m
 */
#include "caseio.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(cond, msg)                                                       \
    do {                                                                       \
        if (!(cond)) {                                                         \
            fprintf(stderr, "smoke: %s\n", (msg));                             \
            return 1;                                                          \
        }                                                                      \
    } while (0)

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <case.m>\n", argv[0]);
        return 2;
    }

    char err[CIO_ERRBUF_MIN];
    CioCase *c = cio_parse(argv[1], NULL, err, sizeof err);
    CHECK(c != NULL, err);

    size_t nb = cio_n_buses(c);
    size_t m = cio_n_branches(c);
    size_t ng = cio_n_gens(c);
    double base = cio_base_mva(c);
    printf("parsed %s: %zu buses, %zu branches, %zu gens, baseMVA %g\n", argv[1],
           nb, m, ng, base);
    CHECK(nb > 0 && m > 0, "empty case");
    CHECK(cio_n_components(c) >= 1, "bad component count");
    CHECK(cio_reference_bus(c) >= 0, "no single reference bus");

    /* Pull branch endpoints (dense indices) and reactances, as a solver would. */
    int64_t *from = malloc(m * sizeof *from);
    double *x = malloc(m * sizeof *x);
    CHECK(from && x, "out of memory");
    cio_branches(c, from, NULL, NULL, x, NULL, NULL, NULL, NULL);
    for (size_t k = 0; k < m; k++) {
        CHECK(from[k] >= 0 && (size_t)from[k] < nb, "branch from-index out of range");
        CHECK(x[k] != 0.0, "zero reactance");
    }
    free(from);
    free(x);

    /* Byte-exact MATPOWER echo comes back as an owned string. */
    char *echo = cio_write_matpower(c);
    CHECK(echo != NULL && strlen(echo) > 0, "write_matpower returned empty");
    cio_string_free(echo);

    /* Cross-format convert reaches the converter and returns owned text. */
    char *raw = cio_convert(argv[1], "psse", NULL, NULL, 0, err, sizeof err);
    CHECK(raw != NULL, err);
    cio_string_free(raw);

    /* NULL handle is the documented safe default. */
    CHECK(cio_n_buses(NULL) == 0, "NULL handle did not return 0");
    CHECK(cio_reference_bus(NULL) == -1, "NULL handle did not return -1");

    cio_case_free(c);
    printf("C ABI smoke test OK\n");
    return 0;
}
