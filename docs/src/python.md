# Python API

Install the base package for parsing, emission, PowerIO IR, and typed values:

```bash
pip install powerio
```

The matrix and graph helpers need optional packages, so install the extra you
want:

```bash
pip install 'powerio[matrix]'   # NumPy and SciPy
pip install 'powerio[graph]'    # NetworkX
pip install 'powerio[gridfm]'   # Polars
pip install 'powerio[all]'      # the three above
pip install 'powerio[pandas]'   # pandas and PyArrow tables, Python 3.10 or later
pip install 'powerio[mcp]'      # the MCP server, Python 3.10 or later
```

Importing `powerio` and calling `parse`, `emit`, `serialize`, or `deserialize`
does not import any of those optional packages.

## Parse one source

`powerio.parse` accepts a path, a file object, or a bytes-like object. A `str`
is always a path, so wrap raw text in `io.StringIO`. There is no Python
`Source` class, because a path, file object, or bytes-like value already says
where the bytes come from and the interpreter owns them; `parse` takes it
directly. Rust and C build a `Source` because they need that ownership made
explicit, as [Rust, Python, Julia, and C](languages.md) explains.

```python
from io import StringIO
from pathlib import Path
import powerio

case = powerio.parse(Path("case9.m"))
case_from_text = powerio.parse(
    StringIO(matpower_text), format="matpower", name="case9.m"
)
case_from_binary = powerio.parse(
    pwb_bytes, format="pwb", name="case.pwb"
)
```

`format` is optional when the source name and content identify the format.
`name` applies only to memory and file object sources, where it supplies a
source name for diagnostics and format detection. There is no separate
`parse_file`, `parse_text`, or `parse_bytes` API.

## Parse a GO Challenge 3 solution

Put the GO Challenge 3 problem file and its matching solution file in one
directory, and the ordinary `parse` call reads both:

```python
solution = powerio.parse("scenario_002")
assert isinstance(solution.value, powerio.AcScucSolution)
```

With only the problem file, the same call returns `AcScucInstance`. A solution
file on its own fails, because it has neither the component definitions nor the
time axis. The solution module keeps both files and its diagnostics.

## Module values and diagnostics

Parsing returns a `PioModule[T]`, where `module.value` is the concrete Python
value and `module.diagnostics` is the list of diagnostics stored on that
module.

```python
module = powerio.parse("case9.m")
print(module.type_name)                      # powerio.BalancedNetwork

if isinstance(module.value, powerio.BalancedNetwork):
    print(module.value.n_buses)

for diagnostic in module.diagnostics:
    print(diagnostic.code, diagnostic.severity, diagnostic.message)

records = powerio.diagnostic_records(module.diagnostics)
```

Diagnostics live on the module rather than on the contained network or
solution. Branch on `module.value` with `isinstance`; there is no `.kind`
property, kind enum, or typed narrowing helper. `module.type_name` is the
canonical structural name of the value, the same string the C ABI and PowerIO
IR use, for messages and machine-readable results.

`powerio.diagnostic_records` turns those diagnostics into JSON-ready
dictionaries, and `powerio.diagnostic_record` turns one. Each record keeps
`code`, `severity`, `message`, and `target`, and adds `id`,
`suggested_action`, `related`, `details`, and `spans` when the diagnostic
carries them.

The value classes are `BalancedNetwork`, `dist.MulticonductorNetwork`,
`OperatingPoint`, `TimeSeries`, `ScenarioSet`, `GeoLayer`, `ContingencySet`,
`SubsystemSet`, `MonitoredSet`, the PF, OPF, and SCUC instances and solutions,
and `SocwrOpfSolution`.

## Emit grid exchange formats

`powerio.emit` is the only function that writes a grid exchange format:

```python
result = powerio.emit(module, "matpower")
text = result.text

result = powerio.emit(module, "psse", "case.raw")
result = powerio.emit(module, "pypsa", "case-directory")
```

