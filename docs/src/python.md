# Python API

Install the base package for parsing, output, JSON transport, and file conversion. It has no required third party Python packages:

```bash
pip install powerio
```

Install extras only for the outputs that need them:

```bash
pip install 'powerio[matrix]'   # numpy, scipy
pip install 'powerio[graph]'    # networkx
pip install 'powerio[gridfm]'   # polars
pip install 'powerio[pandas]'   # pandas and pyarrow compatibility reads (Python 3.10+)
pip install 'powerio[all]'      # matrix, graph, and gridfm reads
```

`import powerio`, `parse_file`, `PioModule.emit`, and JSON transport do not import NumPy, SciPy, NetworkX, Polars, pandas, or pyarrow.

## One call parses

`powerio.parse_file(path)` reads a case file or supported case directory into a `PioModule` and detects the value kind from the extension and content. `format` forces a format name; `value_type` asserts the expected value class and raises when the source parses to something else.

```python
import powerio as pio

case = pio.parse_file("case9.m")     # PioModule, kind "balanced_network"
net = case.value                     # BalancedNetwork
case.diagnostics                     # the reader's findings, typed records
feeder = pio.parse_file("IEEE13Nodeckt.dss")  # kind "multiconductor_network"
instance = pio.parse_file("goc3_case.json")   # kind "ac_scuc_instance"
instance.inspect()                   # the operations the value supports
```

Balanced network formats accepted by `parse_file` include `matpower`, `psse`, `powerworld`, `pslf`, `powermodels-json`, `egret-json`, `pandapower-json`, and `surge-json`, plus their documented aliases. Multiconductor sources are OpenDSS (`dss`) and PMD engineering JSON. DOE GO Challenge 3 (`goc3-json`), BMOPF, and DeepMind OPFData (`opfdata-json`) produce calculation instances or solutions. Balanced model JSON is the bindings' data transport rather than a case format and has no name here: read and write it with `powerio.from_json` and `BalancedNetwork.to_json`. `opfdata-json` reads one extracted JSON document from a DeepMind OPFData FullTop or N-1 release without PyTorch and parses to its solved calculation. PyPSA CSV folders and GridFM Parquet datasets are directory formats, so `parse_file` accepts a folder path too.

When `include_root` is omitted, a file's referenced includes resolve only beneath its own containing directory; passing `include_root` widens that boundary to the named ancestor directory, and with it the set of files the parse may read.

## The module

`PioModule` carries one typed value with its records: the retained source, the reader's findings, and the descriptive history. `module.kind` names the value; `module.value` returns it. Balanced and multiconductor networks come back as the full network handles; the calculation instances, solutions, time series, and scenario sets come back as thin typed holders (`AcScucInstance`, `TimeSeries`, ...) that point back at the owning module.

```python
module = pio.parse_file("case9.m")
module.kind                        # "balanced_network"
net = module.value                 # typed handle
stored = module.emit("pio-json")   # EmitResult(text, diagnostics)
psse = module.emit("psse")         # EmitResult(text, diagnostics)
written = module.emit("psse", "case.raw")  # text is None after commit
```

`module.value` returns a `BalancedNetwork`, `MulticonductorNetwork`, collection, calculation instance, or solution according to `module.kind`. `as_balanced_network()` and `as_multiconductor_network()` remain useful when a type checker needs a narrow return type. `parse_text(text, name="module.pio.json")` reads stored `.pio.json` text; a released 0.9 document upgrades one way on read.

`module.emit(format)` selects the emitter from the value kind and returns `EmitResult(text, diagnostics)`. `module.emit(format, destination)` commits the complete artifact inventory to a file or directory and returns the same `EmitResult` shape with `text=None`. The explicit format keeps JSON destinations unambiguous. A parsed, unchanged module emitted to its source format reproduces the retained bytes.

## Findings are records

Every reader and converter reports what it could not represent as `Diagnostic` records: a stable dotted `code`, a `severity` (`error`, `warning`, `remark`, `note`), the rendered `message`, and, when stated, a `target`, a `suggested_action`, `spans` (byte ranges into the retained source, as `SourceSpan` records), `related` record ids, and an open `details` object. Branch on `code`, never on message text.

```python
for d in pio.parse_file("case.raw").diagnostics:
    print(d.code, d.severity, d.message)
    for span in d.spans:
        print("  at", span.source, span.byte_start, span.byte_end)
```

Parse failures raise `PowerIOParseError` and data failures `PowerIODataError`, both `PowerIOError` (a `ValueError`); the message opens with the finding's code.

## Networks, transformations, and output

