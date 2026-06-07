/* C ABI smoke test: drive powerio-capi from C the way a real consumer would.
 *
 * Built and run in CI against the checked-in static library, so a break in the
 * ABI (or the header drifting from the Rust source) fails the build rather than
 * silently shipping. Not a unit test — it asserts the calls work end to end and
 * returns non-zero on any failure.
 *
 *   cc -I powerio-capi/include powerio-capi/examples/smoke.c \
 *      target/release/libpowerio_capi.a -o smoke   (+ -lpthread -ldl -lm on Linux)
 *   ./smoke tests/data/case9.m
 */
#include "powerio.h"

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

    char err[PIO_ERRBUF_MIN];
    PioCase *c = pio_parse(argv[1], NULL, err, sizeof err);
    CHECK(c != NULL, err);

    size_t nb = pio_n_buses(c);
    size_t m = pio_n_branches(c);
    size_t ng = pio_n_gens(c);
    double base = pio_base_mva(c);
    printf("parsed %s: %zu buses, %zu branches, %zu gens, baseMVA %g\n", argv[1],
           nb, m, ng, base);
    CHECK(nb > 0 && m > 0, "empty case");
    CHECK(pio_n_components(c) >= 1, "bad component count");
    CHECK(pio_reference_bus(c) >= 0, "no single reference bus");

    /* Pull branch endpoints (dense indices) and reactances, as a solver would. */
    int64_t *from = malloc(m * sizeof *from);
    double *x = malloc(m * sizeof *x);
    CHECK(from && x, "out of memory");
    pio_branches(c, from, NULL, NULL, x, NULL, NULL, NULL, NULL);
    for (size_t k = 0; k < m; k++) {
        CHECK(from[k] >= 0 && (size_t)from[k] < nb, "branch from-index out of range");
        CHECK(x[k] != 0.0, "zero reactance");
    }
    free(from);
    free(x);

    /* Byte-exact MATPOWER echo comes back as an owned string. */
    char *echo = pio_write_matpower(c);
    CHECK(echo != NULL && strlen(echo) > 0, "write_matpower returned empty");
    pio_string_free(echo);

    /* Cross-format convert reaches the converter and returns owned text. */
    char *raw = pio_convert(argv[1], "psse", NULL, NULL, 0, err, sizeof err);
    CHECK(raw != NULL, err);
    pio_string_free(raw);

    /* JSON transport: serialize, rebuild, and confirm the counts survive. */
    char *json = pio_to_json(c, err, sizeof err);
    CHECK(json != NULL, err);
    PioCase *c2 = pio_from_json(json, err, sizeof err);
    CHECK(c2 != NULL, err);
    CHECK(pio_n_buses(c2) == nb && pio_n_branches(c2) == m && pio_n_gens(c2) == ng,
          "JSON round-trip changed the table sizes");
    pio_string_free(json);
    pio_case_free(c2);

    /* NULL handle is the documented safe default. */
    CHECK(pio_n_buses(NULL) == 0, "NULL handle did not return 0");
    CHECK(pio_reference_bus(NULL) == -1, "NULL handle did not return -1");

    pio_case_free(c);
    printf("C ABI smoke test OK\n");
    return 0;
}
