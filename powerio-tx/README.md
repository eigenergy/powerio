# powerio-tx

`powerio-tx` is the balanced transmission model and its format parsers. A
successful parse returns `PioModule<BalancedNetwork>` with the source bytes
retained and structured diagnostics attached. The top level `powerio` facade
adds universal format dispatch, dynamic values, stored modules, and `emit` on
top of it.

The supported formats are MATPOWER, PSS/E RAW revisions 32 through 35 (fresh
output at 33 through 35), PSS/E RAWX revision 35, PowSybl XIIDM and JIIDM 1.0
through 1.17 (fresh output at 1.17), CIM CGMES 2.4.15 and 3.0 (fresh output
at 3.0), ENTSO-E UCTE-DEF, PowerWorld AUX, PSLF, PowerModels JSON, egret JSON,
pandapower JSON, PyPSA CSV directories, and Surge JSON. PowerWorld PWB, the
IEEE Common Data Format, and DeepMind OPFData are read only. GO Challenge 3
defines a calculation rather than a bare network, so `powerio_tx::parse`
refuses it; the top level `powerio::parse` returns `AcScucInstance` for a
GO Challenge 3 problem and `AcScucSolution` for a problem with its solution.

```rust
use powerio_core::Source;
use powerio_tx::parse;

let module = parse(Source::open("case14.m")?)?;
let net = module.value();
assert_eq!(net.buses().len(), 14);
```

The [workspace README](https://github.com/eigenergy/powerio) lists the CLI,
language bindings, matrix builders, and validation commands.
