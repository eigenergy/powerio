# powerio-prob

A problem instance is the complete numerical input for one problem family. It
contains indexed coefficients, bounds, mappings, units, and conventions. It is
separate from the source network, matrix projections, solver formulations, and
solutions.

`powerio-prob`, short for problem instance builders, assembles these instances
from PowerIO models.

The default build has no sparse matrix dependency. It provides index based
`DcOpfInstance` and `AcOpfInstance` data whose bus, generator, and branch
arrays can be consumed directly by an operations research model, and a matrix
free `ScopfInstance` for GOC3 data. Relaxations of AC OPF, the SOC forms
included, consume the same `AcOpfInstance`. Enable the `matrix` feature to
derive sparse DC OPF operators from a `DcOpfInstance`.

```rust
use powerio::{IndexedNetwork, parse_matpower_file};
use powerio_prob::{DcOpfOptions, build_dc_opf_instance};

let net = parse_matpower_file("case14.m")?;
let view = IndexedNetwork::new(&net);
let problem = build_dc_opf_instance(&view, &DcOpfOptions::default())?;
assert_eq!(problem.n_buses, view.n());
# Ok::<(), powerio::Error>(())
```
