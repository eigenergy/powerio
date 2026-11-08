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
