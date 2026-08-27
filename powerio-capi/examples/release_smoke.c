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
    CHECK(pio_dist_abi_version() == PIO_DIST_ABI_VERSION, "distribution ABI mismatch");

    const char *features[] = {"arrow", "matrix", "gridfm", "dist", "prob"};
    for (size_t i = 0; i < sizeof features / sizeof features[0]; i++) {
        CHECK(pio_has_feature(features[i]) == 1, "required release feature missing");
    }

    char *schemas = pio_schema_versions_json();
    CHECK(schemas != NULL, "schema version report missing");
    CHECK(strstr(schemas, "powerio_version") != NULL, "schema report has no release version");
    pio_string_free(schemas);

    char *build = pio_build_info();
    CHECK(build != NULL, "build report missing");
    CHECK(strstr(build, argv[1]) != NULL, "build report has the wrong release version");
    for (size_t i = 0; i < sizeof features / sizeof features[0]; i++) {
        CHECK(strstr(build, features[i]) != NULL, "build report omits a release feature");
    }
    pio_string_free(build);

    /* Exercise one representative entry point for each additive feature. */
    char err[PIO_ERRBUF_MIN] = {0};
    char *arrow = pio_arrow_catalog_json(err, sizeof err);
    CHECK(arrow != NULL, err);
    pio_string_free(arrow);
    CHECK(pio_matrix_available() == 1, "matrix entry point is unavailable");
    CHECK(pio_scenario_ids("", "gridfm", NULL, 0, err, sizeof err) < 0,
          "invalid gridfm path should fail through the gridfm entry point");
    char *dist = pio_dist_capabilities_json();
    CHECK(dist != NULL, "distribution capability report missing");
    pio_string_free(dist);

    printf("powerio %s; ABI %u; distribution ABI %u; release features OK\n",
           pio_version(), pio_abi_version(), pio_dist_abi_version());
    return 0;
}
