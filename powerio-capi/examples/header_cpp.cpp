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
#endif

int main() {
    return pio_abi_version() == PIO_ABI_VERSION ? 0 : 1;
}
