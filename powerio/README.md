# powerio

`powerio` parses power system case files into a typed `Network`, writes retained
source text back to the same format, and converts between MATPOWER, PSS/E,
PowerWorld, PowerModels JSON, and egret JSON.

```rust
use powerio::{TargetFormat, parse_matpower_file, write_as};

let net = parse_matpower_file("case14.m")?;
let converted = write_as(&net, TargetFormat::PowerModelsJson);
std::fs::write("case14.json", converted.text)?;
# Ok::<(), powerio::Error>(())
```

The workspace README covers the CLI, Python package, C ABI, matrix builders, and
validation suite: <https://github.com/eigenergy/powerio>.
