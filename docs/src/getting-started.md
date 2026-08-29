# Getting Started

## Install

Rust:

```sh
cargo add powerio            # parsing, conversion, .pio.json
cargo add powerio -F matrix  # + sparse matrices and graph data
```

Python (parse, convert, and write need only the interpreter; matrices pull scipy):

```sh
pip install powerio          # or: pip install "powerio[all]"
```

Julia:

```julia
using Pkg; Pkg.add("PowerIO")
```

C and C++: clone the repository, build the shared library, and use the installed header.

```sh
git clone https://github.com/eigenergy/powerio
cd powerio
cargo build -p powerio-capi --release --features arrow,matrix,gridfm,dist,prob
# → target/release/libpowerio_capi.{so,dylib}, header powerio-capi/include/powerio.h
```

## Parse, inspect, write

Julia:

```julia
using PowerIO
case = parse_file("case9.m")
case isa PioModule{BalancedNetwork}   # true: the kind was detected
n_buses(case.value)                   # 9
diagnostics(case)                     # the reader's findings, usually empty here
write_file(case, "copy.m")            # same format: byte exact echo
text, findings = to_format(case.value, "psse")  # cross format: text + reported losses
```

Python:

```python
import powerio
case = powerio.parse("case9.m")
case.kind                       # "balanced_network"
net = case.value                # BalancedNetwork
case.diagnostics()              # native records: code, severity, message, spans
```

Rust:

```rust,ignore
use powerio::Source;

let module = powerio::parse(Source::open("case9.m")?)?;
let case: powerio::PioModule<powerio::BalancedNetwork> =
    powerio::try_into_typed(module)?;
let (text, findings) = powerio::write_module_str(
    &case.map_value(powerio::PioValue::from), "matpower")?;
```

Command line:

```sh
powerio convert case9.m --to psse -o case9.raw
powerio summary case9.m               # the canonical network summary JSON
```

## Where next

- The value families and what each source parses to: [Core Concepts](concepts.md).
- Balanced transmission work: [Transmission Networks](transmission.md). Conductor level distribution work: [Distribution Networks](distribution.md).
- Matrices, signs, units, and row identities: [Matrices and Graphs](matrices.md).
- Each format's supported profile and write behavior: [Formats and Fidelity](format-fidelity.md).
