/* Runtime probe for packaged ABI 7 libraries. */
#include "powerio.h"

#include <stdio.h>
#include <string.h>

#define CHECK(condition, message)                                \
    do {                                                         \
        if (!(condition)) {                                      \
            fprintf(stderr, "release probe: %s\n", (message)); \
            return 1;                                            \
        }                                                        \
    } while (0)

static int view_equals(PioStringView view, const char *text) {
    size_t len = strlen(text);
    return view.len == len && view.data != NULL && memcmp(view.data, text, len) == 0;
}

int main(int argc, char **argv) {
    CHECK(argc == 2, "expected release version argument");
    CHECK(view_equals(pio_version(), argv[1]), "library version mismatch");
    CHECK(pio_abi_version() == PIO_ABI_VERSION, "ABI mismatch");
    CHECK(PIO_ABI_VERSION == 7, "release probe is not ABI 7");

    PioError *error = NULL;
    PioString *schema = pio_schema_report(&error);
    CHECK(schema != NULL, "schema report missing");
    PioStringView schema_text = pio_string_view(schema);
    CHECK(schema_text.data != NULL && schema_text.len != 0, "schema report is empty");
    pio_string_release(schema);

    const char dss[] = "new circuit.probe basekv=12.47 bus1=src.1.2.3\n";
    PioSource *source = pio_source_from_memory(
        "probe.dss", 9, (const uint8_t *)dss, sizeof dss - 1, &error);
    CHECK(source != NULL, "memory source failed");
    PioModule *module = pio_parse(source, "dss", 3, &error);
    pio_source_release(source);
    CHECK(module != NULL, "distribution parse failed");
    PioValueHandle *value = pio_module_value(module);
    CHECK(value != NULL, "parsed value missing");
    CHECK(view_equals(pio_value_type_name(value), "powerio.MulticonductorNetwork"),
          "distribution structural type mismatch");
    pio_value_release(value);
    pio_module_release(module);

    printf("powerio %.*s; ABI %u\n", (int)pio_version().len, pio_version().data,
           pio_abi_version());
    return 0;
}
