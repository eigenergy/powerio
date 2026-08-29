# Python API

Install the base package for parsing, writing, JSON transport, and file conversion. It has no required third party Python packages:

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

`import powerio`, `parse`, `convert_file`, `convert_str`, `to_matpower`, and `to_json` do not import NumPy, SciPy, NetworkX, Polars, pandas, or pyarrow.

## One call parses

`powerio.parse(source)` reads a case path or in-memory bytes into a `PioModule` and detects the value kind from the extension and content. `from_` forces a format name; `value_type` asserts the expected value class and raises when the source parses to something else.

```python
import powerio as pio

case = pio.parse("case9.m")          # PioModule, kind "balanced_network"
net = case.value                     # BalancedNetwork
case.diagnostics()                   # the reader's findings, typed records
feeder = pio.parse("IEEE13Nodeckt.dss")  # kind "multiconductor_network"
instance = pio.parse("goc3_case.json")  # kind "ac_scuc_instance"
instance.inspect()                   # the operations the value supports
```

Balanced network formats accepted by `parse` and the `convert_*` functions include `matpower`, `psse`, `powerworld`, `pslf`, `powermodels-json`, `egret-json`, `pandapower-json`, `goc3-json`, `surge-json`, and `opfdata-json`, plus their documented aliases. Multiconductor sources are OpenDSS (`dss`) and PMD engineering JSON. Balanced model JSON is the bindings' data transport rather than a case format and has no name here: read and write it with `powerio.from_json` and `BalancedNetwork.to_json`. `opfdata-json` reads one extracted JSON document from a DeepMind OPFData FullTop or N-1 release without PyTorch and parses to its solved calculation. PyPSA CSV folders and GridFM Parquet datasets are directory formats: `parse` takes the folder path, and `BalancedNetwork.write_pypsa_csv_folder`, `read_gridfm`, and `BalancedNetwork.write_gridfm` write and read them.

When `include_root` is omitted, a file's referenced includes resolve only beneath its own containing directory; passing `include_root` widens that boundary to the named ancestor directory, and with it the set of files the parse may read.

## The module

`PioModule` carries one typed value with its records: the retained source, the reader's findings, and the descriptive history. `module.kind` names the value; `module.value` returns it. Balanced and multiconductor networks come back as the full network handles; the calculation instances, solutions, time series, and scenario sets come back as thin typed holders (`AcScucInstance`, `TimeSeries`, ...) that point back at the owning module.

```python
module = pio.parse("case9.m")
module.kind                        # "balanced_network"
net = module.as_balanced_network() # typed handle, provenance threaded on
text = module.to_json()            # the .pio.json stored document
```

`as_balanced_network()` and `as_multiconductor_network()` assert the kind and hand back the typed network with the module's retained source and findings threaded on, so a same format write still echoes the source bytes. `PioModule.from_json` reads stored `.pio.json` text (a released 0.9 document upgrades one way on read); `PioModule.from_file`, `from_str`, and `from_bytes` parse case input, equivalent to `powerio.parse`.

## Findings are records

Every reader and converter reports what it could not represent as `Diagnostic` records: a stable dotted `code`, a `severity` (`error`, `warning`, `remark`, `note`), the rendered `message`, and, when stated, a `target`, a `suggested_action`, `spans` (byte ranges into the retained source, as `SourceSpan` records), `related` record ids, and an open `details` object. Branch on `code`, never on message text.

```python
for d in pio.parse("case.raw").diagnostics():
    print(d.code, d.severity, d.message)
    for span in d.spans:
        print("  at", span.source, span.byte_start, span.byte_end)
```

Parse failures raise `PowerIOParseError` and data failures `PowerIODataError`, both `PowerIOError` (a `ValueError`); the message opens with the finding's code.

## Networks and conversion

`BalancedNetwork` is the balanced transmission value; `powerio.dist.MulticonductorNetwork` is the multiconductor distribution value. Conversion serializes through the typed model and reports fidelity as records:

```python
net = pio.parse("case9.m").value
same_text = net.to_matpower()            # same format echo, byte exact
psse_text, warnings = net.to_format("psse")
raw = pio.convert_file("case9.m", "psse")         # Conversion(text, warnings)
aux = pio.convert_str(json_text, "powerworld", from_="powermodels-json")
report = net.write_file("case9.raw", "psse")
normalized = net.to_normalized()
```

`Conversion` is a named tuple of the output text and the writer's `Diagnostic` records. A parsed, unchanged network writing its own format echoes the retained source bytes exactly; `to_canonical_format` bypasses the echo and serializes from the typed model.

The matrix extra serves the sparse system matrices and DC data under the same names every language uses: `net.bprime()`, `net.bdoubleprime()`, `net.ybus()`, `net.ybus_parts()`, `net.incidence()`, `net.adjacency()`, `net.ptdf()`, `net.lodf()`, `net.weighted_laplacian()`, `net.lacpf()`, and `net.dc_data(formula)`. The graph extra adds `net.to_networkx()`, and `feeder.graph()` returns the multiconductor bus and terminal graph as Python data.

## Display artifacts

Display artifacts are drawings rather than network cases, so they use the separate display API:

```python
from pathlib import Path

display = pio.parse_display_file("case.pwd")
same = pio.parse_display_bytes(Path("case.pwd").read_bytes(), "pwd")

assert display.kind == "powerworld"
first = display.data.substations[0]
print(first.number, first.name, first.x, first.y)
```

`display.data` is a `PwdDisplay` with `canvas_width`, `canvas_height`, `stamp`, and `substations`.

## Problem data

A source that defines a calculation parses to that calculation's typed value: GO Challenge 3 JSON to an AC SCUC instance, BMOPF JSON to a multiconductor AC OPF instance, and OPFData JSON to a solved AC OPF. `module.inspect()` names the operations the value supports, and `BalancedNetwork.dc_data(formula)` serves the DC branch data every language reads under the same names.

## Collections and state selection

Network time series, operating point time series, and scenario sets are collections. `module.state_inventory()` lists the typed time points or scenario ids; `module.select_state(...)` describes one selected item, and `module.export_state(...)` materializes it as an independent static module. A static or instance module refuses them with `REQUEST.STATE.NOT_A_COLLECTION`. Multiconductor values lower through `module.to_balanced_inspect()` and `module.to_balanced()`.

```python
series = pio.PioModule.from_json(stored_series_text)
inventory = series.state_inventory()
static_module = series.export_state(time_position=0)
net = static_module.as_balanced_network()
```

## PyPSA folders

PyPSA CSV folders are multi-file datasets, so they use explicit read and write helpers instead of `Conversion.text`.

```python
case = pio.parse("case14.m").value
out = case.write_pypsa_csv_folder("case14-pypsa")
round_trip = pio.parse(out["dir"], "pypsa-csv").value
```

The written folder can be imported with `pypsa.Network().import_from_csv_folder(path)`. PyPSA itself is not a runtime dependency of powerio.

CSV folders are PyPSA's native static component format and carry the network topology: buses, lines, transformers, generators, loads, shunts, storage units, and links (read as HVDC). NetCDF and HDF5 time series are not supported; they are tracked in [#107](https://github.com/eigenergy/powerio/issues/107).

## GridFM reads

The native wheel includes the GridFM Parquet writer and reader.

`read_gridfm(dir, scenario=0)` rebuilds a `BalancedNetwork` from a dataset, the inverse of `BalancedNetwork.write_gridfm`, returning a `GridfmRead(network, scenario, warnings)` named tuple. The read is lossy but recovers everything a power flow needs; `warnings` lists what the gridfm schema could not round trip (synthesized bus ids, folded per bus load and shunt, dropped HVDC and storage, piecewise costs). `read_gridfm_scenarios(dir)` returns one `GridfmRead` per scenario. `dir` resolves the `raw/` leaf, a `<case>/` directory, or a parent with one `*/raw/` child.

```python
out = pio.parse("case14.m").value.write_gridfm("out")
net, scenario, warnings = pio.read_gridfm(out["dir"])
text = net.to_matpower()                 # gridfm → any classical format
```

To inspect the raw Parquet tables instead, the preferred read extra is Polars:

```python
import polars as pl

bus = pl.read_parquet(f"{out['dir']}/bus_data.parquet")
```

Use `powerio[pandas]` only for downstream code that expects pandas DataFrames.

## Build identity

`powerio.versions()` returns the release API discovery document: the powerio release, the stored module schema name and version, and the BMOPF schema this build speaks. Its keys agree with the C `pio_schema_versions_json` report where both apply.

## MCP server

MCP clients can request stored module output from `parse` through the `module` transport and pass that same value back to the other network tools:

```python
parsed = parse(path="case9.m", transport="module")
stored = parsed["module_json"]
summary(module_json=stored)
matrix("bprime", module_json=stored)
save(out_path="case9.raw", to_format="psse", module_json=stored)
diagnostics(stored)
```

`summary`, `normalize`, `matrix`, and `save` also detect stored module JSON passed through the `json` argument. The stored metadata routes balanced and multiconductor model JSON.

`python -m powerio.mcp` and the `powerio-mcp` console script are consumer entry points and do not move without a version bump.

The optional MCP server accepts local filesystem paths and `file://` URIs for `path` and `out_path` arguments. Remote URI schemes are rejected. Deployments that need filesystem containment can set `POWERIO_MCP_ALLOWED_ROOTS` to an `os.pathsep` separated list of directories; all MCP reads and writes must resolve under one of those roots. Two legacy single root spellings are read when it is unset, in order: `POWERIO_MCP_ROOT`, then `POWERIO_MCP_ALLOWED_ROOT`. The first variable that is set and non-empty wins.

The policy itself is `powerio.mcp.sandbox`, which imports only the standard library, so a server built on another MCP SDK can apply the same rules:

```python
from powerio.mcp.sandbox import checked_path

path = checked_path(arg, purpose="path")
out = checked_path(arg, purpose="out_path", for_write=True)
```

`checked_path` decodes the argument (local path or `file://` URI), refuses remote schemes, resolves symlinks — including a dangling final component under `for_write`, so a link inside a root cannot redirect a write out of it — and raises `PathNotAllowed` (a `ValueError` subclass) when the result lands outside the roots. Its parts, `allowed_roots`, `decode_local_path`, and `check_allowed_path`, are public too, as is `PathNotAllowed`.
