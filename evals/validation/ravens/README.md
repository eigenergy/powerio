# MG-RAVENS profile validation

This gate uses the unmodified schema generator from
[`lanl-ansi/MG-RAVENS@094ad1163cb111df676884635aa03faa0538b688`](https://github.com/lanl-ansi/MG-RAVENS/tree/094ad1163cb111df676884635aa03faa0538b688),
the revision of the committed distribution example. It downloads the upstream
UML/template at CI time; those large files are not vendored here.

The distribution profile uses the generated schema without alterations. The
balanced profile follows upstream `ravens/parsers/mpc2ravens.py`, whose output
contains two fields missing from its own UML/template: inline
`ConnectivityNode.SvVoltage` and the root `MySettings` base-MVA setting.
`balanced_profile.py` supplies only these two closed, typed shapes. It does not
disable `additionalProperties`, remove emitted information, or relax the
distribution schema. The gate runs the native MATPOWER converter on case9,
proves the discrepancy is exactly those fields, and verifies that malformed
voltages and unknown fields still fail. Balanced results are labelled
converter-profile conformance, not unmodified upstream schema conformance.

Every committed micro OpenDSS case, PMD ENGINEERING and MATHEMATICAL fixture,
distribution RAVENS example, and BMOPF example is accounted for, alongside
MATPOWER cases 9, 14, 30, 57, and 118. Every time-series snapshot is checked.
The archived ENWL example has a known source-grounding error; its precise
refusal is asserted and reported separately. No emitted document is exempted
because its conversion reports losses. Each conversion passes through PowerIO
IR first, removing retained source bytes so the writer cannot pass by echoing
the input. Unknown-class negative controls run on each emitted document.

The Markdown artifact names each case and profile, records every diagnostic
and failure, and distinguishes refusal from successful output validation.
Schema validity checks structure and vocabulary; electrical equivalence is
covered by the conversion matrix and the native numerical oracles.

From the repository root, after checking out the pinned upstream revision:

```sh
python3 -m venv .venv-ravens
.venv-ravens/bin/python -m pip install /path/to/MG-RAVENS -r evals/validation/ravens/requirements.txt
cargo build --locked -p powerio-cli
.venv-ravens/bin/python evals/validation/ravens/generate_schema.py /path/to/MG-RAVENS target/ravens-upstream-schema.json
.venv-ravens/bin/python evals/validation/ravens/check.py \
  --powerio target/debug/powerio --upstream /path/to/MG-RAVENS \
  --schema target/ravens-upstream-schema.json --report target/ravens-schema-report.md
```

Schema generation imports the native upstream package, including its large
colour lookup table, and can use several GB of RAM. Generation runs in a
separate process so this memory is released before conversion checks.
