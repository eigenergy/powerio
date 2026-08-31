/* C ABI smoke test: drive powerio-capi from C the way a real consumer would.
 *
 * Built and run in CI against the checked-in library, so a break in the ABI
 * (or the header drifting from the Rust source) fails the build rather than
 * silently shipping. Not a unit test — it asserts the calls work end to end
 * and returns non-zero on any failure.
 *
 *   cc -I powerio-capi/include powerio-capi/examples/smoke.c \
 *      target/release/libpowerio_capi.a -o smoke   (+ -lpthread -ldl -lm on Linux)
 *   ./smoke tests/data/case9.m [gridfm_dataset_dir]
 */
#include "powerio.h"

#ifdef PIO_ARROW
#include "arrow_c_data_interface.h" /* full ArrowArray/ArrowSchema definitions */
#endif

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if PIO_ABI_VERSION != 6
#error "PIO_ABI_VERSION changed without updating the C ABI smoke test"
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

static int failures = 0;

#define CHECK(cond, what)                                                      \
    do {                                                                       \
        if (!(cond)) {                                                         \
            fprintf(stderr, "FAIL %s:%d %s\n", __FILE__, __LINE__, (what));    \
            failures++;                                                        \
        }                                                                      \
    } while (0)

/* Print and release a structured error; every failure path funnels here so a
 * broken error channel is itself caught. */
