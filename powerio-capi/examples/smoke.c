/* End to end C probe for ABI 7.
 *
 *   cc -I powerio-capi/include powerio-capi/examples/smoke.c \
 *      -L target/release -lpowerio_capi -o smoke
 *   ./smoke tests/data/case9.m [gridfm_dataset_dir]
 */
#include "powerio.h"

#include <stdio.h>
#include <string.h>

#if PIO_ABI_VERSION != 7
#error "update the C probe for the current ABI"
#endif

static int failures;

#define CHECK(condition, message)                                              \
    do {                                                                       \
        if (!(condition)) {                                                    \
            fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, message); \
            failures++;                                                        \
        }                                                                      \
    } while (0)

static int view_equals(PioStringView view, const char *text) {
    size_t len = strlen(text);
    return view.len == len && view.data != NULL && memcmp(view.data, text, len) == 0;
}

static void report_error(const char *operation, PioError *error) {
    if (error == NULL) {
        fprintf(stderr, "%s failed without a PioError\n", operation);
    } else {
        PioStringView code = pio_error_code(error);
        PioStringView message = pio_error_message(error);
        fprintf(stderr, "%s: %.*s: %.*s\n", operation, (int)code.len, code.data,
                (int)message.len, message.data);
        pio_error_release(error);
    }
    failures++;
}

