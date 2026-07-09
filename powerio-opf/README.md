# powerio-opf

`powerio-opf` builds OPF instances — input data for one class of optimal power
flow, with no solver assumptions baked in — from a parsed `powerio` network. An
instance is input data: a consumer reads it and builds its own formulation and
solver on top.

The instances are index based and carry no matrices, so the crate depends only
on `powerio`. The generator→bus map is the `bus_of_col` index vector rather than
a sparse `C_g`. `powerio-matrix` keeps the graphical DC-OPF builder — its
`OpfInstance`, the sparse `C_g`, and the Matrix Market bundle writer — for
consumers that want that form. The two crates are independent siblings on
`powerio`.

```rust
use powerio::{IndexedNetwork, parse_matpower_file};
use powerio_opf::{Units, build_dc_opf_instance};

let net = parse_matpower_file("case14.m")?;
let view = IndexedNetwork::new(&net);
let opf = build_dc_opf_instance(&view, Units::PerUnit)?;
assert_eq!(opf.bus.q.len(), view.n());
```

`DcOpfInstance` is shipped: bus-indexed cost, bounds, thermal limits, the
`bus_of_col` generator→bus index map, and nodal load. `AcOpfInstance` is a field
skeleton with no builder yet. `ScopfInstance` is not started
(eigenergy/powerio#235); PowerIO.jl keeps building it as a Julia-side projection
until the Rust IR can represent reserves, contingencies, and cross-period energy
budgets. The workspace README has the full overview:
<https://github.com/eigenergy/powerio>.
