/* C ABI smoke test: drive powerio-capi from C the way a real consumer would.
 *
 * Built and run in CI against the checked-in static library, so a break in the
 * ABI (or the header drifting from the Rust source) fails the build rather than
 * silently shipping. Not a unit test â€” it asserts the calls work end to end and
 * returns non-zero on any failure.
 *
 *   cc -I powerio-capi/include powerio-capi/examples/smoke.c \
 *      target/release/libpowerio_capi.a -o smoke   (+ -lpthread -ldl -lm on Linux)
 *   ./smoke tests/data/case9.m
 */
#include "powerio.h"

#ifdef PIO_ARROW
#include "arrow_c_data_interface.h" /* full ArrowArray/ArrowSchema definitions */
#endif

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

    /* ABI handshake: a consumer refuses a library whose ABI version differs from
     * the header it compiled against. pio_version() is a static, non-owned string. */
    CHECK(pio_abi_version() == PIO_ABI_VERSION, "ABI version mismatch");
    printf("powerio %s (ABI %u)\n", pio_version(), pio_abi_version());

    char err[PIO_ERRBUF_MIN];
    PioNetwork *c = pio_parse_file(argv[1], NULL, err, sizeof err);
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

    /* Pull branch endpoints (1-based bus ids, same space as pio_bus_ids) and
     * reactances, as a solver would. */
    int64_t *from = malloc(m * sizeof *from);
    double *x = malloc(m * sizeof *x);
    CHECK(from && x, "out of memory");
    pio_branches(c, from, NULL, NULL, x, NULL, NULL, NULL, NULL);
    for (size_t k = 0; k < m; k++) {
        CHECK(from[k] >= 1, "branch from-id should be a valid 1-based bus id");
        CHECK(x[k] != 0.0, "zero reactance");
    }
    free(from);
    free(x);

    /* Byte-exact MATPOWER echo comes back as an owned string. */
    char *echo = pio_to_matpower(c, err, sizeof err);
    CHECK(echo != NULL && strlen(echo) > 0, err);
    pio_string_free(echo);

    /* Cross-format convert reaches the converter and returns owned text. */
    char *raw = pio_convert_file(argv[1], "psse", NULL, NULL, 0, err, sizeof err);
    CHECK(raw != NULL, err);
    pio_string_free(raw);

    /* JSON transport: serialize, rebuild, and confirm the counts survive. */
    char *json = pio_to_json(c, err, sizeof err);
    CHECK(json != NULL, err);
    PioNetwork *c2 = pio_from_json(json, err, sizeof err);
    CHECK(c2 != NULL, err);
    CHECK(pio_n_buses(c2) == nb && pio_n_branches(c2) == m && pio_n_gens(c2) == ng,
          "JSON round-trip changed the table sizes");
    pio_string_free(json);
    pio_network_free(c2);

    /* In-memory parse: read the bytes ourselves and parse them with an explicit
     * format, then confirm it agrees with the path-based parse. */
    {
        FILE *fp = fopen(argv[1], "rb");
        CHECK(fp != NULL, "could not reopen case file");
        fseek(fp, 0, SEEK_END);
        long sz = ftell(fp);
        fseek(fp, 0, SEEK_SET);
        char *buf = malloc((size_t)sz + 1);
        CHECK(buf != NULL, "out of memory");
        size_t rd = fread(buf, 1, (size_t)sz, fp);
        buf[rd] = '\0';
        fclose(fp);

        PioNetwork *cs = pio_parse_str(buf, "matpower", err, sizeof err);
        CHECK(cs != NULL, err);
        CHECK(pio_n_buses(cs) == nb && pio_n_branches(cs) == m && pio_n_gens(cs) == ng,
              "pio_parse_str disagrees with pio_parse_file on table sizes");
        free(buf);

        /* Normalize into a NEW handle: per unit, radians, filtered, reindexed.
         * It has no more buses than the raw case, has at least one reference bus
         * (several if the file marked several), and still serializes through the
         * JSON transport. Use pio_n_reference_buses, not pio_reference_bus >= 0:
         * the latter returns -1 for a multi-slack case, which is valid here. */
        PioNetwork *cn = pio_to_normalized(cs, err, sizeof err);
        CHECK(cn != NULL, err);
        CHECK(pio_n_buses(cn) <= nb && pio_n_buses(cn) > 0, "normalized bus count out of range");
        CHECK(pio_n_reference_buses(cn) >= 1, "normalized case lost its reference bus");
        char *njson = pio_to_json(cn, err, sizeof err);
        CHECK(njson != NULL, err);
        pio_string_free(njson);
        pio_network_free(cn);
        pio_network_free(cs);
        printf("parse_str + to_normalized OK\n");
    }

#ifdef PIO_ARROW
    /* Zero-copy Arrow C Data Interface export: pull the bus table, check the row
     * count, then release the producer-owned buffers. */
    {
        struct ArrowArray arr;
        struct ArrowSchema sch;
        memset(&arr, 0, sizeof arr);
        memset(&sch, 0, sizeof sch);
        int rc = pio_export_arrow(c, PIO_ARROW_TABLE_BUS, &arr, &sch, err, sizeof err);
        CHECK(rc == 0, err);
        CHECK(arr.length == (int64_t)nb, "arrow bus table row count mismatch");
        CHECK(arr.release != NULL && sch.release != NULL, "missing arrow release callbacks");
        arr.release(&arr);
        sch.release(&sch);
        printf("arrow export OK: %zu bus rows\n", nb);
    }
#endif

    /* NULL handle is the documented safe default. */
    CHECK(pio_n_buses(NULL) == 0, "NULL handle did not return 0");
    CHECK(pio_reference_bus(NULL) == -1, "NULL handle did not return -1");

    pio_network_free(c);
    printf("C ABI smoke test OK\n");
    return 0;
}