static void report_error(const char *what, PioError *error) {
    if (error) {
        fprintf(stderr, "%s: %s: %s\n", what,
                pio_error_code(error) ? pio_error_code(error) : "(no code)",
                pio_error_message(error) ? pio_error_message(error) : "(no message)");
        pio_error_release(error);
    } else {
        fprintf(stderr, "%s: failed with no error handle\n", what);
    }
    failures++;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s case.m [gridfm_dataset_dir]\n", argv[0]);
        return 2;
    }
    const char *case_path = argv[1];

    /* The handshake precedes every other call. */
    CHECK(pio_abi_version() == PIO_ABI_VERSION, "abi handshake");
    CHECK(pio_version() != NULL, "version string");
    {
        char *info = pio_build_info();
        CHECK(info && strstr(info, "\"abi\""), "build info reports the ABI");
        pio_string_release(info);
    }

    PioError *error = NULL;

    /* Structured failure: a missing file is a coded error, not a crash. */
    {
        PioModule *missing = pio_parse_file("does-not-exist.m", NULL, &error);
        CHECK(missing == NULL && error != NULL, "missing file fails with an error");
        if (error) {
            CHECK(pio_error_code(error) != NULL, "error carries a code");
            PioDiagnostics *records = pio_error_diagnostics(error);
            CHECK(records != NULL, "error diagnostics handle");
            pio_diagnostics_release(records);
            pio_error_release(error);
            error = NULL;
        }
    }

    /* NULL handles are no-ops or refusals, never crashes. */
    pio_module_release(NULL);
    pio_balanced_network_release(NULL);
    pio_error_release(NULL);
    pio_diagnostics_release(NULL);
    pio_string_release(NULL);
    CHECK(pio_module_kind(NULL) == NULL, "kind of NULL module");
    CHECK(pio_diagnostics_len(NULL) == 0, "len of NULL diagnostics");

    /* Parse the case into a module. */
    PioModule *module = pio_parse_file(case_path, NULL, &error);
    if (!module) {
        report_error("parse_file", error);
        return 1;
    }
    CHECK(strcmp(pio_module_kind(module), "balanced_network") == 0, "detected kind");

    /* retain/release independence: a retained sibling survives. */
    {
        PioModule *sibling = pio_module_retain(module);
        CHECK(sibling != NULL, "module retain");
        pio_module_release(sibling);
    }

    /* Structured diagnostics walk. */
    {
        PioDiagnostics *findings = pio_module_diagnostics(module, &error);
        CHECK(findings != NULL, "module diagnostics");
        size_t len = pio_diagnostics_len(findings);
        for (size_t i = 0; i < len; i++) {
            CHECK(pio_diagnostic_code(findings, i) != NULL, "diagnostic code");
            CHECK(pio_diagnostic_severity(findings, i) != NULL, "diagnostic severity");
            CHECK(pio_diagnostic_message(findings, i) != NULL, "diagnostic message");
        }
        CHECK(pio_diagnostic_code(findings, len) == NULL, "out of range row is NULL");
        pio_diagnostics_release(findings);
    }

    /* The typed value: an independently owned network handle that outlives
     * the module. */
    PioBalancedNetwork *net = pio_module_balanced_network(module, &error);
    if (!net) {
        report_error("module_balanced_network", error);
        pio_module_release(module);
        return 1;
    }
    size_t n = pio_balanced_network_n_buses(net);
    size_t m = pio_balanced_network_n_branches(net);
    size_t ng = pio_balanced_network_n_gens(net);
    CHECK(n > 0 && m > 0, "element counts");
    CHECK(pio_balanced_network_base_mva(net) > 0.0, "base MVA");

    /* Caller fill extractors: count query first, then exact fill. */
    {
        size_t total = pio_balanced_network_bus_ids(net, NULL, 0);
        CHECK(total == n, "bus id count query");
        int64_t *ids = malloc(n * sizeof *ids);
        CHECK(pio_balanced_network_bus_ids(net, ids, n) == n, "bus id fill");
        int64_t *from = malloc(m * sizeof *from);
        int64_t *to = malloc(m * sizeof *to);
        double *x = malloc(m * sizeof *x);
        CHECK(pio_balanced_network_branches(net, from, to, NULL, x, NULL, NULL, NULL,
                                            NULL, m) == m,
              "branch fill");
        double *pd = malloc(n * sizeof *pd);
        double *qd = malloc(n * sizeof *qd);
        CHECK(pio_balanced_network_bus_demand(net, pd, qd, n) == n, "demand fill");
        (void)ng;
        free(ids); free(from); free(to); free(x); free(pd); free(qd);
    }

    /* Same format emission echoes the source bytes; cross format emission
     * reports its findings through the structured channel. */
    {
        char *echo = pio_module_emit_string(module, "matpower", NULL, &error);
        if (!echo) report_error("emit_string matpower", error);
        else pio_string_release(echo);

        PioDiagnostics *losses = NULL;
        char *pm =
            pio_module_emit_string(module, "powermodels-json", &losses, &error);
        if (!pm) report_error("emit_string powermodels-json", error);
        else pio_string_release(pm);
        pio_diagnostics_release(losses);

        /* An unknown format is a coded refusal. */
        char *bad = pio_module_emit_string(module, "not-a-format", NULL, &error);
        CHECK(bad == NULL && error != NULL, "unknown format refused");
        if (error) {
            CHECK(strstr(pio_error_code(error), "REQUEST.WRITE") != NULL,
                  "unknown format code family");
            pio_error_release(error);
            error = NULL;
        }
    }

    /* The network JSON transport round trips, and a bare network transforms
     * into a module for semantic emission. */
    {
        char *json = pio_balanced_network_to_json(net, &error);
        if (!json) report_error("to_json", error);
        else {
            PioBalancedNetwork *rebuilt = pio_balanced_network_from_json(json, &error);
            if (!rebuilt) report_error("from_json", error);
            else {
                CHECK(pio_balanced_network_n_buses(rebuilt) == n, "round trip bus count");
                PioModule *wrapped = pio_balanced_network_to_module(rebuilt, &error);
                if (!wrapped) report_error("balanced_network_to_module", error);
                else {
                    char *text = pio_module_emit_string(wrapped, "matpower", NULL, &error);
                    if (!text) report_error("semantic emission", error);
                    else pio_string_release(text);
                    pio_module_release(wrapped);
                }
                pio_balanced_network_release(rebuilt);
            }
            pio_string_release(json);
        }
    }

    /* Wrong kind extraction is a coded refusal that keeps the module alive. */
    {
#ifdef PIO_DIST
        PioMulticonductorNetwork *wrong = pio_module_multiconductor_network(module, &error);
        CHECK(wrong == NULL && error != NULL, "wrong kind refused");
        if (error) { pio_error_release(error); error = NULL; }
        CHECK(strcmp(pio_module_kind(module), "balanced_network") == 0,
              "module survives the refusal");
#endif
    }

    /* Stored module document: emit and parse back through the universal path. */
    {
        char *stored = pio_module_emit_string(module, "pio-json", NULL, &error);
        if (!stored) report_error("emit_string pio-json", error);
        else {
            PioModule *reread =
                pio_parse_text("stored.pio.json", stored, "pio-json", &error);
            if (!reread) report_error("parse_str pio-json", error);
            else {
                CHECK(strcmp(pio_module_kind(reread), "balanced_network") == 0,
                      "stored round trip kind");
                pio_module_release(reread);
            }
            pio_string_release(stored);
        }
    }

    /* Released compatibility convenience for one call conversion. */
    {
        char *raw = pio_convert_file(case_path, NULL, "psse", NULL, NULL, &error);
        if (!raw) report_error("convert_file", error);
        else pio_string_release(raw);
    }

