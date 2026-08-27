# Python API

Install the base package for parsing, writing, JSON transport, and file
conversion. It has no required third party Python packages:

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

`import powerio`, `parse`, `convert_file`, `convert_str`, `to_matpower`, and
`to_json` do not import NumPy, SciPy, NetworkX, Polars, pandas, or pyarrow.

Transmission text and file format names accepted by `parse` and `convert_*` include
`matpower`, `psse`, `powerworld`, `pslf`, `powermodels-json`, `egret-json`,
`pandapower-json`, `goc3-json`, `surge-json`, and `opfdata-json`, plus their
documented aliases. Balanced model JSON is not a case format and has no name
here: read and write it with `powerio.from_json` and
`BalancedNetwork.to_json`. `opfdata-json` reads one extracted JSON document
from a DeepMind OPFData FullTop or N-1 release without PyTorch, and parses to
its solved calculation. PyPSA CSV folders and GridFM Parquet datasets are
directory formats: `parse` takes the folder path, and
`BalancedNetwork.write_pypsa_csv_folder`, `read_gridfm`, and
`BalancedNetwork.write_gridfm` write and read them.

## Canonical use

```python
import powerio as pio

net = pio.parse("case9.m", value_type=pio.BalancedNetwork)
same_text = net.to_matpower()
json_text = net.to_json()
pm = net.to_format("powermodels-json")
pp = net.to_format("pandapower-json")
raw = pio.convert_file("case9.m", "psse")
aux = pio.convert_str(json_text, "powerworld", from_="powermodels-json")
pypsa_out = net.write_pypsa_csv_folder("case9-pypsa")
display = pio.parse_display_file("case.pwd")

module = pio.parse("goc3_case.json")     # a calculation defining source
module.kind                              # "ac_scuc_instance"
module.inspect()                         # the operations the value supports

normalized = net.to_normalized()
bprime = net.bprime()        # needs powerio[matrix]
graph = net.to_networkx()    # needs powerio[graph]
dist_graph = pio.dist.parse_file("feeder.dss").graph()
```

## Model names

`powerio.BalancedNetwork` is the existing balanced transmission handle. v0.4 also
exports `powerio.BalancedNetwork` as the long term family name for the same
handle.
The old `powerio.Case` compatibility alias was removed in v0.4.

For distribution models, use `powerio.dist.MulticonductorNetwork` or the
existing `powerio.dist.MulticonductorNetwork` handle name. The old
`powerio.dist.DistCase` alias was removed in v0.4. `dist_net.graph()` returns
the collapsed bus and terminal graph as Python data.

`parse(source, from_=None, include_root=..., value_type=...)` reads a case
path or in-memory bytes into a `StoredModule` of whichever family claims it
(inferred from the extension and content, or forced with `from_`);
`value_type` narrows to `BalancedNetwork` or `dist.MulticonductorNetwork` in
the same call. Display artifacts are not network cases, so they use the
separate display API:

```python
from pathlib import Path

display = pio.parse_display_file("case.pwd")
same = pio.parse_display_bytes(Path("case.pwd").read_bytes(), "pwd")

assert display.kind == "powerworld"
first = display.data.substations[0]
print(first.number, first.name, first.x, first.y)
```

`display.data` is a `PwdDisplay` with `canvas_width`,
`canvas_height`, `stamp`, and `substations`.

## Problem data

A source that defines a calculation parses to that calculation's typed value:
GO Challenge 3 JSON to an AC SCUC instance, BMOPF JSON to a multiconductor
AC OPF instance, and OPFData JSON to a solved AC OPF. `parse` returns the
`StoredModule` carrying it; `module.inspect()` names the operations the value
supports, and `BalancedNetwork.dc_data(formula)` serves the DC branch data
every language reads under the same names.

## PyPSA folders

PyPSA CSV folders are multi-file datasets, so they use explicit read and write
helpers instead of `Conversion.text`.

```python
import powerio as pio

case = pio.parse("case14.m", value_type=pio.BalancedNetwork)
out = case.write_pypsa_csv_folder("case14-pypsa")
round_trip = pio.parse(out["dir"], "pypsa-csv", value_type=pio.BalancedNetwork)
```

The written folder can be imported with
`pypsa.Network().import_from_csv_folder(path)`. PyPSA itself is not a runtime
dependency of powerio.

