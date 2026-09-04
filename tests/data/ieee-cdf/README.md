# IEEE Common Data Format fixtures

`ieee14cdf.txt` and `ieee30cdf.txt` are the IEEE 14 bus and 30 bus test
cases in the IEEE Common Data Format, as published by the University of
Washington Power Systems Test Case Archive (`https://labs.ece.uw.edu/pstca/`,
UW ARCHIVE title cards dated 08/19/93 and 08/20/93).

The two files are copied without any edit from the
[PowSybl Core repository](https://github.com/powsybl/powsybl-core), where they sit under
`ieee-cdf/ieee-cdf-model/src/main/resources/` and are distributed under the
Mozilla Public License 2.0. The files match the copies in that tree at commit
`0939bfcc2c0c094de907dc818dd688b4cbfb7281`, the PowSybl Core
commit the PowSybl interoperability gate pins:

| File | Lines | Bytes | SHA-256 |
| --- | --- | --- | --- |
| `ieee14cdf.txt` | 48 | 4664 | `68afd87021f42eca37d2787ad71db101cd2170d40528c291e8e94c9dd427abd8` |
| `ieee30cdf.txt` | 85 | 9275 | `8a4833f02f012b316ad978a176100963d162d17019f8ea71627e7920118e6c1a` |

`ieee30cdf.txt` carries a quirk of the archive copy: its interchange data
record follows the `-9` terminator instead of preceding it, so the record
sits outside any section. The reader reports it and reads no area.

The MATPOWER cases `tests/data/case14.m` and `tests/data/case30.m` derive
from the same IEEE data. `case14.m` was converted from `ieee14cdf.txt` by
MATPOWER's `cdf2matp`; `case30.m` restates the 30 bus system from Alsac and
Stott with rounded branch parameters, rescaled shunts, and different
generator locations and limits. The integration tests in
`powerio-tx/tests/ieee_cdf.rs` state those differences.

The remaining public IEEE CDF cases (9, 57, 118, and 300 buses) are not
copied; the PowSybl interoperability gate reads them from its sparse
checkout of PowSybl Core.