#ifdef PIO_PROB
    /* PioDcData owns ABI 6 matrix input arrays past the module's release. */
    {
        PioDcData *dc = pio_dc_data_build(module, "series_susceptance", &error);
        if (!dc) report_error("dc_data_build", error);
        else {
            size_t rows = pio_dc_data_n_rows(dc);
            size_t buses = pio_dc_data_n_buses(dc);
            CHECK(rows > 0 && buses == n, "PioDcData dimensions");
            const double *b = pio_dc_data_susceptance(dc);
            const int64_t *fi = pio_dc_data_from_indices(dc);
            CHECK(b != NULL && fi != NULL, "PioDcData spans");
            double *va = calloc(buses, sizeof *va);
            double *flow = malloc(rows * sizeof *flow);
            if (!pio_dc_data_calc_branch_flow(dc, va, buses, flow, rows, &error))
                report_error("calc_branch_flow", error);
            free(va); free(flow);
            pio_dc_data_release(dc);
        }
    }
#endif

#ifdef PIO_ARROW
    /* Arrow export off the network handle. */
    {
        struct ArrowArray array;
        struct ArrowSchema schema;
        memset(&array, 0, sizeof array);
        memset(&schema, 0, sizeof schema);
        int rc = pio_balanced_network_to_arrow(net, PIO_ARROW_TABLE_BUS, &array,
                                               &schema, &error);
        if (rc != 0) report_error("to_arrow", error);
        else {
            CHECK(array.length == (int64_t)n, "arrow bus rows");
            if (array.release) array.release(&array);
            if (schema.release) schema.release(&schema);
        }
        char *catalog = pio_arrow_catalog_json(&error);
        if (!catalog) report_error("arrow_catalog", error);
        else pio_string_release(catalog);
    }
#endif

    /* The module releases first; the network handle stays valid after. */
    pio_module_release(module);
    CHECK(pio_balanced_network_n_buses(net) == n, "child survives parent release");
    pio_balanced_network_release(net);

#ifdef PIO_DIST
    /* Distribution: an in-memory OpenDSS circuit through the same one parse. */
    {
        const char *dss =
            "new circuit.smoke basekv=12.47 bus1=src.1.2.3\n"
            "new line.l1 bus1=src.1.2.3 bus2=b2.1.2.3 length=1\n"
            "new load.d1 bus1=b2.1.2.3 kv=12.47 kw=100 model=1\n";
        PioModule *feeder = pio_parse_text("smoke.dss", dss, "dss", &error);
        if (!feeder) report_error("parse_str dss", error);
        else {
            CHECK(strcmp(pio_module_kind(feeder), "multiconductor_network") == 0,
                  "dss kind");
            PioMulticonductorNetwork *mc = pio_module_multiconductor_network(feeder, &error);
            if (!mc) report_error("multiconductor accessor", error);
            else {
                char *summary = pio_multiconductor_network_summary_json(mc, &error);
                if (!summary) report_error("mc summary", error);
                else pio_string_release(summary);
                pio_multiconductor_network_release(mc);
            }
            pio_module_release(feeder);
        }
    }
#endif

#ifdef PIO_GRIDFM
    /* A GridFM dataset parses to a scenario set when a directory is given. */
    if (argc > 2) {
        PioModule *dataset = pio_parse_file(argv[2], NULL, &error);
        if (!dataset) report_error("parse_file gridfm", error);
        else {
            CHECK(strcmp(pio_module_kind(dataset), "balanced_network_scenario_set") == 0,
                  "gridfm kind");
            char *inventory = pio_module_list_states_json(dataset, &error);
            if (!inventory) report_error("list states", error);
            else pio_string_release(inventory);
            pio_module_release(dataset);
        }
    }
#endif

    if (failures) {
        fprintf(stderr, "%d smoke failure(s)\n", failures);
        return 1;
    }
    puts("C ABI smoke OK");
    return 0;
}
