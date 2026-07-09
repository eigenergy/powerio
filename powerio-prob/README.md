# powerio-prob

`powerio-prob` builds complete numerical problem instances from PowerIO
networks. An instance contains the indexed coefficients, bounds, mappings,
units, and conventions needed to formulate a problem. It is distinct from the
source network, a matrix projection, a solver formulation, and a solution.

The default build has no sparse matrix dependency. It provides an index based
`DcOpfInstance` whose generator and branch arrays can be consumed directly by
an operations research model. It also provides a matrix free `ScopfInstance`
for GOC3 data. Enable the `matrix` feature to derive sparse DC OPF operators
from a `DcOpfInstance`.

```rust
use powerio::{IndexedNetwork, parse_matpower_file};
use powerio_prob::{DcOpfOptions, build_dc_opf_instance};

let net = parse_matpower_file("case14.m")?;
let view = IndexedNetwork::new(&net);
let problem = build_dc_opf_instance(&view, &DcOpfOptions::default())?;
assert_eq!(problem.n_buses, view.n());
# Ok::<(), powerio::Error>(())
```
