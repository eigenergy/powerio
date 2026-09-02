# Getting Started

## Install

Rust:

```sh
cargo add powerio            # parsing, emission, .pio.json
cargo add powerio -F matrix  # + sparse matrices and graph data
```

Python (parse and emit need only the interpreter; matrices pull scipy):

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

## Parse, inspect, emit

Julia:

```julia
using PowerIO
case = parse("case9.m")
case isa PioModule{BalancedNetwork}
length(case.value.buses)              # 9
case.diagnostics                      # the reader's findings, usually empty here
emit(case, "matpower", "copy.m")     # same format: byte exact echo
result = emit(case, "psse")         # another format: artifacts + diagnostics
```

Python:

```python
import powerio
case = powerio.parse("case9.m")
net = case.value                # BalancedNetwork
case.diagnostics                # native records: code, severity, message, spans
powerio.emit(case, "matpower", "copy.m")
```

Rust:

```rust,ignore
let source = powerio::Source::open("case9.m")?;
let module = powerio::parse(source, None)?;
match &module.value {
    powerio::PioValue::BalancedNetwork(network) => {
        println!("{} buses", network.buses().len());
    }
    other => println!("value type: {}", other.type_name()),
}
let result = powerio::emit(
    &module,
    "matpower",
    powerio::Destination::path("copy.m"),
)?;
for diagnostic in result.diagnostics() {
    eprintln!("{}", diagnostic.code());
}
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
