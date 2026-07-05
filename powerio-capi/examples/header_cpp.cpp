// C++ header/link sanity check. The ABI is C, but many direct consumers include
// powerio.h from C++ and rely on the extern "C" guard.
#include "powerio.h"

#include <cstddef>
#include <cstdint>

static_assert(PIO_ABI_VERSION == 4);
static_assert(PIO_ERRBUF_MIN == 256);

#ifdef PIO_DIST
static_assert(PIO_DIST_ABI_VERSION == 1);
#endif

#ifdef PIO_ARROW
static_assert(PIO_ARROW_TABLE_BUS == 0);
static_assert(PIO_ARROW_TABLE_BRANCH == 1);
static_assert(PIO_ARROW_TABLE_GEN == 2);
static_assert(PIO_ARROW_TABLE_LOAD == 3);
static_assert(PIO_ARROW_TABLE_SHUNT == 4);
static_assert(PIO_ARROW_TABLE_YBUS == 15);
static_assert(PIO_ARROW_TABLE_INCIDENCE == 16);
static_assert(PIO_ARROW_TABLE_BPRIME == 17);
static_assert(PIO_ARROW_TABLE_BDOUBLEPRIME == 18);
static_assert(PIO_ARROW_TABLE_MATRIX_BUS == 19);
static_assert(PIO_ARROW_TABLE_MATRIX_BRANCH == 20);
#endif

int main() {
    (void)pio_matrix_available();
    return pio_abi_version() == PIO_ABI_VERSION ? 0 : 1;
}
