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

if isinstance(module.value, powerio.BalancedNetwork):
    print(module.value.n_buses)

for diagnostic in module.diagnostics:
    print(diagnostic.code, diagnostic.severity, diagnostic.message)
```

Diagnostics live on the module rather than on the contained network or
solution. To find out what you have, use Python's normal type system; there is
no `.kind` property, kind enum, or typed narrowing helper.

The value classes are `BalancedNetwork`, `dist.MulticonductorNetwork`,
`OperatingPoint`, `TimeSeries`, `ScenarioSet`, `GeoLayer`, the PF, OPF, and
SCUC instances and solutions, and `SocwrOpfSolution`.

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

`calc_admittance_matrix`, `calc_bprime_matrix`, `calc_ptdf`, `calc_lodf`,
`to_normalized`, and `to_networkx` are methods of the same class. SciPy is
imported only when you ask for a sparse matrix, NumPy only for the array based
helpers, and NetworkX only inside `to_networkx`.

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

Filesystem access is off unless `POWERIO_MCP_ALLOWED_ROOTS` lists the
directories the server may read, and remote URI schemes are rejected. Host
approval, request identifiers, timeouts, and cancellation are MCP transport
concerns and do not touch the PowerIO data.
