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

#ifdef PIO_ARROW
#include "arrow_c_data_interface.h" /* full ArrowArray/ArrowSchema definitions */
#endif

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if PIO_ABI_VERSION != 4
#error "PIO_ABI_VERSION changed without updating the C ABI smoke test"
#endif

#if PIO_ERRBUF_MIN != 256
#error "PIO_ERRBUF_MIN changed without updating the C ABI smoke test"
#endif

#ifdef PIO_DIST
#if PIO_DIST_ABI_VERSION != 1
#error "PIO_DIST_ABI_VERSION changed without updating the C ABI smoke test"
#endif
#endif

#ifdef PIO_ARROW
#if PIO_ARROW_TABLE_BUS != 0 || PIO_ARROW_TABLE_BRANCH != 1 ||                \
    PIO_ARROW_TABLE_GEN != 2 || PIO_ARROW_TABLE_LOAD != 3 ||                  \
    PIO_ARROW_TABLE_SHUNT != 4 || PIO_ARROW_TABLE_YBUS != 15 ||               \
    PIO_ARROW_TABLE_INCIDENCE != 16 || PIO_ARROW_TABLE_BPRIME != 17 ||         \
    PIO_ARROW_TABLE_BDOUBLEPRIME != 18 ||                                      \
    PIO_ARROW_TABLE_MATRIX_BUS != 19 || PIO_ARROW_TABLE_MATRIX_BRANCH != 20
#error "PIO_ARROW_TABLE_* ids changed without updating the C ABI smoke test"
#endif
#endif

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
#ifdef PIO_GRIDFM
    if (argc < 3) {
        fprintf(stderr, "usage: %s <case.m> <gridfm-raw-dir>\n", argv[0]);
        return 2;
    }
#endif

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
    CHECK(pio_n_islands(c) >= 1, "bad island count");
    CHECK(pio_ref_bus_index(c) >= 0, "no single reference bus");
    /* The MATPOWER reader is total: no warnings attached to the handle. */
    CHECK(pio_warnings(c, NULL, 0) == 0, "unexpected parse warnings");

    /* Pull branch endpoints (1-based bus ids, same space as pio_bus_ids) and
     * reactances, as a solver would. The all-NULL call is the count query; the
     * fill returns the same total, so a short buffer is detectable. */
    CHECK(pio_branches(c, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0) == m,
          "count query disagrees with pio_n_branches");
    int64_t *from = malloc(m * sizeof *from);
    double *x = malloc(m * sizeof *x);
    CHECK(from && x, "out of memory");
    CHECK(pio_branches(c, from, NULL, NULL, x, NULL, NULL, NULL, NULL, m) == m,
          "branch fill did not return the total");
    for (size_t k = 0; k < m; k++) {
        CHECK(from[k] >= 1, "branch from-id should be a valid 1-based bus id");
        CHECK(x[k] != 0.0, "zero reactance");
    }
    free(from);
    free(x);

    /* Byte-exact MATPOWER echo: matpower is a format string, not a symbol. */
    char warn[PIO_ERRBUF_MIN];
    warn[0] = '\0';
    char *echo = pio_to_format(c, "matpower", warn, sizeof warn, err, sizeof err);
    CHECK(echo != NULL && strlen(echo) > 0, err);
    pio_string_free(echo);

    /* Cross-format convert reaches the converter and returns owned text. */
    char *raw = pio_convert_file(argv[1], NULL, "psse", NULL, 0, err, sizeof err);
    CHECK(raw != NULL, err);
    pio_string_free(raw);

    /* The canonical snapshot: serialize to powerio-json, parse it back, and
     * confirm the counts survive. Lossless, validated on read. */
    char *json = pio_to_format(c, "powerio-json", NULL, 0, err, sizeof err);
    CHECK(json != NULL, err);
    PioNetwork *c2 = pio_parse_str(json, "powerio-json", err, sizeof err);
    CHECK(c2 != NULL, err);
    CHECK(pio_n_buses(c2) == nb && pio_n_branches(c2) == m && pio_n_gens(c2) == ng,
          "snapshot round-trip changed the table sizes");
    pio_string_free(json);
    pio_network_free(c2);

