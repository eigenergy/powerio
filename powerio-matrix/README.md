# powerio-matrix

`powerio-matrix` calculates sparse matrices and graph data from parsed
networks of either family, and every numerical result comes with its element
mappings. It sits over the component crates (`powerio-tx`, `powerio-dist`,
`powerio-prob`), and the `powerio` facade re-exports it behind the `matrix`
feature.

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
`powerio-prob` builds the calculation instances, and this crate calculates
the problem specific operators from them (`DcOperators::build`,
`calc_power_flow_jacobian`, and `calc_multiconductor_admittance_matrix`). The
[workspace README](https://github.com/eigenergy/powerio) lists the formats
and validation commands.