CSV folders are PyPSA's native static component format and carry the network
topology: buses, lines, transformers, generators, loads, shunts, storage
units, and links (read as HVDC).
NetCDF and HDF5 time series are not supported. They are tracked in
[#107](https://github.com/eigenergy/powerio/issues/107).

## GridFM reads

The native wheel includes the GridFM Parquet writer and reader.

`read_gridfm(dir, scenario=0)` rebuilds a `BalancedNetwork` from a dataset, the inverse
of `Network.write_gridfm`, returning a `GridfmRead(network, scenario, warnings)`
namedtuple. The read is lossy but recovers everything a power flow needs;
`warnings` lists what the gridfm schema couldn't round-trip (synthesized bus
ids, folded per bus load/shunt, dropped HVDC/storage, piecewise costs).
`read_gridfm_scenarios(dir)` returns one `GridfmRead` per scenario. `dir`
resolves the `raw/` leaf, a `<case>/` directory, or a parent with one `*/raw/`
child.

```python
import powerio as pio

out = pio.parse("case14.m", value_type=pio.BalancedNetwork).write_gridfm("out")
net, scenario, warnings = pio.read_gridfm(out["dir"])
text = net.to_matpower()                 # gridfm → any classical format
```

To inspect the raw Parquet tables instead, the preferred read extra is Polars:

```python
import polars as pl

bus = pl.read_parquet(f"{out['dir']}/bus_data.parquet")
```

Use `powerio[pandas]` only for downstream code that expects pandas DataFrames.

## `.pio.json` documents

`powerio.StoredModule` is the handle for `.pio.json` documents.
`StoredModule.from_json` reads stored text (a released 0.9 package upgrades
one way on read), `StoredModule.from_file` / `from_str` / `from_bytes` parse
case input into a module, and `powerio.parse` is the same universal entry
with `value_type` narrowing. `module.kind` names the typed value;
`as_balanced_network()` / `as_multiconductor_network()` hand back typed
network handles with the module's retained source and findings threaded on,
so a same format write still echoes the source bytes.

`module.inspect()` names the value and its supported operations;
`module.diagnostics()` returns the findings; `module.state_inventory()`
lists the typed time points or scenario IDs a collection carries;
`module.select_state(...)` describes the selected item, and
`module.export_state(...)` materializes it as an independent static module.
Multiconductor values lower through `module.to_balanced_inspect()` and
`module.to_balanced()`.

```python
module = pio.parse("goc3_case.json")           # ac_scuc_instance
module.inspect()                               # names the supported operations

series = pio.StoredModule.from_json(stored_series_text)
inventory = series.state_inventory()           # typed time points or scenarios
static_module = series.export_state(time_position=0)
net = static_module.as_balanced_network()
```

Selection and export apply to the collection kinds (network time series,
operating point time series, scenario sets); a static or instance module
refuses them with `REQUEST.STATE.NOT_A_COLLECTION`.

## MCP path handling

MCP clients can request `.pio.json` document output from `parse` through the
`package` transport and pass that same value back to the other network tools:

```python
parsed = parse(path="case9.m", transport="package")
pkg = parsed["package_json"]
summary(package_json=pkg)
matrix("bprime", package_json=pkg)
save(out_path="case9.raw", to_format="psse", package_json=pkg)
diagnostics(pkg)
```

`summary`, `normalize`, `matrix`, and `save` also auto-detect `.pio.json`
document JSON passed through the legacy `json` argument. The document
metadata's `model_kind` routes balanced and multiconductor model JSON.

`python -m powerio.mcp` and the `powerio-mcp` console script are consumer entry points and do not move without a version bump.

The optional MCP server accepts local filesystem paths and `file://` URIs for
`path` and `out_path` arguments. Remote URI schemes are rejected. Deployments
that need filesystem containment can set `POWERIO_MCP_ALLOWED_ROOTS` to an
`os.pathsep` separated list of directories; all MCP reads and writes must
resolve under one of those roots. Two legacy single root spellings are read when it is unset, in order: `POWERIO_MCP_ROOT`, then `POWERIO_MCP_ALLOWED_ROOT`, an alternate legacy spelling. The first variable that is set and non-empty wins.

The policy itself is `powerio.mcp.sandbox`, which imports only the standard library, so a server built on another MCP SDK can apply the same rules:

```python
from powerio.mcp.sandbox import checked_path

path = checked_path(arg, purpose="path")
out = checked_path(arg, purpose="out_path", for_write=True)
```

`checked_path` decodes the argument (local path or `file://` URI), refuses
remote schemes, resolves symlinks — including a dangling final component under
`for_write`, so a link inside a root cannot redirect a write out of it — and
raises `PathNotAllowed` (a `ValueError` subclass) when the result lands
outside the roots. Its parts, `allowed_roots`, `decode_local_path`, and
`check_allowed_path`, are public too, as is `PathNotAllowed`.