#ifdef PIO_PKG
    /* Compiler package surface: wrap the live balanced handle, validate the
     * package, serialize it, and parse it back. */
    {
        CHECK(pio_has_feature("pkg") == 1, "pio_has_feature(pkg) should be 1");
        PioPackage *pkg = pio_package_from_balanced_network(c, 0, err, sizeof err);
        CHECK(pkg != NULL, err);
        CHECK(pio_package_validate(pkg, err, sizeof err) == 0, err);

        char *validation = pio_package_validation_json(pkg, err, sizeof err);
        CHECK(validation != NULL, err);
        CHECK(strstr(validation, "\"status\":\"ok\"") != NULL,
              "package validation JSON did not report ok");
        pio_string_free(validation);

        char *diagnostics = pio_package_diagnostics_json(pkg, err, sizeof err);
        CHECK(diagnostics != NULL, err);
        CHECK(strcmp(diagnostics, "[]") == 0, "unexpected package diagnostics");
        pio_string_free(diagnostics);

        char *pkg_json = pio_package_to_json(pkg, err, sizeof err);
        CHECK(pkg_json != NULL, err);
        CHECK(strstr(pkg_json, "\"model_kind\":\"balanced\"") != NULL,
              "package JSON lost the balanced model kind");

        PioPackage *pkg2 = pio_package_parse_str(pkg_json, err, sizeof err);
        CHECK(pkg2 != NULL, err);
        pio_package_free(pkg2);
        pio_string_free(pkg_json);
        pio_package_free(pkg);
        printf("package surface OK\n");
    }
#endif

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

        /* In-memory convert: parse + serialize fused, no filesystem. */
        char *pm = pio_convert_str(buf, "matpower", "powermodels-json",
                                   NULL, 0, err, sizeof err);
        CHECK(pm != NULL, err);
        pio_string_free(pm);

        char *old_order = pio_convert_str(buf, "powermodels-json", "matpower",
                                          NULL, 0, err, sizeof err);
        if (old_order != NULL) {
            pio_string_free(old_order);
            CHECK(0, "pio_convert_str accepted target/source argument order");
        }
        CHECK(strlen(err) > 0, "pio_convert_str old-order error was empty");
        free(buf);

        /* Normalize into a NEW handle: per unit, radians, filtered, reindexed.
         * It has no more buses than the raw case, has at least one reference bus
         * (several if the file marked several), and still snapshots. Count the
         * references with the NULL-out query, not pio_ref_bus_index >= 0: the
         * latter returns -1 for a multi-slack case, which is valid here. */
        PioNetwork *cn = pio_normalize(cs, err, sizeof err);
        CHECK(cn != NULL, err);
        CHECK(pio_n_buses(cn) <= nb && pio_n_buses(cn) > 0, "normalized bus count out of range");
        CHECK(pio_ref_bus_indices(cn, NULL, 0) >= 1, "normalized case lost its reference bus");
        char *njson = pio_to_format(cn, "powerio-json", NULL, 0, err, sizeof err);
        CHECK(njson != NULL, err);
        pio_string_free(njson);
        pio_network_free(cn);
        pio_network_free(cs);
        printf("parse_str + convert_str + normalize OK\n");
    }

    /* Directory writer: 0 on success, -1 on error (message in errbuf). The
     * format is a string here too; pypsa-csv is the one directory format. */
    {
        warn[0] = '\0';
        char outdir[512];
        snprintf(outdir, sizeof outdir, "%s-pypsa-smoke", argv[1]);
        int rc = pio_write_dir(c, "pypsa-csv", outdir, warn, sizeof warn, err, sizeof err);
        CHECK(rc == 0, err);
        char buses[600];
        snprintf(buses, sizeof buses, "%s/buses.csv", outdir);
        FILE *bf = fopen(buses, "rb");
        CHECK(bf != NULL, "PyPSA folder missing buses.csv");
        fclose(bf);
        rc = pio_write_dir(NULL, "pypsa-csv", outdir, NULL, 0, err, sizeof err);
        CHECK(rc == -1, "NULL network handle should fail the directory write");
        printf("pypsa csv directory write OK: %s\n", outdir);
    }

