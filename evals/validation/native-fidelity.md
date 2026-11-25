# Native conversion fidelity pilot

The pilot measures two native format pairs from [#957](https://github.com/eigenergy/powerio/issues/957).
The inputs are the vendored MATPOWER case14 and the official IEEE 13-bus
OpenDSS project. The scorecard pins SHA-256 digests for the MATPOWER case,
the OpenDSS deck, its redirected line codes, and its bus coordinates. The
fixture READMEs record upstream provenance and license terms.

## Reproduce the scorecard

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

The terminal scorecard shows passes and verified checks over the fixed
applicable count. A measured failure counts toward coverage. A missing
check stays in the denominator. The JSON records each check, matrix
diagnostics and model differences, numerical residuals, and the tested
Git revision, fixture hashes, and oracle versions. The checked-in baseline
uses `--compact` to omit the detailed matrix diagnostics. To regenerate it,
run the same command with `--compact` on the baseline's recorded revision.

The runner writes evidence even when a check observes a fidelity loss.
It exits with an error if the run leaves an applicable check missing or the
negative control fails to detect the mutation. An explicit
`--skip-independent` run reports the skipped checks and exits successfully
if it completed the requested checks.

## Check scope

- **Byte replay** compares every original file byte for byte. The OpenDSS
  emission places the files under a generated redirect entrypoint.
- **Fresh write** emits from the typed network and uses the existing
  conversion-matrix readback and electrical checks. The grouped matrix check
  covers typed core counts and totals, plus AC admittance and connected
  bus injections for MATPOWER or the source constraints and physical
  component projections for OpenDSS. The JSON also lists source, write,
  and readback findings and observed typed-model changes.
- **Independent validation** reads the source and fresh MATPOWER files with
  CaseFrames and reuses the MATPOWER `makeBdc` and `makePTDF` transcriptions
  from `validate_dc_matpower.py`. It checks the row axes, `Bbus`, `Bf`,
  both shift injection vectors, and PTDF. Case14's original shift vectors
  are zero. For OpenDSS, it reuses the nonlinear snapshot solve from
  `validate_opendss.py` and checks the labelled node set and every node's
  voltage magnitude. Applicability and numeric comparison rules are declared
  in `check_spec`.

The baseline measures zero residual for all five MATPOWER metrics. Both
OpenDSS decks solve and have the same 41 conductors. The maximum voltage
magnitude difference is 137.631 V at `680.1`, above the 0.001 V tolerance.
The fresh-write findings include three dropped `RegControl` records.

The runner also doubles branch 1–2's reactance from 0.05917 to 0.11834 pu
in a temporary copy of the fresh case14 output. The same independent
`matpower_dc_bbus` check moves from a zero residual to 8.45023 pu,
above its 1e-9 tolerance. The control is outside the score.

## Inspect a coverage delta

```sh
.venv-native-fidelity/bin/python evals/validation/native_fidelity.py \
  --skip-independent --output target/native-fidelity-partial.json
python3 scripts/conversion-matrix-plan.py target/native-fidelity-partial.json \
  --previous target/native-fidelity.json \
  --output target/native-fidelity-coverage-delta.json
python3 -c 'import json; r=json.load(open("target/native-fidelity-coverage-delta.json")); print(*("{case}/{check}".format(**c) for c in r["changes"] if c["coverage_reduced"]), sep="\n")'
```

The comparison lists all eight independent checks with reduced coverage.
It also exposes check-level status, residual, rule, and applicability
changes and flags different corpora or oracle versions.
