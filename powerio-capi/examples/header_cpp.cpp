// C++ include and link probe. The public ABI itself remains C.
#include "powerio.h"

#include <type_traits>

static_assert(PIO_ABI_VERSION == 7);
static_assert(std::is_standard_layout_v<PioStringView>);
static_assert(std::is_standard_layout_v<PioByteView>);
static_assert(std::is_standard_layout_v<PioF64View>);
static_assert(std::is_standard_layout_v<PioSizeView>);
static_assert(std::is_standard_layout_v<PioModuleProducerView>);
static_assert(std::is_standard_layout_v<PioModuleSourceView>);
static_assert(std::is_standard_layout_v<PioModuleSourceMapEntryView>);
static_assert(std::is_standard_layout_v<PioSourceSpanView>);
static_assert(std::is_standard_layout_v<PioModuleHistoryEntryView>);
static_assert(std::is_standard_layout_v<PioModuleHistoryParameterView>);
static_assert(std::is_standard_layout_v<PioModuleExtensionView>);
static_assert(std::is_standard_layout_v<PioJsonValueView>);
static_assert(std::is_standard_layout_v<PioJsonObjectEntryView>);
static_assert(std::is_standard_layout_v<PioBalancedGeoView>);
static_assert(std::is_standard_layout_v<PioBalancedLocationView>);
static_assert(std::is_standard_layout_v<PioBalancedBusView>);
static_assert(std::is_standard_layout_v<PioBalancedLoadView>);
static_assert(std::is_standard_layout_v<PioBalancedBranchView>);
static_assert(std::is_standard_layout_v<PioBalancedGeneratorView>);
static_assert(std::is_standard_layout_v<PioBalancedStorageView>);
static_assert(std::is_standard_layout_v<PioDetailedTerminalView>);
static_assert(std::is_standard_layout_v<PioTapChangerView>);
static_assert(std::is_standard_layout_v<PioDcBusSpecificationView>);
static_assert(std::is_standard_layout_v<PioAcBusSpecificationView>);
static_assert(std::is_standard_layout_v<PioScucDeviceView>);
static_assert(std::is_standard_layout_v<PioMulticonductorGeoView>);
static_assert(std::is_standard_layout_v<PioMulticonductorLocationView>);
static_assert(std::is_standard_layout_v<PioMulticonductorNetworkCountsView>);
static_assert(std::is_standard_layout_v<PioMulticonductorBusView>);
static_assert(std::is_standard_layout_v<PioMulticonductorLineCodeView>);
static_assert(std::is_standard_layout_v<PioMulticonductorLineView>);
static_assert(std::is_standard_layout_v<PioMulticonductorSwitchView>);
static_assert(std::is_standard_layout_v<PioMulticonductorTransformerView>);
static_assert(std::is_standard_layout_v<PioMulticonductorTransformerWindingView>);
static_assert(std::is_standard_layout_v<PioMulticonductorLoadView>);
static_assert(std::is_standard_layout_v<PioMulticonductorGeneratorView>);
static_assert(std::is_standard_layout_v<PioInverterBasedResourceView>);
static_assert(std::is_standard_layout_v<PioControlProfileView>);
static_assert(std::is_standard_layout_v<PioMulticonductorShuntView>);
static_assert(std::is_standard_layout_v<PioMulticonductorCapacitorView>);
static_assert(std::is_standard_layout_v<PioVoltageSourceView>);
static_assert(std::is_standard_layout_v<PioMulticonductorUntypedObjectView>);
static_assert(std::is_standard_layout_v<PioMulticonductorUntypedPropertyView>);
static_assert(std::is_standard_layout_v<PioMulticonductorCommandView>);

int main() {
    return pio_abi_version() == PIO_ABI_VERSION ? 0 : 1;
}