#ifdef PIO_GRIDFM
    /* Dataset reader surface: gridfm is a directory format and returns the same
     * PioNetwork handle family as parse_file/parse_str. */
    {
        const char *gridfm_dir = argv[2];
        CHECK(pio_has_feature("gridfm") == 1, "pio_has_feature(gridfm) should be 1");

        ptrdiff_t count = pio_scenario_ids(gridfm_dir, "gridfm", NULL, 0, err, sizeof err);
        CHECK(count > 0, err);
        int64_t ids[4] = {-1, -1, -1, -1};
        CHECK(pio_scenario_ids(gridfm_dir, "gridfm", ids, 4, err, sizeof err) == count,
              "scenario-id fill did not return the total");

        PioNetwork *g = pio_read_dir(gridfm_dir, "gridfm", ids[0], err, sizeof err);
        CHECK(g != NULL, err);
        CHECK(pio_n_buses(g) > 0, "gridfm read returned an empty network");
        CHECK(pio_warnings(g, NULL, 0) > 0, "gridfm read should report fidelity warnings");
        pio_network_free(g);
        printf("gridfm surface OK\n");
    }
#endif

#ifdef PIO_DIST
    /* Distribution surface: parse an in-memory OpenDSS case, read its parse
     * warnings, convert it to BMOPF JSON, and check the byte-exact dss echo. */
    {
        const char *dss =
            "clear\n"
            "new circuit.smoke basekv=12.47 bus1=src\n"
            "new line.l1 bus1=src bus2=b2 length=100 units=m\n"
            "new load.d1 bus1=b2 kv=12.47 kw=50\n"
            "solve\n";
        PioDistNetwork *d = pio_dist_parse_str(dss, "dss", err, sizeof err);
        CHECK(d != NULL, err);

        char warn[1024];
        /* Warnings use the size-then-fill idiom of pio_warnings: the return is
         * the byte length needed (0 here, this case is clean). */
        pio_dist_warnings(d, warn, sizeof warn);

        char *bmopf = pio_dist_to_format(d, "bmopf", warn, sizeof warn, err, sizeof err);
        CHECK(bmopf != NULL, err);
        CHECK(strstr(bmopf, "\"bus\"") != NULL, "BMOPF output lost the bus table");
        pio_string_free(bmopf);

        /* Same-format write echoes the retained source byte for byte. */
        char *echo2 = pio_dist_to_format(d, "dss", warn, sizeof warn, err, sizeof err);
        CHECK(echo2 != NULL, err);
        CHECK(strcmp(echo2, dss) == 0, "dss echo is not byte exact");
        pio_string_free(echo2);
        pio_dist_network_free(d);

        /* One-shot string conversion into PMD ENGINEERING JSON; parameter
         * order is input, source, target, like pio_dist_convert_file. */
        char *pmd = pio_dist_convert_str(dss, "dss", "pmd", warn, sizeof warn, err, sizeof err);
        CHECK(pmd != NULL, err);
        CHECK(strstr(pmd, "\"data_model\": \"ENGINEERING\"") != NULL,
              "PMD output lost the data_model marker");
        pio_string_free(pmd);

        char *old_dist_order = pio_dist_convert_str(dss, "pmd", "dss",
                                                    warn, sizeof warn, err, sizeof err);
        if (old_dist_order != NULL) {
            pio_string_free(old_dist_order);
            CHECK(0, "pio_dist_convert_str accepted target/source argument order");
        }
        CHECK(strlen(err) > 0, "pio_dist_convert_str old-order error was empty");

        /* NULL handle is the documented safe default: a 0-length count. */
        CHECK(pio_dist_warnings(NULL, warn, sizeof warn) == 0,
              "NULL dist handle did not return 0");
        /* The optional dist surface reports itself through the feature query. */
        CHECK(pio_has_feature("dist") == 1, "pio_has_feature(dist) should be 1");
        CHECK(pio_dist_abi_version() == PIO_DIST_ABI_VERSION,
              "dist ABI version mismatch");
        char *dist_caps = pio_dist_capabilities_json();
        CHECK(dist_caps != NULL, "pio_dist_capabilities_json returned NULL");
        CHECK(strstr(dist_caps, "\"dist\":true") != NULL,
              "dist capabilities did not report dist=true");
        CHECK(strstr(dist_caps, "\"schema_version\":\"1.0.0\"") != NULL,
              "dist capabilities schema_version mismatch");
        CHECK(strstr(dist_caps, "\"bmopf_fixed_taps\":true") != NULL,
              "fixed tap capability missing");
        CHECK(strstr(dist_caps, "\"bmopf_center_tap_leakage\":true") != NULL,
              "center tap capability missing");
        CHECK(strstr(dist_caps, "\"bmopf_delta_wye_leakage\":true") != NULL,
              "delta wye leakage capability missing");
        CHECK(strstr(dist_caps, "\"bmopf_delta_roll\":true") != NULL,
              "delta roll capability missing");
        CHECK(strstr(dist_caps, "\"bmopf_voltage_source_merge\":true") != NULL,
              "voltage source merge capability missing");
        CHECK(strstr(dist_caps, "\"bmopf_transformer_diagnostics\":true") != NULL,
              "transformer diagnostics capability missing");
        pio_string_free(dist_caps);
        printf("dist surface OK\n");
    }
