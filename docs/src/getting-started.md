# Getting started

## Install

Rust:

```sh
cargo add powerio             # parsing, emission, PowerIO IR
cargo add powerio -F matrix   # and sparse matrices, sensitivities, graph data
```

Python:

```sh
pip install powerio           # parsing, emission, PowerIO IR
pip install 'powerio[all]'    # and SciPy matrices, NetworkX graphs, Polars for GridFM
```

Julia:

```julia
using Pkg; Pkg.add("PowerIO")
```

The `powerio` command:

```sh
cargo install powerio-cli
```

For C and C++, build the shared library from a checkout and include the
checked in header.

```sh
git clone https://github.com/eigenergy/powerio
cd powerio
cargo build -p powerio-capi --release --features arrow,matrix,gridfm,dist,prob
# target/release/libpowerio_capi.{so,dylib,dll}; header powerio-capi/include/powerio.h
```

## Parse, inspect, emit

Rust. The example is a complete program; it imports only `parse`, `emit`, and
the `PioValue` enum, and `?` hands any failure to `main`.

```rust,ignore
use powerio::{PioValue, emit, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = parse("case9.m")?;
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("expected a balanced network");
    };
    println!("{} buses", network.buses().len());
    for finding in module.diagnostics() {
        eprintln!("{}: {}", finding.code(), finding.message());
    }
    emit(&module, "matpower", "copy.m")?;      // the source bytes, unchanged
    let result = emit(&module, "psse", "case9.raw")?;
    for finding in result.diagnostics() {
        eprintln!("{}", finding.code());       // what PSS/E cannot carry
    }
    Ok(())
}
```

Python:

```python
import powerio

module = powerio.parse("case9.m")
network = module.value                       # BalancedNetwork
for finding in module.diagnostics:
    print(finding.code, finding.message)
powerio.emit(module, "matpower", "copy.m")   # the source bytes, unchanged
result = powerio.emit(module, "psse")        # text in memory
result.text
result.diagnostics
```

Julia:

```julia
using PowerIO

module_ = parse("case9.m")
net = module_.value                          # BalancedNetwork
length(net.buses)                            # 9
module_.diagnostics
emit(module_, "matpower", "copy.m")          # the source bytes, unchanged
result = emit(module_, "psse")               # text in memory
result.diagnostics
```

Command line:

```sh
powerio convert case9.m --to psse -o case9.raw   # findings on stderr
powerio summary case9.m                          # counts, bases, and findings as JSON
```

## Where next

- What each source parses to and what a module contains: [Core concepts](concepts.md).
- Balanced transmission cases: [Transmission networks](transmission.md).
- Conductor level distribution cases: [Distribution networks](distribution.md).
- Matrices, signs, units, and index mappings: [Matrices and graphs](matrices.md).
- What each reader keeps and each writer reports: [Formats and fidelity](format-fidelity.md).
- The same operations in each language: [Rust, Python, Julia, and C](languages.md).
