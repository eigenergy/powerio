# MATPOWER AC/DC IO qualification

This harness qualifies declared data and fresh output, not a coupled solver.
`Manifest.toml` pins Julia 1.12.6 dependencies and PowerModelsACDC commit
`9ada4082ac087aed649f48fb5f977680b0fe06c7`. No Julia dependency is added to PowerIO.

From the repository root:

```sh
julia --project=evals/validation/acdc -e 'using Pkg; Pkg.instantiate()'
cargo run -p powerio --example acdc_validation -- tests/data/pglib-hvdc/case5_3_he.m /tmp/powerio-acdc-oracle
julia --project=evals/validation/acdc evals/validation/acdc/compare.jl tests/data/pglib-hvdc/case5_3_he.m /tmp/powerio-acdc-oracle
```

The comparison checks every DC-bus, branch and converter declaration against
PowerModels' parser, including PCC setpoint signs and valve-side limits, and
compares the fresh file after IR restoration. PGLib aliases are explicitly
renamed in the reference dictionaries before PowerModelsACDC preprocessing;
source files and native declarations are never corrected in place.
`reference.json` records source and fresh SHA-256 hashes and both the source
and effective reference dictionaries. On case5 the reference enlarges `Imax`;
PowerIO preserves the input limits. These snapshots are generated artifacts,
not checked-in solver results.

The frozen-reader proof runs independently of the current extension:

```sh
python3 -m venv /tmp/powerio-acdc-legacy
/tmp/powerio-acdc-legacy/bin/pip install powerio==0.11.0
/tmp/powerio-acdc-legacy/bin/python evals/validation/acdc/check_legacy_ir.py
# Repeat in a separate environment with powerio==0.11.9.
```

It first establishes that eight historical raw/nested network shapes load,
then reproduces silent loss of an unguarded network field, and proves that
`required_capabilities` forces rejection on each shape. Current Rust tests
exercise the real equipment, capability mismatches, malformed declarations,
COW edits, explicit subsets, and unsupported calculation paths.

The same oracle was also run on `case24_7_jb.m`, `case39_10_he.m`, and
`case67.m` from PGLib-HVDC commit
`f928f95cde06d035554d92854f03950e074850e0`. All four cases passed declaration,
fresh-output and preprocessing comparisons; case67 exercises the optional area
column. Only case5 is vendored. Download the other cases from that pinned
revision and pass each path to the two commands above to reproduce.