`BalancedNetwork` is the balanced transmission value; `powerio.dist.MulticonductorNetwork` is the multiconductor distribution value. `to_*` methods transform a value in memory. `emit` writes an external representation and reports fidelity as records:

```python
module = pio.parse_file("case9.m")
net = module.value
normalized = net.to_normalized()               # in-memory transformation
psse = module.emit("psse")                     # text plus diagnostics
written = module.emit("psse", "case9.raw")    # None text plus diagnostics

feeder = pio.parse_file("IEEE13Nodeckt.dss")
report = feeder.to_balanced_report()            # inspect family conversion
if report["ready"]:
    balanced_module = feeder.to_balanced()      # explicit family conversion
```

`EmitResult` is a named tuple of optional output text and the emitter's `Diagnostic` records. Its `text` is `None` when a destination received the artifact. A parsed, unchanged module emitted to its own format echoes the retained source bytes exactly.

The network handles expose their model tables as detached Python lists of dictionaries. Mutating those copies does not change the native network. Parse findings remain on the owning `PioModule.diagnostics` field. Balanced tables include `buses`, `branches`, `generators`, `loads`, `shunts`, `switches`, `storage`, `hvdc`, `transformers_3w`, and `areas`; use `n_generators` and `n_islands` for their preferred counts. A multiconductor network exposes `buses`, `line_codes`, `lines`, `switches`, `transformers`, `loads`, `generators`, `ibrs`, `control_profiles`, `shunts`, `capacitors`, `voltage_sources`, and `untyped_objects`, with matching counts.

`net.calc_connectivity_report()` computes the balanced network's island and
isolation summary. `net.to_geo_layer()` and `feeder.to_geo_layer()` transform
stored coordinates to the shared geographic layer.

The matrix extra serves sparse matrices under verb names: `net.calc_bprime_matrix()`, `net.calc_bdoubleprime_matrix()`, `net.calc_admittance_matrix()`, `net.calc_incidence_matrix()`, `net.calc_branch_susceptance_matrix()`, `net.calc_bus_susceptance_matrix()`, `net.calc_phase_shift_injection()`, `net.calc_branch_flow_dc(va)`, `net.calc_bus_injection_dc(va)`, `net.calc_adjacency_matrix()`, `net.calc_ptdf()`, `net.calc_lodf()`, and `net.calc_lacpf_matrix()`. The `calc_*` prefix marks values computed from the network; nouns remain fields and accessors. The admittance calculation returns the complex `Y_bus` matrix directly. The DC incidence matrix `A` is branches by buses with `+1` at the from bus and `-1` at the to bus. The bus and branch matrices are `B = A.T @ diag(b) @ A` and `Bf = diag(b) @ A`; branch flow is `-Bf @ va + b * shift` and bus injection is `-B @ va + p_shift`. Select the branch susceptance formula with the `formula` argument. Graph projections use `net.to_networkx()` for balanced networks and `feeder.to_graph()` for multiconductor networks.

`net.emit_dcopf_bundle(destination)` emits the specialized Matrix Market DC
OPF bundle.

## Display artifacts

Display artifacts are drawings rather than network cases, so they use the separate display API:

```python
display = pio.parse_display_file("case.pwd")

assert display.kind == "powerworld"
first = display.data.substations[0]
print(first.number, first.name, first.x, first.y)
```

`display.data` is a `PwdDisplay` with `canvas_width`, `canvas_height`, `stamp`, and `substations`.

## Problem data

A source that defines a calculation parses to that calculation's typed value: DOE GO Challenge 3 JSON to an AC SCUC instance, BMOPF JSON to a multiconductor AC OPF instance, and OPFData JSON to a solved AC OPF. `module.inspect()` names the operations the value supports. Balanced network DC matrices and vectors come from the named operations in the previous section.

## Collections and state selection

Balanced network time series, balanced operating point time series, and balanced network scenario sets are collections. A `TimeSeries` supports `len(series)`, integer and negative indexing, and iteration; each item exports an independently usable static `PioModule`. A `ScenarioSet` supports `len(scenarios)`, `keys()`, membership, iteration over ids, and string indexing to an exported module. Export does not reparse the source. It carries the producer, source descriptions, diagnostics, history, and extensions into a fresh module, severs targets that referred to the collection value, and records the selection in history. `module.list_states()` lists the typed time points or scenario ids, `module.inspect_state(...)` describes one selected item, and `module.export_state(...)` performs the explicit export. A static or instance module refuses these operations with `REQUEST.STATE.NOT_A_COLLECTION`.