With no destination the artifacts stay in memory; a path destination writes one
file or a directory, and a writable file object accepts a single file artifact.
An `EmitResult` has the artifacts (one `Artifact` per produced file, with its
`name` and either `data` for a memory result or `path` after a filesystem
commit), the layout, the fidelity, and the emission diagnostics. `result.text`
is the UTF-8 memory artifact when there is a single one, and `None` otherwise.

PowerIO IR has its own pair of functions:

```python
ir = powerio.serialize(module)
powerio.serialize(module, "case.pio.json")
same_module = powerio.deserialize(ir.artifacts[0].data)
```

The IR header is `"schema": "pio-ir"` with the integer `"version": 2`, and
`powerio.versions()["powerio_ir"]` reports both. The producer record gives
`powerio.__version__` separately. `deserialize` refuses a document whose schema
or version it does not support and reports what it found. PowerIO IR is not a
grid exchange format, so it does not appear in format discovery.

## PSS/E contingency analysis files

A `.con`, `.sub`, or `.mon` file parses to `ContingencySet`, `SubsystemSet`,
or `MonitoredSet`. Each has a `text` property holding the file. A contingency
set and a subsystem set also reach a network through a `BalancedNetwork`
method.

```python
case = powerio.parse("case.raw").value
cases = powerio.parse("cases.con").value
print(cases.text.splitlines()[0])

resolution = case.resolve_contingencies(cases.text)
print(resolution["resolved"], "of", resolution["cases"], "cases bound")
for result in resolution["case_results"]:
    if not result["resolved"]:
        print(result["name"], result["unresolved"][0]["reason"])

groups = powerio.parse("groups.sub").value
expanded, notes = case.expand_contingencies(cases.text, groups.text)
print(expanded)
print(case.select_subsystem_buses(groups.text, "A1"))
```

`resolve_contingencies` reports rather than refuses: a case naming an element
the network does not hold is counted unresolved and listed in `case_results`
with the reason each action did not bind. Each reason is a fixed name such as
`no_such_branch`; C reports the same names.

Each element a case bound to states its `type`, which names the table, the
`row` it occupies there, the element's own `in_service` flag, and its `id`.
`id` is `None` when the network states no identity for that row, and `type`
and `row` name the element either way.

`expand_contingencies` turns an automatic specification such as `SINGLE
BRANCH IN SUBSYSTEM 'A1'` into one explicit case per element, and returns the
expanded `.con` text with the readers' and the expansion's notes.

`MonitoredSet` is text only in Python. It carries the file as its `text`
property and has no method that binds it to a network, because binding a
monitored element file to a network and a subsystem set is a Rust operation.
C reads a monitored set's statement count with
`pio_monitored_set_statement_count` and its text with
`pio_monitored_set_to_mon`.

## Collections

`TimeSeries` behaves like a Python sequence and `ScenarioSet` like a mapping:

```python
series = module.value
first = series[0]
for value in series:
    use(value)

scenarios = scenario_module.value
base = scenarios["base"]
for scenario_id in scenarios:
    use(scenario_id, scenarios[scenario_id])
```

Entries are owner rooted typed values, so indexing does not serialize or copy a
complete network.

## Typed updates

PowerIO supplies `OperatingPointUpdate`, `NetworkUpdate`, and
`CalculationUpdate`. Each update targets a stable `ComponentId`, and power
values use `ActivePower`, `ReactivePower`, or `ApparentPower` so the unit is
explicit.

```python
report = powerio.apply_updates(
    module,
    [
        powerio.OperatingPointUpdate.set_load_active_power(
            load_id, powerio.ActivePower.megawatts(42.0)
        )
    ],
)

for change in report.changes:
    print(change.component_id, change.field)
print(report.connectivity_changed)
```

The whole batch is validated before anything is mutated, so a failed batch
leaves the module unchanged. The `UpdateReport` lists each change and says
whether energized connectivity changed.

## Matrices and vectors

The derived calculations are `calc_*` methods on `BalancedNetwork`:

