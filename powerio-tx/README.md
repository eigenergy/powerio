# powerio

`powerio` parses power system case files into a typed `Network`. Readers retain
source text where supported, so writing back to the same format can return the
original bytes. Cross format conversion passes through the format neutral
network model and reports fields the target cannot represent.

Read and write support covers MATPOWER, PSS/E, PowerWorld AUX, PSLF,
PowerModels JSON, egret JSON, pandapower JSON, PyPSA CSV folders, and Surge
JSON. GOC3 JSON and PowerWorld PWB are read only inputs.

```rust
use powerio::{TargetFormat, parse_file};

let parsed = parse_file("case14.m", None)?;
let net = parsed.network;
let converted = net.to_format(TargetFormat::PowerModelsJson)?;
std::fs::write("case14.json", converted.text)?;
```

The [workspace README](https://github.com/eigenergy/powerio) lists the CLI,
language bindings, matrix builders, and validation commands.
