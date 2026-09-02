# PowerFactory DGS fixtures

DIgSILENT PowerFactory DGS V5 ASCII exports vendored byte for byte from the
PowSybl Core test resources, directory
`powerfactory/powerfactory-converter/src/test/resources/`, at commit
`0939bfcc2c0c094de907dc818dd688b4cbfb7281` (the PowSybl Core 7.3.0 build the
PowSybl interoperability gate pins). PowSybl Core is licensed under the Mozilla
Public License 2.0, (c) RTE and the PowSybl contributors,
<https://github.com/powsybl/powsybl-core/blob/main/LICENSE.txt>. No file was
edited.

| File | Exercises |
| --- | --- |
| `ieee14.dgs` | The IEEE 14 bus case: lines, three transformers with off nominal rated voltages, loads, a capacitor bank, and five machines |
| `Tower.dgs` | Two circuits on one `ElmTow` tower whose `TypTow` states positive sequence circuit matrices, plus decimal commas |
| `Hvdc.dgs` | Two `ElmVsc` converters joined by two DC lines, read as one HVDC record |
| `Switches.dgs` | An `ElmCoup` breaker joining two terminals into one calculated bus, and solved terminal voltages |
| `ExternalGrid.dgs` | An `ElmXnet` slack external grid |
| `CapabilityCurve.dgs` | An `IntQlim` reactive capability curve on a machine |
| `ThreeWindingsTransformerVoltageControl.dgs` | An `ElmTr3` three winding transformer with a voltage regulating tap changer |
| `Transformer-Phase-with-mTaps.dgs` | A phase shifting transformer with an explicit `mTaps` table |
| `MediumVoltageLoad.dgs` | `ElmLodmv` medium voltage loads with every `mode_inp` pair and generation beside demand |
| `robustness.dgs` | Optional attributes absent from the export, `DEF` and `EC` input modes, and dangling cubicles |

SHA-256 digests:

```text
ea031bcf34c5d012846f20b262c208c397b36850c999798af1a30d39fdf70090  CapabilityCurve.dgs
74369aa4fada066d6c1db4348a0c00f48286eb827c6f58cce5fa61e47c710c25  ExternalGrid.dgs
c7ccaf906ee9850918b4b363d462b09e09b00e973d441e3d553cdf8e1d9ac884  Hvdc.dgs
2e2d3a45383b508ccd94210c805adf73bcd639c48c5457c3d8bbca043e908e55  ieee14.dgs
b2f83749020d7072bbc202b43c214558f4fcefba67ccd053b562b028cb436cb6  MediumVoltageLoad.dgs
415617c8d7201c114ad831eef1708c75812ee3fb8c947bb06d6cd0b737c07141  robustness.dgs
1cd38e4cc62e2b2a155c40bd4a3f74a2c30c010bb6074d9f1ffbce18c690a691  Switches.dgs
5c2cfa0baab07f3bdc35caf55137031fc9b95bf89e3e64ef9b50491b3e78d364  ThreeWindingsTransformerVoltageControl.dgs
46fdfd0d9087d110b2f9df8ebe171487edf7654634e4811bb79a1524562b1df4  Tower.dgs
19985e11e9cb00e8f58f5a08daf3b4b2b41715e4c22746927ba6af60eec25550  Transformer-Phase-with-mTaps.dgs
```

`lv-feeder.dgs` is original to this repository under its code license: a
synthetic four wire low voltage feeder whose terminals state a phase
technology, whose line type states a neutral conductor, and whose loads state
per phase demand, so it routes to the multiconductor network.