```python
A = network.calc_incidence_matrix()
b = network.calc_branch_susceptances()
B = network.calc_bus_susceptance_matrix()
Bf = network.calc_branch_flow_matrix()
p_branch = network.calc_branch_flow_dc(voltage_angles)
p_bus = network.calc_bus_injection_dc(voltage_angles)
```

Every DC calculation shares two axes. `calc_dc_index_map` names them:
`bus_ids` maps a bus row to the source bus id (every bus in table order),
`branch_rows` maps a branch row to its position in the branch table (three
winding transformer windings follow the branches), and `branch_ids` gives the
stable identity of that row, the branch uid when the source states one and
`branches:<row>` otherwise. Out of service branches and self loops have no
row. A zero impedance branch fails a DC calculation with
`BUILD.OPERATOR.ZERO_IMPEDANCE`; `skip_zero_impedance=True` drops it and
`calc_dc_index_map` lists it under `skipped_branch_rows`.

```python
axes = network.calc_dc_index_map(skip_zero_impedance=True)
A = network.calc_incidence_matrix(skip_zero_impedance=True)
assert A.shape == (len(axes["branch_ids"]), len(axes["bus_ids"]))
```

`calc_admittance_matrix`, `calc_bprime_matrix`, `calc_ptdf`, `calc_lodf`,
`to_normalized`, and `to_networkx` are methods of the same class. SciPy is
imported only when you ask for a sparse matrix, NumPy only for the array based
helpers, and NetworkX only inside `to_networkx`.

An `OperatingPoint` entry of a `TimeSeries` or `ScenarioSet` exposes
`.network`, the balanced network with that point's values applied, so a solver
receives the entry without emitting and reparsing it. The property returns the
network alone: net bus injection quantities have no balanced network field, so
they are dropped and the property reports nothing. `emit` states that same
omission as `EMIT.OPERATING_POINT.DATA_OMITTED`, so emit the collection when
you need the diagnostic.

## Other functions

| Function | Result |
|---|---|
| `resolve_format(name)` | the canonical `FormatInfo` for a token or alias, or `None` |
| `features()` | which build features the installed extension carries |
| `versions()` | the release, the PowerIO IR identity, and the BMOPF schema version |
| `parse_geo(text, name_hint=None)` | a geographic layer in canonical form with its diagnostics |
| `parse_display(path, format=None)` | the raw PowerWorld `.pwd` display record as `DisplayData` |
| `from_ppc(ppc)` | a `BalancedNetwork` from a pandapower or PYPOWER case dictionary |
| `PioModule.from_value(value)` | a module around a value built in Python |
| `diagnostic_record(diagnostic)` | one diagnostic as a JSON-ready dictionary |
| `diagnostic_records(diagnostics)` | every diagnostic as a JSON-ready dictionary, in order |
| `module.to_balanced_report()`, `module.to_balanced()` | the multiconductor to balanced transformation |

## Errors

A parse failure raises `PowerIOParseError`, and valid data that cannot satisfy
an operation raises `PowerIODataError`. Both derive from `PowerIOError` and
have a stable diagnostic code, so branch on `.code` rather than on the rendered
message. A Rust panic inside the extension raises `PowerIOError` with code
`BIND.PY.PANIC` instead of `pyo3_runtime.PanicException`, and the module is
left unchanged, because each mutation is built in full before it is installed.

## MCP server

The optional MCP server accepts paths, grid exchange content held in memory,
and serialized PowerIO modules through the `powerio_ir` field. Electrical
inputs and outputs stay PowerIO types and PowerIO IR; the server does not
define another network, calculation, update, or solution schema.

Filesystem reads and writes default to the directory captured at server startup.
`POWERIO_MCP_ALLOWED_ROOTS` selects explicit directories instead; the compatibility
settings `POWERIO_MCP_ROOT` and `POWERIO_MCP_ALLOWED_ROOT` follow it in precedence.
Remote URI schemes are rejected. Host
approval, request identifiers, timeouts, and cancellation are MCP transport
concerns and do not touch the PowerIO data.
