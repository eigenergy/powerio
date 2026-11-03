# Component validation commands

These are focused checks supporting the declared reader profile. They are not
whole-case acceptance claims. Paths under `/tmp` or `/external` are placeholders;
retain the acquisition steps and use source/record hashes in the adjacent reports.
Scripts define their positional arguments and dependencies in their source.
Historical exploratory outputs are not automatically current acceptance evidence.
Numerical scripts require NumPy, SciPy, OpenDSSDirect.py and/or pandapower;
consult each recorded report for the tested versions. For recipes using shell
variables, set external input and scratch output directories first:

```sh
sources=/external/native-models
records=/external/acquired-records
output=/tmp/sincal-checks
mkdir -p "$output"
```

## Conductor-resolved reader development

```sh
cargo build -p powerio-dist --example sincal_multiconductor
# Complete mapping: rejects the entire network when any required mode fails.
target/debug/examples/sincal_multiconductor records read /tmp/records.json 1
# Diagnostic audit: component reports are not successful network parses.
target/debug/examples/sincal_multiconductor records audit /tmp/records.json 1
```

```sh
python3 evals/sincal/audit_distribution.py /tmp/native-models /tmp/acquired-records \
  /tmp/distribution-csiro.json --reader target/debug/examples/sincal_multiconductor
```

## Finite source zero sequence

```sh
POWERIO_SINCAL_SOURCE_ORACLE_DIR=/tmp/source-oracle cargo test -p powerio-dist \
  sincal::legacy_tests::export_source_zero_sequence_oracle --lib -- --ignored
python3 evals/sincal/check_source_zero_sequence.py /tmp/source-oracle \
  --report /tmp/source-zero-sequence.json
```

## Native phase-pair loads

```sh
POWERIO_SINCAL_LOAD_RECORDS=/tmp/acquired-records/representative06.json \
POWERIO_SINCAL_LOAD_EXPORT=/tmp/csiro06-load-components.json \
  cargo test -p powerio-dist --lib \
  sincal::load_mapping_tests::export_csiro_phase_pair_loads -- --ignored
python3 evals/sincal/check_phase_pair_loads.py \
  /tmp/acquired-records/representative06.json /tmp/native-models/csiro-representative06.mdb \
  /tmp/csiro06-load-components.json --report /tmp/load-phase-pairs.json
```

## Explicit daily load snapshots

```sh
POWERIO_SINCAL_PROFILE_RECORDS=/tmp/acquired-records \
POWERIO_SINCAL_PROFILE_EXPORT=/tmp/profile-components.json \
  cargo test -p powerio-dist --lib \
  sincal::load_profile_tests::export_csiro_daily_loads -- --ignored
python3 evals/sincal/check_daily_profiles.py /tmp/native-models /tmp/acquired-records \
  /tmp/profile-components.json --report /tmp/load-daily-profiles.json
cargo build -p powerio-dist --example sincal_multiconductor
python3 evals/sincal/audit_distribution.py /tmp/native-models /tmp/acquired-records \
  /tmp/distribution-csiro-profiles.json --reader target/debug/examples/sincal_multiconductor \
  --snapshot-hours 0
```

## Ideal connection lines

```sh
POWERIO_SINCAL_CONNECTION_RECORDS=/tmp/acquired-records/representative09.json \
POWERIO_SINCAL_CONNECTION_EXPORT=/tmp/connection-components.json \
  cargo test -p powerio-dist --lib \
  sincal::connection_tests::export_csiro_connections -- --ignored
python3 evals/sincal/check_connections.py /tmp/native-models/csiro-representative09.mdb \
  /tmp/acquired-records/representative09.json /tmp/connection-components.json \
  --report /tmp/connections-csiro09.json
```

## Ungrounded Y0 autotransformers

```sh
POWERIO_SINCAL_AUTO_EXPORT=/tmp/y0-primitives.json cargo test -p powerio-dist --lib \
  sincal::autotransformer_tests::export_y0_primitives -- --ignored
POWERIO_SINCAL_AUTO_RECORDS=/tmp/acquired-records/representative01.json \
POWERIO_SINCAL_AUTO_NATIVE_EXPORT=/tmp/y0-native.json cargo test -p powerio-dist --lib \
  sincal::autotransformer_tests::export_native_neutral_y0 -- --ignored
python3 evals/sincal/check_autotransformers.py /tmp/y0-primitives.json \
  /tmp/acquired-records/representative01.json /tmp/native-models/csiro-representative01.mdb \
  /tmp/y0-native.json --report /tmp/autotransformers.json
```

## Schema-12.8 LPC component compatibility

```sh
python3 evals/sincal/import_access.py /tmp/matlab-lpc-eu.mdb /tmp/lpc-eu-records.json
POWERIO_SINCAL_LPC_RECORDS=/tmp/lpc-eu-records.json \
POWERIO_SINCAL_LPC_EXPORT=/tmp/lpc-eu-components.json cargo test -p powerio-dist --lib \
  export_lpc_european_components -- --ignored
python3 evals/sincal/check_lpc_components.py /tmp/lpc-eu-records.json \
  /tmp/matlab-lpc-eu.mdb /tmp/lpc-eu-components.json --report /tmp/lpc-components.json
```

## Complete CSIRO09 snapshots and asymmetric stress validation

```sh
cargo build -p powerio-dist --example sincal_multiconductor
python3 evals/sincal/check_csiro09_corpus.py \
  /tmp/acquired-records/representative09.json \
  /tmp/native-models/csiro-representative09.mdb \
  target/debug/examples/sincal_multiconductor /tmp/csiro09-network.json
```

## Public multiconductor facade validation

