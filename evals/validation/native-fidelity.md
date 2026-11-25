# What PowerIO preserves when it rewrites two grid cases

| Input | Output | Original files copied unchanged | PowerIO wrote and checked new files | External tool result |
| --- | --- | --- | --- | --- |
| MATPOWER case14 | MATPOWER | Yes, the case file was identical | Passed 4 of 4 checks | CaseFrames and the MATPOWER formulas agreed on the bus and branch order and all five DC calculations |
| IEEE 13-bus feeder in OpenDSS | OpenDSS | Yes, all three project files were identical | Passed 4 of 4 checks | OpenDSS solved both versions on the same 41 conductors. At bus 680, phase 1, the voltage magnitude changed from 2360.459 V to 2222.828 V. The 137.631 V difference exceeded the 0.001 V limit |

These are two same-format tests for [#957](https://github.com/eigenergy/powerio/issues/957).
Tests that write either case to a different format are still open. PowerIO
reported dropping three OpenDSS voltage regulator controls (`RegControl`)
when it generated the new files.

## Run these checks again

Run from the repository root with Python 3.12 and the repository's Rust
toolchain.

```sh
python3.12 -m venv .venv-native-fidelity
.venv-native-fidelity/bin/python -m pip install -r evals/validation/native-fidelity-requirements.txt
.venv-native-fidelity/bin/python evals/validation/native_fidelity.py \
  --output target/native-fidelity.json
python3 scripts/conversion-matrix-plan.py target/native-fidelity.json \
  --previous evals/validation/native-fidelity-baseline.json \
  --output target/native-fidelity-backlog.json
```

The terminal lists how many checks passed and how many ran. Both numbers
use the same total, even if a check is skipped. The JSON records the result
and tolerance for each check, the files' SHA-256 hashes, the tested Git
revision, and the versions of the external tools. It also includes PowerIO's
warnings and the differences it found when it read its own output.

The script writes the report even when a comparison fails, as OpenDSS does
here. The exit code signals whether it collected the requested results and
whether the deliberate error was detected. `--skip-independent` explicitly
requests a run without the external tool checks.

## What the tests do

- **Copy the originals.** Ask PowerIO to return the original files it saved
  when it opened the case. Compare those files byte for byte. For OpenDSS,
  that includes the circuit file, line code definitions, and bus coordinates.
- **Write and check new files.** Read the original files with PowerIO, write
  new files from the network it parsed, and read those files back with
  PowerIO. Compare equipment counts, power totals, and the electrical
  quantities covered by the existing conversion tests. For MATPOWER these
  include AC admittance and the load and generation at each bus. The OpenDSS
  checks compare the modeled sources, lines, switches, injections, shunts,
  and transformers.
- **Use tools outside PowerIO.** For case14, read both files with CaseFrames
  and use the existing implementations of MATPOWER's `makeBdc` and
  `makePTDF`. The five calculated quantities have zero difference at a
  tolerance of 1e-9. Case14 has no nonzero branch phase shifts. For the feeder,
  solve both versions in OpenDSS and compare the voltage magnitude on every
  conductor.

## Deliberate error

The script doubles branch 1–2's reactance from 0.05917 to 0.11834 per unit
in a temporary copy of the generated case14 file. The `matpower_dc_bbus`
check measures a difference of 8.45023 per unit, up from zero, and fails.
The table above records the checks on the unmodified output.

## See which checks stopped running

```sh
.venv-native-fidelity/bin/python evals/validation/native_fidelity.py \
  --skip-independent --output target/native-fidelity-partial.json
python3 scripts/conversion-matrix-plan.py target/native-fidelity-partial.json \
  --previous target/native-fidelity.json \
  --output target/native-fidelity-coverage-delta.json
python3 -c 'import json; r=json.load(open("target/native-fidelity-coverage-delta.json")); print(*("{case}/{check}".format(**c) for c in r["changes"] if c["coverage_reduced"]), sep="\n")'
```

The comparison lists all eight external checks that ran the first time and
were skipped the second time. It also shows changes to individual results,
the rules for passing, the input files, or the versions of the tools.
