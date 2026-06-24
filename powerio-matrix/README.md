# powerio-matrix

`powerio-matrix` builds sparse matrices and graph outputs from `powerio::Network`.
It re-exports the `powerio` data layer so one import covers parsing and matrix
construction.

```rust
use powerio_matrix::{BuildOptions, IndexedNetwork, build_bprime, parse_matpower_file};

let net = parse_matpower_file("case14.m")?;
let view = IndexedNetwork::new(&net);
let bprime = build_bprime(&view, &BuildOptions::default())?;
```

Outputs include B', B'', Y_bus components, LACPF, incidence, weighted
Laplacians, PTDF, LODF, DC OPF bundles, adjacency, and a petgraph graph. The
workspace README has the full format and validation overview:
<https://github.com/eigenergy/powerio>.
