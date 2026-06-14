# powerio

`powerio` parses power system case files into a typed `Network`, writes retained
source text back to the same format, and converts between MATPOWER, PSS/E,
PowerWorld, PowerModels JSON, and egret JSON. Display artifacts such as
PowerWorld `.pwd` use `parse_display_file` instead of `parse_file`.

```rust
use powerio::{TargetFormat, parse_file};

let parsed = parse_file("case14.m", None)?;
let net = parsed.network;
let converted = net.to_format(TargetFormat::PowerModelsJson);
std::fs::write("case14.json", converted.text)?;
```

The workspace README covers the CLI, Python package, C ABI, matrix builders, and
validation suite: <https://github.com/eigenergy/powerio>.
