/* Runtime probe for the five packaged release libraries. */
#include "powerio.h"

#include <stdio.h>
#include <string.h>

#define CHECK(condition, message)                                              \
    do {                                                                       \
        if (!(condition)) {                                                    \
            fprintf(stderr, "release smoke: %s\n", (message));               \
            return 1;                                                          \
        }                                                                      \
    } while (0)

int main(int argc, char **argv) {
    CHECK(argc == 2, "expected release version argument");
    CHECK(strcmp(pio_version(), argv[1]) == 0, "library version mismatch");
    CHECK(pio_abi_version() == PIO_ABI_VERSION, "core ABI mismatch");

    const char *features[] = {"arrow", "matrix", "gridfm", "dist", "prob"};
    for (size_t i = 0; i < sizeof features / sizeof features[0]; i++) {
        CHECK(pio_has_feature(features[i]) == 1, "required release feature missing");
    }

    char *schemas = pio_schema_versions_json();
    CHECK(schemas != NULL, "schema version report missing");
    CHECK(strstr(schemas, "powerio_version") != NULL, "schema report has no release version");
    pio_string_release(schemas);

    char *build = pio_build_info();
    CHECK(build != NULL, "build report missing");
    CHECK(strstr(build, argv[1]) != NULL, "build report has the wrong release version");
    for (size_t i = 0; i < sizeof features / sizeof features[0]; i++) {
        CHECK(strstr(build, features[i]) != NULL, "build report omits a release feature");
    }
    pio_string_release(build);

    /* Exercise one representative entry point for each additive feature and
     * the module surface itself. */
    PioError *error = NULL;
    char *arrow = pio_arrow_catalog_json(&error);
    CHECK(arrow != NULL, "arrow catalog missing");
    pio_string_release(arrow);

    PioModule *missing = pio_parse_file("release-smoke-missing.m", NULL, &error);
    CHECK(missing == NULL && error != NULL,
          "a missing case must fail with a structured error");
    CHECK(pio_error_code(error) != NULL, "structured error carries a code");
    pio_error_release(error);
    error = NULL;

    const char *dss = "new circuit.probe basekv=12.47 bus1=src.1.2.3\n";
    PioModule *feeder = pio_parse_text("probe.dss", dss, "dss", &error);
    CHECK(feeder != NULL, "distribution parse through the release library failed");
    CHECK(strcmp(pio_module_kind(feeder), "multiconductor_network") == 0,
          "distribution kind mismatch");
    pio_module_release(feeder);

    printf("powerio %s; ABI %u; release features OK\n",
           pio_version(), pio_abi_version());
    return 0;
}
