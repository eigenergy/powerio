# Python API

Install the base package for parsing, emission, PowerIO IR, and typed values:

```bash
pip install powerio
```

Matrix and graph helpers use optional Python packages:

```bash
pip install 'powerio[matrix]'   # NumPy and SciPy
pip install 'powerio[graph]'    # NetworkX
pip install 'powerio[gridfm]'   # Polars
pip install 'powerio[all]'
```

Importing `powerio` and using `parse`, `emit`, `serialize`, or `deserialize`
does not import those optional packages.

## Parse one source

`powerio.parse` accepts a path, file object, or bytes-like object. A Python
`str` always names a path. Use `io.StringIO` for raw text. There is no Python
`Source` class: the path, file object, or bytes-like value already states
where the bytes come from, and the interpreter owns them, so `parse` takes it
directly. Rust and C build a `Source` because they need that ownership stated
explicitly; see [Rust, Python, Julia, and C](languages.md).

```python
from io import StringIO
from pathlib import Path
import powerio

case = powerio.parse(Path("case9.m"))
case_from_text = powerio.parse(
    StringIO(matpower_text), format="matpower", name="case9.m"
)
case_from_binary = powerio.parse(
    pwb_bytes, format="powerworld-pwb", name="case.pwb"
)
```

`format` is optional when the source name and content identify the format.
`name` applies only to memory and file object sources; it supplies a source
name for diagnostics and format detection.

There is no separate `parse_file`, `parse_text`, or `parse_bytes` API.

## Parse a GO Challenge 3 solution

Put the GO Challenge 3 problem and matching solution files in one directory.
The ordinary `parse` operation reads both:

```python
solution = powerio.parse("scenario_002")
assert isinstance(solution.value, powerio.AcScucSolution)
```

With only the problem file, the same call returns `AcScucInstance`. A solution
file alone fails because it does not contain the component definitions or time
axis. The solution module retains both files and its diagnostics.

## Module values and diagnostics

Parsing returns `PioModule[T]`. `module.value` is the concrete Python value
and `module.diagnostics` is the list of diagnostics stored on that module.

```python
module = powerio.parse("case9.m")

if isinstance(module.value, powerio.BalancedNetwork):
    print(module.value.n_buses)

for diagnostic in module.diagnostics:
    print(diagnostic.code, diagnostic.severity, diagnostic.message)
```

Diagnostics are fields of the module, not methods on the contained network or
solution. Python uses its normal type system; there is no `.kind` property,
kind enum, or typed narrowing helper.

Registered values include `BalancedNetwork`,
`dist.MulticonductorNetwork`, `OperatingPoint`, `TimeSeries`, `ScenarioSet`,
the PF/OPF/SCUC calculation instances and solutions, and
`SocwrOpfSolution`.

## Emit grid exchange formats

`powerio.emit` is the one operation that produces a grid exchange
representation:

```python
result = powerio.emit(module, "matpower")
text = result.text

result = powerio.emit(module, "psse", "case.raw")
result = powerio.emit(module, "pypsa", "case-directory")
```

With no destination, artifacts stay in memory. A path destination writes one
file or a directory. A writable file object accepts a single file artifact.
Every `EmitResult` carries the artifact inventory (one `Artifact` per produced
file, with its `name` and either `data` for a memory result or `path` after a
filesystem commit), the layout, the fidelity, and the emission diagnostics.
`result.text` is the one UTF-8 memory artifact when the inventory holds
exactly one, and `None` otherwise.

PowerIO IR uses separate operations:

```python
ir = powerio.serialize(module)
powerio.serialize(module, "case.pio.json")
same_module = powerio.deserialize(ir.artifacts[0].data)
```

The IR header is `"schema": "pio-ir"` and integer `"version": 2`;
`powerio.versions()["powerio_ir"]` reports both. The producer record separately
identifies `powerio.__version__`. `deserialize` refuses an unsupported identity
or generation with what it found. PowerIO IR is not a grid exchange format and
is absent from format discovery.

## Collections

`TimeSeries` follows Python sequence behavior. `ScenarioSet` follows keyed
collection behavior.

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

Entries are owner rooted typed values. Indexing does not serialize or copy a
complete network.

## Typed updates

PowerIO supplies `OperatingPointUpdate`, `NetworkUpdate`, and
`CalculationUpdate`. Each update targets a stable `ComponentId`; power values
use `ActivePower`, `ReactivePower`, or `ApparentPower` so units are explicit.

```python
report = powerio.apply_updates(
    module,
    [
        powerio.OperatingPointUpdate.set_load_active_power(
            load_id, powerio.ActivePower.mw(42.0)
        )
    ],
)

for change in report.changes:
    print(change.component_id, change.field)
print(report.connectivity_changed)
```

The complete batch is validated before mutation. A failed batch leaves the
module unchanged. `UpdateReport` lists exact changes and says whether
energized connectivity changed.

## Matrices and vectors

`BalancedNetwork` exposes verb led derived calculations:

```python
A = network.calc_incidence_matrix()
b = network.calc_branch_susceptances()
B = network.calc_bus_susceptance_matrix()
Bf = network.calc_branch_flow_matrix()
p_branch = network.calc_branch_flow_dc(voltage_angles)
p_bus = network.calc_bus_injection_dc(voltage_angles)
```

SciPy is imported only when a sparse matrix is requested. NumPy is imported
only for array based helpers. The public API has no DC data bundle.

## Errors

Parse failures raise `PowerIOParseError`. Valid data that cannot satisfy an
operation raises `PowerIODataError`. Both derive from `PowerIOError` and carry
a stable diagnostic code. Branch on `.code`, not the rendered message.

## MCP server

The optional MCP server accepts paths, in-memory grid exchange content, and
serialized PowerIO modules through the `powerio_ir` field. Electrical inputs and outputs remain
PowerIO types and PowerIO IR. The server does not define another network,
calculation, update, or solution schema.

Filesystem access is disabled unless the deployment configures allowed roots.
Remote URI schemes are rejected. Host approval, request identifiers, timeout,
and cancellation handling remain MCP transport concerns and do not alter the
PowerIO data.