#endif

#ifdef PIO_ARROW
    /* Zero-copy Arrow C Data Interface export: pull the bus table, check the row
     * count, then release the producer-owned buffers. */
    {
        struct ArrowArray arr;
        struct ArrowSchema sch;
        memset(&arr, 0, sizeof arr);
        memset(&sch, 0, sizeof sch);
        int rc = pio_to_arrow(c, PIO_ARROW_TABLE_BUS, &arr, &sch, err, sizeof err);
        CHECK(rc == 0, err);
        CHECK(arr.length == (int64_t)nb, "arrow bus table row count mismatch");
        CHECK(arr.release != NULL && sch.release != NULL, "missing arrow release callbacks");
        arr.release(&arr);
        sch.release(&sch);
        printf("arrow export OK: %zu bus rows\n", nb);
    }
#ifdef PIO_MATRIX
    {
        CHECK(pio_matrix_available() == 1, "matrix Arrow tables should be available");
        struct ArrowArray arr;
        struct ArrowSchema sch;
        memset(&arr, 0, sizeof arr);
        memset(&sch, 0, sizeof sch);
        int rc = pio_to_arrow(c, PIO_ARROW_TABLE_BPRIME, &arr, &sch, err, sizeof err);
        CHECK(rc == 0, err);
        CHECK(arr.length > 0, "Bprime table should not be empty for case9");
        CHECK(arr.release != NULL && sch.release != NULL,
              "missing matrix arrow release callbacks");
        arr.release(&arr);
        sch.release(&sch);
        memset(&arr, 0, sizeof arr);
        memset(&sch, 0, sizeof sch);
        rc = pio_to_arrow(c, PIO_ARROW_TABLE_MATRIX_BUS, &arr, &sch, err, sizeof err);
        CHECK(rc == 0, err);
        CHECK(arr.length == (int64_t)nb, "matrix_bus axis row count mismatch");
        CHECK(arr.release != NULL && sch.release != NULL,
              "missing matrix_bus arrow release callbacks");
        arr.release(&arr);
        sch.release(&sch);

        char *catalog = pio_arrow_catalog_json(err, sizeof err);
        CHECK(catalog != NULL, err);
        CHECK(strstr(catalog, "\"name\":\"matrix_bus\"") != NULL,
              "Arrow catalog missing matrix_bus");
        CHECK(strstr(catalog, "\"name\":\"matrix_branch\"") != NULL,
              "Arrow catalog missing matrix_branch");
        pio_string_free(catalog);
        printf("matrix arrow export OK\n");
    }
#else
    {
        CHECK(pio_matrix_available() == 0,
              "matrix Arrow tables should be unavailable without PIO_MATRIX");
        struct ArrowArray arr;
        struct ArrowSchema sch;
        memset(&arr, 0, sizeof arr);
        memset(&sch, 0, sizeof sch);
        int rc = pio_to_arrow(c, PIO_ARROW_TABLE_BPRIME, &arr, &sch, err, sizeof err);
        CHECK(rc == -1, "matrix Arrow table should fail without matrix support");
        CHECK(strlen(err) > 0, "matrix Arrow failure should explain the missing feature");
    }
#endif
#endif

    /* NULL handle is the documented safe default. */
    CHECK(pio_n_buses(NULL) == 0, "NULL handle did not return 0");
    CHECK(pio_ref_bus_index(NULL) == -1, "NULL handle did not return -1");

    pio_network_free(c);
    printf("C ABI smoke test OK\n");
    return 0;
}
