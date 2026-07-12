# powerio-matrix

`powerio-matrix` projects a `powerio::Network` into sparse matrices and graph
views. It reexports `powerio`, so the same import can cover parsing and matrix
construction.

```rust
use powerio_matrix::{BuildOptions, IndexedNetwork, build_bprime, parse_matpower_file};

let net = parse_matpower_file("case14.m")?;
let view = IndexedNetwork::new(&net);
let bprime = build_bprime(&view, &BuildOptions::default())?;
```

Outputs include MATPOWER Bp/Bpp, Y_bus components, LACPF, signed incidence,
weighted bus Laplacians, PTDF, LODF, adjacency, and a petgraph graph.
`powerio-prob` builds complete problem instances. Its optional `matrix` feature
derives problem specific operators from an instance. See the
[workspace README](https://github.com/eigenergy/powerio) for formats and
validation commands.
