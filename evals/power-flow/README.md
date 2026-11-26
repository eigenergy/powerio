# AC power flow and admittance after format conversion

[Measured results and full matrix](RESULTS.md)

For each input file, compare three files after one round trip:

| File | How it is obtained | Which tool reads it for the comparison |
| --- | --- | --- |
| Original | The reference file in its original format | A solver that reads that format |
| Converted | PowerIO reads the original and writes the selected output format | A solver that reads the output format |
| Converted back | PowerIO reads the converted file and writes the original format | The solver used for the original file |

The matrix reports all three pairwise comparisons. For example, MATPOWER
solves the original case14 and the file converted back from Egret. Egret solves
the intermediate JSON file. Agreement after converting back alone can miss an
error that affected the intermediate file. This is a numerical application of
[translation validation](https://weizmann.esploro.exlibrisgroup.com/esploro/outputs/conferenceProceeding/Translation-validation/993262143603596).
This checks a specific conversion on a specific case and operating point. Run
it after changing a reader, writer, or the intermediate representation.

There are two additional file checks. One asks PowerIO to recover the original
files exactly. The other asks PowerIO to write a new case from the grid data it
parsed, then compares the grid data after reading the new files. The second
check compares fields in the parsed data, so it can flag changed names, defaults,
or metadata even when the power flow agrees. The returned file can be formatted
differently from the original. The power flow comparison uses numerical results.

## Reference cases and tools

[corpus.json](corpus.json) lists one reference case per format or profile,
including where an input still needs to be found. The initial cases include the
vendored MATPOWER case14 and IEEE 13-bus OpenDSS files and the upstream MG-RAVENS
three-bus example. The benchmark keeps downloads and generated files under
`target/`. The report records the source file hashes, solver versions, and Git
revision. Cross-format comparisons stay within the same electrical model family.
The first snapshot is used where a format stores a series.

Independent tools include MATPOWER in GNU Octave, Egret with Ipopt, PowerModels,
PowSybl, Surge, pandapower, PyPSA, PowerFlows, OpenDSS, and
PowerModelsDistribution, PNNL GridLAB-D, LightSim2Grid, and Tellegen. PowSybl also reads PSS/E
RAW and RAWX files. PNNL GridLAB-D reads the GLM syntax it shares with Arras.
The Linux workflow uses the official GridLAB-D 5.3.0 executable. The default
local setup uses the GridLAB-D 6.0.0 Python package. Set `GRIDLABD_BIN` to
use an installed executable and record that version in the report.
The report identifies both file readers in every comparison. If a format
has no configured independent reader, PowerModels can solve a balanced
network exported by PowerIO and Tellegen can solve a multiconductor network
parsed by PowerIO. These comparisons check the resulting electrical model.
They count separately from comparisons that use independent readers for
both files. Tellegen's BMOPF reader also uses PowerIO.
LightSim2Grid uses pandapower's file reader and admittance. Its additional
comparison checks the AC power flow solution.

The MG-RAVENS example uses the nominal demand in its three load records.
Tellegen solves a copy of the parsed model with the example's load schedule
disabled for this static comparison. The results also compare its nine
physical phase nodes with the upstream OpenDSS example at the same nominal
demand. The original file and the files written for the conversion tests
retain their respective schedules.

A comparison can be unavailable because the reference file, a solver, or a
reliable way to match the buses is missing. The matrix also reports cases that
PowerIO could not convert or that the solver accepted but could not solve.
Success requires all reported quantities to be available and within their limits.

## Numerical comparisons

- Bus voltage magnitude uses a maximum absolute difference of 1e-9 per unit
  for transmission and 0.001 V for OpenDSS conductor voltages. Voltage angles
  are compared within 1e-9 radians after subtracting one common reference
  angle per comparison. For multiple islands, the angle comparison is marked
  unavailable until the solver results identify matching islands.
- Branch active and reactive power at both ends use limits of 1e-7 MW and
  1e-7 Mvar. Branches must have unambiguous endpoints. The report counts the
  branches compared and records the worst difference. Parallel branches with
  no shared identity do not count as compared.
- The admittance comparison is the relative matrix 2-norm,
  `||Y_output − Y_reference||₂ / ||Y_reference||₂`, with a limit of 1e-9.
  It uses solver matrices aligned by bus or conductor label. Per-unit
  matrices on different system MVA bases are rescaled to the reference.
  OpenDSS's matrix is its `SystemY` after solving, which includes source and
  load contributions. It is compared only with another OpenDSS `SystemY`.
  PowSybl's matrix is assembled from the solver's internal network dump.
  The harness checks the branch stamps against the terminal powers PowSybl
  calculates at the solved voltages. PowerModelsDistribution's `Y0` excludes
  generator and load contributions and is converted to siemens using the
  voltage base for each node.
- Voltage limits, controls, and operating modes are part of a case. The
  benchmark records tool results and their differences. For transmission,
  tools with an option to disable reactive limit enforcement use it. The
  tools can still differ in defaults, controls and models. The results record
  those differences for investigation.

## Run the benchmark

Run from the repository root with Python 3.12. Install GNU Octave, Ipopt,
Zstandard, and Julia separately.
With Conda on macOS, set `OCTAVE_HOME` to the Octave environment directory if
Octave needs it. The Surge wheel on macOS also needs SuiteSparse libraries.

```sh
cargo build --locked -p powerio-cli
python3.12 -m venv .venv-power-flow
.venv-power-flow/bin/python -m pip install -r evals/power-flow/requirements.txt
julia --startup-file=no --project=evals/validation -e 'using Pkg; Pkg.instantiate()'
julia --startup-file=no --project=powerio-dist/tools/pmd -e 'using Pkg; Pkg.instantiate()'
julia --startup-file=no --project=target/pf-julia-powersystems -e 'using Pkg; Pkg.add([PackageSpec(name="PowerSystems", version="5.12.5"), PackageSpec(name="PowerFlows", version="0.25.5"), PackageSpec(name="JSON")])'
.venv-power-flow/bin/python evals/power-flow/prepare_corpus.py \
  --powersystems-project target/pf-julia-powersystems
export MATPOWER_PATH="$PWD/target/ac-power-flow-corpus/matpower-8.1"
bash evals/power-flow/install_matpower.sh
export PMD_PROJECT="$PWD/powerio-dist/tools/pmd"
export POWERSYSTEMS_PROJECT="$PWD/target/pf-julia-powersystems"
export TELLEGEN=/path/to/tellegen
.venv-power-flow/bin/python evals/power-flow/benchmark.py --output target/power-flow-report
.venv-power-flow/bin/python evals/power-flow/check_coverage.py target/power-flow-report/report.json
```

The GitHub Conversion Matrix workflow builds Tellegen at
`7f7551b1839695165e86183969ef918a093856ba`. To use the same version locally,
check out that commit and run `cargo build --locked -p tellegen-cli` from its
repository, then point `TELLEGEN` at the resulting executable.

Open `target/power-flow-report/matrix.md` for the matrix. The detailed
`report.json` includes the numerical differences, changed grid fields, input
hashes, and conversion warnings. `solutions.json` records the measured values.
`measured-differences.json` lists the checks outside tolerance with their
conversion diagnostics and changed grid fields.
To examine one conversion after changing PowerIO, rebuild it and run:

```sh
.venv-power-flow/bin/python evals/power-flow/benchmark.py \
  --source matpower-case14 --target egret-json \
  --output target/power-flow-after --previous target/power-flow-report/report.json
```

Use `changes.json` to see which measurements changed and which stopped
running. Since this second command selects a single cell, the comparison will
also show the cells that were left out. Use the same cases and target formats
in both runs when comparing code revisions.

Use `--merge-buses switches` or `--merge-buses exact` to test PowerIO's bus
merging during the forward balanced conversion. Use `--missing-gen-cost
quadratic --default-gen-cost 0.01,2,0` to check the generated files with that
cost policy. The report records these options. A topology change can prevent
node-by-node comparison when there is no one-to-one bus correspondence.

GitHub also adds the electrical results to the existing Conversion Matrix PR
comment. Download its evidence from the link in the comment. If a successful
run for the base commit or the previous revision of the branch has electrical
evidence, the workflow includes `changes.json`. Use the named differences and the conversion diagnostics to
pick a reader, writer, or model change to test. A second run then shows any
new differences or checks that stopped running.