A multiconductor operating point series stays an `UnknownValue` in Python for
now: its terminal voltages, per winding tap state, and capacitor steps have no
lossless static network representation. The module remains inspectable,
storable, and emit capable; PowerIO does not invent a projection merely to make
indexing appear to work. Multiconductor networks report conversion readiness
through `module.to_balanced_report()` and transform through
`module.to_balanced()`.

```python
module = pio.parse_text(stored_series_text, name="series.pio.json")
series = module.value
static_module = series[-1]
net = static_module.as_balanced_network()
```

## PyPSA folders

PyPSA CSV folders contain several files, so emit them to a directory. The returned `EmitResult.text` is `None` and its diagnostics describe any fidelity loss.

```python
case = pio.parse_file("case14.m")
written = case.emit("pypsa-csv", "case14-pypsa")
round_trip = pio.parse_file("case14-pypsa", "pypsa-csv").value
```

The written folder can be imported with `pypsa.Network().import_from_csv_folder(path)`. PyPSA itself is not a runtime dependency of powerio.

CSV folders are PyPSA's native static component format and carry the network topology: buses, lines, transformers, generators, loads, shunts, storage units, and links (read as HVDC). NetCDF and HDF5 time series are not supported; they are tracked in [#107](https://github.com/eigenergy/powerio/issues/107).

## GridFM directories

The native wheel includes the GridFM Parquet writer and parser. GridFM is a
directory format, so it uses the same `parse_file` entry point as PyPSA. A one
scenario dataset parses to a balanced network module. A multi scenario dataset
parses to a scenario set that shares one network identity set. Parse losses are
`Diagnostic` records on the returned module, not a separate warning channel.

```python
out = pio.parse_file("case14.m").value.emit_gridfm("out")
dataset = pio.parse_file(out["dir"], format="gridfm")
print(dataset.kind, dataset.diagnostics)

if dataset.kind == "balanced_network_scenario_set":
    scenario_id = dataset.value.keys()[0]
    first = dataset.export_state(scenario=scenario_id)
```

To inspect the raw Parquet tables instead, the preferred read extra is Polars:

```python
import polars as pl

bus = pl.read_parquet(f"{out['dir']}/bus_data.parquet")
```

Use `powerio[pandas]` only for downstream code that expects pandas DataFrames.

## The 1.0 grammar

The Python surface follows one path:

```text
parse_file(path) or parse_text(text)
    -> PioModule { value, diagnostics }
    -> to_* transformation or calc_* calculation
    -> emit(format, destination)
```

There is no second `parse`, `read`, `write`, `to_format`, warning, DC data, or
noun named calculation surface. The exact 0.10 removals are listed in
[Final 1.0 API cleanup](final-v1-api-cleanup.md).

## Build identity

`powerio.versions()` returns the release API discovery document: the powerio release, the stored module schema name and version, and the BMOPF schema this build speaks. Its keys agree with the C `pio_schema_versions_json` report where both apply.

## MCP server

MCP clients can request stored module output from `parse` through the `module` transport and pass that same value back to the other network tools:

```python
parsed = parse(path="case9.m", transport="module")
stored = parsed["module_json"]
summarize(module_json=stored)
calc_matrix("bprime", module_json=stored)
emit(format="psse", destination="case9.raw", module_json=stored)
diagnostics(stored)
```

`summarize`, `to_normalized`, `calc_matrix`, and `emit` also detect stored
module JSON passed through the `json` argument. The stored metadata routes
balanced and multiconductor model JSON.

`python -m powerio.mcp` and the `powerio-mcp` console script are consumer entry points and do not move without a version bump.

The optional MCP server accepts local filesystem paths and `file://` URIs for `path` and `out_path` arguments. Remote URI schemes are rejected. Deployments that need filesystem containment can set `POWERIO_MCP_ALLOWED_ROOTS` to an `os.pathsep` separated list of directories; all MCP reads and writes must resolve under one of those roots. Two legacy single root spellings are read when it is unset, in order: `POWERIO_MCP_ROOT`, then `POWERIO_MCP_ALLOWED_ROOT`. The first variable that is set and non-empty wins.

The policy itself is `powerio.mcp.sandbox`, which imports only the standard library, so a server built on another MCP SDK can apply the same rules:

```python
from powerio.mcp.sandbox import checked_path

path = checked_path(arg, purpose="path")
out = checked_path(arg, purpose="out_path", for_write=True)
```

`checked_path` decodes the argument (local path or `file://` URI), refuses remote schemes, resolves symlinks — including a dangling final component under `for_write`, so a link inside a root cannot redirect a write out of it — and raises `PathNotAllowed` (a `ValueError` subclass) when the result lands outside the roots. Its parts, `allowed_roots`, `decode_local_path`, and `check_allowed_path`, are public too, as is `PathNotAllowed`.