```sh
cargo build -p powerio --example sincal_public
POWERIO_MAX_PRIMARY_BYTES=69181440 python3 evals/sincal/check_csiro09_corpus.py \
  /tmp/acquired-records/representative09.json \
  /tmp/native-models/csiro-representative09.mdb \
  target/debug/examples/sincal_public /tmp/csiro09-public.json --public-reader
```

## Python and CLI selection validation

```sh
POWERIO_MAX_PRIMARY_BYTES=69181440 python3 evals/sincal/check_public_bindings.py \
  /tmp/models/representative09.mdb /tmp/records/representative09.json \
  target/debug/powerio /tmp/csiro09-bindings.json --acquisition-root /tmp
```

## C and Julia selection validation

```sh
POWERIO_MAX_PRIMARY_BYTES=69181440 POWERIO_CAPI=/path/to/libpowerio_capi.dylib \
  julia --project=/path/to/PowerIO.jl evals/sincal/check_julia_bindings.jl \
  /tmp/models/representative09.mdb /tmp/records/representative09.json \
  target/debug/powerio /tmp /tmp/csiro09-julia-bindings.json
```

## Partial delta–delta transformer validation

```sh
POWERIO_SINCAL_PARTIAL_RECORDS=/tmp/records/representative06.json \
POWERIO_SINCAL_PARTIAL_EXPORT=/tmp/partial-delta-export.json \
  cargo test -p powerio-dist --lib export_partial_delta_circuits -- --ignored
python3 evals/sincal/check_partial_delta.py /tmp/partial-delta-export.json \
  /tmp/records/representative06.json /tmp/models/csiro-representative06.mdb \
  /tmp/partial-delta.json --case 6
```

## CSIRO06 remaining-input audit

```sh
cargo build -p powerio-dist --example sincal_multiconductor
python3 evals/sincal/check_csiro06_blockers.py \
  /tmp/models/csiro-representative06.mdb /tmp/records/representative06.json \
  target/debug/examples/sincal_multiconductor /tmp/csiro06-blockers.json
```

## Rated reactor and capacitor banks

```sh
cargo build -p powerio-dist --example sincal_multiconductor
python3 evals/sincal/acquire_shunt_corpus.py /tmp/models /tmp/records \
  /tmp/shunt-records target/debug/examples/sincal_multiconductor \
  /tmp/shunt-acquisition.json
POWERIO_SINCAL_SHUNT_RECORDS_DIR=/tmp/shunt-records \
POWERIO_SINCAL_SHUNT_EXPORT=/tmp/rated-shunt-circuits.json \
  cargo test -p powerio-dist --lib export_rated_shunt_circuits -- --ignored
python3 evals/sincal/check_rated_shunts.py /tmp/rated-shunt-circuits.json \
  /tmp/models /tmp/shunt-records /tmp/records /tmp/rated-shunts.json
python3 evals/sincal/check_shunt_history.py /tmp/models/csiro-representative03.mdb \
  /tmp/rated-shunt-circuits.json /tmp/rated-shunt-history.json
```

## Legacy line temperatures

```sh
POWERIO_SINCAL_TEMPERATURE_RECORDS=/tmp/records \
POWERIO_SINCAL_TEMPERATURE_EXPORT=/tmp/line-temperature-circuits.json \
  cargo test -p powerio-dist --lib export_legacy_temperature_lines -- --ignored
python3 evals/sincal/check_legacy_temperatures.py /tmp/models /tmp/records \
  /tmp/line-temperature-circuits.json /tmp/legacy-temperatures.json
```

## Sparse legacy line inputs

```sh
POWERIO_SINCAL_SPARSE_LINE_RECORDS=/tmp/records \
POWERIO_SINCAL_SPARSE_LINE_EXPORT=/tmp/sparse-line-circuits.json \
  cargo test -p powerio-dist --lib export_sparse_legacy_lines -- --ignored
python3 evals/sincal/check_sparse_lines.py /tmp/models /tmp/records \
  /tmp/sparse-line-circuits.json /tmp/sparse-lines.json
```

## Materialized load manipulators: CSIRO05

```sh
POWERIO_SINCAL_MANIPULATOR_RECORDS="$records/representative05.json" \
POWERIO_SINCAL_MANIPULATOR_EXPORT="$output/csiro05-loads.json" \
cargo test -p powerio-dist --lib export_csiro05_materialized_loads -- --ignored
python evals/sincal/check_materialized_loads.py \
  "$sources/csiro-representative05.mdb" "$records/representative05.json" \
  "$output/csiro05-loads.json" --report "$output/materialized-loads.json"
```

## Partial mixed-winding investigation and fixed tap-status defaults

```sh
python evals/sincal/audit_partial_mixed_history.py \
  "$sources/csiro-representative06.mdb" "$records/representative06.json" \
  "$output/partial-mixed-history.json"
```

## Relative daily profiles with unequal branch powers

```sh
POWERIO_SINCAL_RELATIVE_EXPORT="$output/relative-profiles.json" \
cargo test -p powerio-dist --lib export_relative_daily_profiles -- --ignored
python evals/sincal/check_relative_profiles.py \
  "$output/relative-profiles.json" "$output/relative-profile-check.json"
```

## Derived-variant structural evidence

```sh
python3 -m unittest discover -s evals/sincal -p 'test_variant_audit.py'
python3 evals/sincal/audit_variants.py "$sources" "$base_records" \
  "$output/variant-inheritance.json"
```

## Balanced Access selection through the CLI

```sh
cargo build -p powerio-cli
python evals/sincal/check_balanced_cli.py target/debug/powerio \
  /external/source/csiro-representative19.mdb /external/csiro19-balanced-records.json \
  /external evals/sincal/balanced-csiro19-cli.json
```

