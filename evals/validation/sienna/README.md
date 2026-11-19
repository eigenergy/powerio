# SiennaSchemas validation

The Rust adapter reads and writes SystemDocument releases 0.1.0, 0.2.0,
0.3.0, and 0.4.0. The validation script pins immutable upstream commits;
no generated model implementation or upstream fixture is vendored.

```sh
POWERIO_SIENNA_OUTPUT_DIR=/tmp/sienna-exports cargo test -p powerio-tx --test sienna
python3 evals/validation/sienna/validate.py /path/to/SiennaSchemas /tmp/sienna-exports
```

Each export is checked as a SystemDocument and each component is checked against
its own schema, including relative references. The versioned native tests also
exercise component-base versus natural units, separate bus numbers and component
identifiers, terminal shunts, and both placements of three-winding magnetizing
admittance. A parse followed by unchanged same-format emission is byte exact.

Optional Python tests use PyPSA's binary readers and writers and TransitionZero's
staging API. Install `powerio[pypsa-files]` and tz-oss-interop in the test environment,
then run `pytest python/tests/test_pypsa_files.py python/tests/test_interop.py`.

Native Julia serialization is checked independently of the schema documents.
The `native` CI matrix runs PowerSystems 4.6.2, 5.9.0, and 5.12.5 against
`powersystems.jl`. Each runtime produces input fixtures from MATPOWER; Rust
compares their admittance matrices against PowerNetworkMatrices and checks
bus-level power against the reference network. The 5.x consumers also load
PowerIO's freshly generated documents, whose admittance must match the original
MATPOWER network. Native imports use the native matrix as their oracle because
PowerSystems versions interpret some MATPOWER transformer charging differently.

Native generator costs are evaluated independently through PowerSystems getters
at 0, 25, and 100 MW, using each curve's declared power units. The MATPOWER
importer in the tested native releases labels its unchanged cost coefficients as
DEVICE_BASE; its resulting curve therefore differs from the original MATPOWER
curve. Imports are compared with that native curve. PowerIO exports are compared
with the original MATPOWER costs. PowerNetworkMatrices releases may use either
Float32 or Float64 internally, so admittance tolerances follow the oracle's
reported element precision.
