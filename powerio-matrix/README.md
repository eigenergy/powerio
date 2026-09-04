# powerio-matrix

`powerio-matrix` calculates sparse matrices and graph data from parsed
network families, with element mappings on every numerical result. It sits
over the component crates (`powerio-tx`, `powerio-dist`, `powerio-prob`);
the `powerio` facade re-exports it behind the `matrix` feature.

```rust
use powerio_matrix::{BuildOptions, IndexedNetwork, calc_bprime_matrix};

let module = powerio::parse("case14.m")?;
let powerio::PioValue::BalancedNetwork(network) = module.value() else {
    panic!("expected a balanced network");
};
let view = IndexedNetwork::new(network);
let bprime = calc_bprime_matrix(&view, &BuildOptions::default())?;
```

Outputs include MATPOWER Bp/Bpp, Y_bus components, LACPF, signed incidence,
weighted bus Laplacians, PTDF, LODF, adjacency, and a petgraph graph.
`powerio-prob` builds the calculation instances; this crate calculates the
problem specific operators from them (`DcOperators::build`,
`calc_power_flow_jacobian`, and `calc_multiconductor_admittance_matrix`). See the
[workspace README](https://github.com/eigenergy/powerio) for formats and
validation commands.
