# Migrating to 0.7

DC OPF problem data moved from `powerio-matrix` to `powerio-prob`.
`powerio-matrix` now owns generic network projections. It does not depend on or
reexport `powerio-prob`.

Replace these 0.6 imports:

```rust,ignore
use powerio_matrix::{OpfInstance, Units, build_opf_instance};
use powerio_matrix::{DcOpfOptions, write_dcopf_bundle};
```

with:

```rust,ignore
use powerio_prob::{DcOpfInstance, DcOpfOptions, Units, build_dc_opf_instance};
use powerio_prob::matrix::{DcOpfBundleOptions, write_dcopf_bundle};
```

`DcOpfInstance` stores generators in generator space. It does not sum cost
coefficients for several generators at one bus. Call
`DcOpfInstance::nodal_generator_data` only when a bus space formulation is
required; it returns an error if the reduction would change the objective.

The default `powerio-prob` feature set contains no sparse matrix dependency.
Enable `matrix` for incidence, Laplacian, flow, generator map, bundle, and KKT
operators:

```toml
[dependencies]
powerio-prob = { version = "0.7", features = ["matrix"] }
```

`powerio_prob::matrix::write_dcopf_bundle` accepts an assembled instance. Cost
policy handling happens before assembly. The writer does not read the source
network again.
