# powerio-matrix

`powerio-matrix` builds sparse matrices and graph data from the parsed
network families, with element mappings on every numerical result. It sits
over the component crates (`powerio-tx`, `powerio-dist`, `powerio-prob`);
the `powerio` facade re-exports it behind the `matrix` feature.

```rust
use powerio_matrix::{BuildOptions, IndexedNetwork, build_bprime};

let module = powerio::parse(powerio_core::Source::open("case14.m")?)?;
let module: powerio_core::PioModule<powerio::BalancedNetwork> =
    powerio::try_into_typed(module)?;
let view = IndexedNetwork::new(module.value());
let bprime = build_bprime(&view, &BuildOptions::default())?;
```

Outputs include MATPOWER Bp/Bpp, Y_bus components, LACPF, signed incidence,
weighted bus Laplacians, PTDF, LODF, adjacency, and a petgraph graph.
`powerio-prob` builds the calculation instances; this crate derives the
problem specific operators from them (`DcOperators::build`,
`calc_power_flow_jacobian`, the multiconductor admittance builders). See the
[workspace README](https://github.com/eigenergy/powerio) for formats and
validation commands.
