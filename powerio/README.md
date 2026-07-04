# powerio

`powerio` is the core Rust crate for parsing power system case files into a
typed `Network`, retaining same format source text where the reader supports
it, and converting through the format neutral model. The workspace README has
the full format matrix and language bindings.

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

The workspace README covers the CLI, Python package, C ABI, matrix builders, and
validation suite: <https://github.com/eigenergy/powerio>.