int main(int argc, char **argv) {
#ifdef PIO_GRIDFM
    if (argc < 2 || argc > 3) {
        fprintf(stderr, "usage: %s case9.m [gridfm_dataset_dir]\n", argv[0]);
#else
    if (argc != 2) {
        fprintf(stderr, "usage: %s case9.m\n", argv[0]);
#endif
        return 2;
    }

    CHECK(pio_abi_version() == PIO_ABI_VERSION, "ABI handshake");
    CHECK(pio_version().len != 0, "version string");

    PioError *error = NULL;
    PioSource *missing = pio_source_open("missing-case.m", 14, &error);
    CHECK(missing == NULL && error != NULL, "missing source reports an error");
    if (error != NULL) {
        PioDiagnostics *diagnostics = pio_error_diagnostics(error);
        CHECK(diagnostics != NULL && pio_diagnostics_len(diagnostics) != 0,
              "failure carries diagnostics");
        pio_diagnostics_release(diagnostics);
        pio_error_release(error);
        error = NULL;
    }

    PioSource *source = pio_source_open(argv[1], strlen(argv[1]), &error);
    if (source == NULL) {
        report_error("pio_source_open", error);
        return 1;
    }
    PioModule *module = pio_parse(source, NULL, 0, &error);
    pio_source_release(source);
    if (module == NULL) {
        report_error("pio_parse", error);
        return 1;
    }

    PioDiagnostics *diagnostics = pio_module_diagnostics(module);
    CHECK(diagnostics != NULL, "module diagnostics");
    for (size_t i = 0; i < pio_diagnostics_len(diagnostics); i++) {
        CHECK(pio_diagnostic_code(diagnostics, i).len != 0, "diagnostic code");
        CHECK(pio_diagnostic_severity(diagnostics, i).len != 0,
              "diagnostic severity");
    }
    pio_diagnostics_release(diagnostics);

    PioValueHandle *value = pio_module_value(module);
    CHECK(value != NULL, "module value");
    CHECK(view_equals(pio_value_type_name(value), "powerio.BalancedNetwork"),
          "balanced structural type");
    CHECK(pio_value_is_type(value, "powerio.BalancedNetwork", 23),
          "exact structural type predicate");

    PioBalancedNetwork *network = pio_value_balanced_network(value, &error);
    pio_value_release(value);
    if (network == NULL) {
        report_error("pio_value_balanced_network", error);
        pio_module_release(module);
        return 1;
    }
    size_t buses = pio_balanced_network_bus_count(network);
    size_t branches = pio_balanced_network_branch_count(network);
    CHECK(buses == 9 && branches == 9, "case9 element counts");
    CHECK(pio_balanced_network_base_mva(network) == 100.0, "case9 base MVA");
    PioBalancedBusView bus;
    CHECK(pio_balanced_network_bus_at(network, 0, &bus, &error),
          "checked bus view");
    CHECK(bus.id == 1 && view_equals(bus.bus_type, "REF"),
          "case9 reference bus");
    PioBalancedBranchView branch;
    CHECK(pio_balanced_network_branch_at(network, 0, &branch, &error),
          "checked branch view");
    CHECK(branch.from_bus_id == 1 && branch.to_bus_id == 4 &&
              branch.rate_a_mva == 250.0 && branch.effective_tap_ratio == 1.0,
          "case9 first branch");
    PioBalancedGeneratorView generator;
    CHECK(pio_balanced_network_generator_at(network, 0, &generator, &error),
          "checked generator view");
    CHECK(generator.has_cost && generator.cost.coefficients.len == 3,
          "case9 generator cost span");

    CHECK(!pio_balanced_network_bus_at(network, buses, &bus, &error) &&
              error != NULL,
          "out of range row reports an error");
    if (error != NULL) {
        CHECK(view_equals(pio_error_code(error), "BIND.CAPI.INDEX_OUT_OF_RANGE"),
              "out of range diagnostic code");
        pio_error_release(error);
        error = NULL;
    }

    PioSparseMatrix *incidence =
        pio_calc_incidence_matrix(network, NULL, 0, &error);
    if (incidence == NULL) {
        report_error("pio_calc_incidence_matrix", error);
    } else {
        CHECK(pio_sparse_matrix_rows(incidence) == branches, "incidence rows");
        CHECK(pio_sparse_matrix_columns(incidence) == buses, "incidence columns");
        CHECK(pio_sparse_matrix_values(incidence).len == 2 * branches,
              "incidence nonzeros");
        pio_sparse_matrix_release(incidence);
    }

    PioDestination *memory = pio_destination_memory("case9.m", 7, &error);
    PioEmitResult *emitted = pio_emit(module, "matpower", 8, memory, &error);
    pio_destination_release(memory);
    if (emitted == NULL) {
        report_error("pio_emit", error);
    } else {
        CHECK(pio_emit_result_artifact_count(emitted) == 1, "one MATPOWER artifact");
        PioArtifact *artifact = pio_emit_result_artifact(emitted, 0, &error);
        CHECK(artifact != NULL && pio_artifact_bytes(artifact).len != 0,
              "MATPOWER bytes");
        pio_artifact_release(artifact);
        pio_emit_result_release(emitted);
    }

    /* The update detaches the module while this borrowed network keeps the
     * pre-edit value alive. */
    PioComponentId *load = pio_component_id_new("load", 4, "bus-5", 5, &error);
    PioActivePower *replacement = pio_active_power_from_megawatts(91.0);
    PioOperatingPointUpdate *operating =
        pio_operating_point_update_set_load_active_power(load, NULL, 0, replacement, &error);
    PioCalculationUpdate *update =
        pio_calculation_update_from_operating_point(operating, &error);
    const PioCalculationUpdate *updates[] = {update};
    PioUpdateReport *report = pio_apply_updates(module, updates, 1, &error);
    if (report == NULL) {
        report_error("pio_apply_updates", error);
    } else {
        CHECK(pio_update_report_len(report) == 1, "one exact update change");
        CHECK(!pio_update_report_connectivity_changed(report),
              "load power does not change connectivity");
        PioUpdateChange *change = pio_update_report_change(report, 0, &error);
        CHECK(change != NULL, "update change view");
        CHECK(view_equals(pio_update_change_field(change), "load_active_power"),
              "updated field name");
        PioComponentId *changed_id = pio_update_change_component_id(change);
        CHECK(view_equals(pio_component_id_type(changed_id), "load"),
              "changed component type");
        CHECK(view_equals(pio_component_id_local_id(changed_id), "bus-5"),
              "changed component identity");
        pio_component_id_release(changed_id);
        pio_update_change_release(change);
        pio_update_report_release(report);
    }
    pio_calculation_update_release(update);
    pio_operating_point_update_release(operating);
    pio_active_power_release(replacement);
    pio_component_id_release(load);

    CHECK(pio_balanced_network_bus_count(network) == buses,
          "borrowed pre-edit network remains valid");
    pio_balanced_network_release(network);

    PioDestination *ir_memory =
        pio_destination_memory("roundtrip.pio.json", 18, &error);
    PioEmitResult *serialized = pio_module_serialize(module, ir_memory, &error);
    pio_destination_release(ir_memory);
    if (serialized == NULL) {
        report_error("pio_module_serialize", error);
    } else {
        PioArtifact *artifact = pio_emit_result_artifact(serialized, 0, &error);
        PioByteView bytes = pio_artifact_bytes(artifact);
        PioSource *ir_source =
            pio_source_from_memory("roundtrip.pio.json", 18, bytes.data, bytes.len, &error);
        PioModule *roundtrip = pio_module_deserialize(ir_source, &error);
        CHECK(roundtrip != NULL, "PowerIO IR round trip");
        pio_module_release(roundtrip);
        pio_source_release(ir_source);
        pio_artifact_release(artifact);
        pio_emit_result_release(serialized);
    }

    pio_module_release(module);

#ifdef PIO_GRIDFM
    if (argc == 3) {
        PioSource *dataset_source =
            pio_source_open(argv[2], strlen(argv[2]), &error);
        PioModule *dataset = NULL;
        if (dataset_source == NULL) {
            report_error("pio_source_open GridFM", error);
            error = NULL;
        } else {
            dataset = pio_parse(dataset_source, NULL, 0, &error);
            pio_source_release(dataset_source);
            if (dataset == NULL) {
                report_error("pio_parse GridFM", error);
                error = NULL;
            }
        }
        if (dataset != NULL) {
            PioValueHandle *dataset_value = pio_module_value(dataset);
            CHECK(dataset_value != NULL, "GridFM module value");
            CHECK(pio_value_is_type(
                      dataset_value,
                      "powerio.ScenarioSet<powerio.BalancedNetwork>",
                      sizeof("powerio.ScenarioSet<powerio.BalancedNetwork>") -
                          1),
                  "GridFM structural type");
            PioScenarioSetHandle *scenarios =
                pio_value_scenario_set(dataset_value, &error);
            if (scenarios == NULL) {
                report_error("pio_value_scenario_set", error);
                error = NULL;
            } else {
                CHECK(pio_scenario_set_len(scenarios) != 0,
                      "GridFM scenario count");
                CHECK(view_equals(pio_scenario_set_element_type(scenarios),
                                  "powerio.BalancedNetwork"),
                      "GridFM scenario element type");
                PioValueHandle *first =
                    pio_scenario_set_get_at(scenarios, 0, &error);
                CHECK(first != NULL, "GridFM first scenario");
                if (first != NULL) {
                    CHECK(view_equals(pio_value_type_name(first),
                                      "powerio.BalancedNetwork"),
                          "GridFM scenario value type");
                    pio_value_release(first);
                }
                pio_scenario_set_release(scenarios);
            }
            pio_value_release(dataset_value);
            pio_module_release(dataset);
        }
    }
#endif

    pio_module_release(NULL);
    pio_value_release(NULL);
    pio_error_release(NULL);
    pio_diagnostics_release(NULL);
    pio_sparse_matrix_release(NULL);
    pio_vector_release(NULL);

    if (failures == 0) {
        puts("PowerIO C ABI 7 probe passed");
    }
    return failures == 0 ? 0 : 1;
}
