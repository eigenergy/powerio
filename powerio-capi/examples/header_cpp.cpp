// C++ include and link probe. The public ABI itself remains C.
#include "powerio.h"

#include <type_traits>

static_assert(PIO_ABI_VERSION == 7);
static_assert(std::is_standard_layout_v<PioStringView>);
static_assert(std::is_standard_layout_v<PioByteView>);
static_assert(std::is_standard_layout_v<PioF64View>);
static_assert(std::is_standard_layout_v<PioSizeView>);
static_assert(std::is_standard_layout_v<PioBalancedBusView>);
static_assert(std::is_standard_layout_v<PioBalancedLoadView>);
static_assert(std::is_standard_layout_v<PioBalancedBranchView>);
static_assert(std::is_standard_layout_v<PioBalancedGeneratorView>);
static_assert(std::is_standard_layout_v<PioBalancedStorageView>);
static_assert(std::is_standard_layout_v<PioDcBusSpecificationView>);
static_assert(std::is_standard_layout_v<PioAcBusSpecificationView>);
static_assert(std::is_standard_layout_v<PioScucDeviceView>);

int main() {
    return pio_abi_version() == PIO_ABI_VERSION ? 0 : 1;
}
