# powerio-tx

`powerio-tx` owns the balanced transmission model and its format parsers. A
successful parse returns `PioModule<BalancedNetwork>`, retaining source bytes
and structured diagnostics. The top level `powerio` facade adds universal
format dispatch, dynamic values, stored modules, and `emit`.

Format support covers MATPOWER, PSS/E RAW revisions 33 through 35, PSS/E RAWX
revision 35, PowSybl XIIDM 1.17, CIM CGMES 2.4.15 and 3.0, PowerWorld AUX,
PSLF, PowerModels JSON, egret JSON, pandapower JSON, PyPSA CSV folders, and
Surge JSON. GOC3 problem JSON and PowerWorld PWB are read only inputs in this
crate. `powerio_tx::parse` exposes the network projection for a Rust consumer
that deliberately omits `powerio-prob`; its diagnostics name the calculation
data that projection does not contain. The top level `powerio::parse` is the
cross language operation: it returns `AcScucInstance` for a GOC3 problem and
`AcScucSolution` for a problem with its matching solution.

```rust
use powerio_core::Source;
use powerio_tx::parse;

let module = parse(Source::open("case14.m")?)?;
let net = module.value();
assert_eq!(net.buses().len(), 14);
```

The [workspace README](https://github.com/eigenergy/powerio) lists the CLI,
language bindings, matrix builders, and validation commands.
