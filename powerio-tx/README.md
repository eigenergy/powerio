# powerio-tx

`powerio-tx` owns the balanced transmission model and its format parsers. A
successful parse returns `PioModule<BalancedNetwork>`, retaining source bytes
and structured diagnostics. The top level `powerio` facade adds universal
format dispatch, dynamic values, stored modules, and `emit`.

Format support covers MATPOWER, PSS/E, PowerWorld AUX, PSLF,
PowerModels JSON, egret JSON, pandapower JSON, PyPSA CSV folders, and Surge
JSON. GOC3 JSON and PowerWorld PWB are read only inputs.

```rust
use powerio_core::Source;
use powerio_tx::parse;

let module = parse(Source::open("case14.m")?)?;
let net = module.value();
assert_eq!(net.buses().len(), 14);
```

The [workspace README](https://github.com/eigenergy/powerio) lists the CLI,
language bindings, matrix builders, and validation commands.
