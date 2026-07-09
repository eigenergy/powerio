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

`import powerio`, `parse_file`, `parse_str`, `convert_file`, `convert_str`,
`to_matpower`, and `to_json` do not import NumPy, SciPy, NetworkX, Polars,
pandas, or pyarrow.

Transmission text and file format names accepted by `parse_*` and `convert_*` include
`matpower`, `psse`, `powerworld`, `pslf`, `powermodels-json`, `egret-json`,
`pandapower-json`, `goc3-json`, `surge-json`, and `powerio-json`, plus their
documented aliases. PyPSA CSV folders and GridFM Parquet datasets are directory
formats; use `read_pypsa_csv_folder`, `Network.write_pypsa_csv_folder`,
`read_gridfm`, `Network.write_gridfm`, or the conversion/package helpers that
take a path.

## Canonical use

```python
import powerio as pio

net = pio.parse_file("case9.m")
same_text = net.to_matpower()
json_text = net.to_json()
pm = net.to_format("powermodels-json")
pp = net.to_format("pandapower-json")
raw = pio.convert_file("case9.m", "psse")
aux = pio.convert_str(json_text, "powerworld", format="powermodels-json")
pypsa_out = net.write_pypsa_csv_folder("case9-pypsa")
display = pio.parse_display_file("case.pwd")
pkg = pio.Package.from_file("goc3_case.json", from_="goc3-json")
points = pkg.operating_points()
period_1 = pkg.materialize_operating_point(1)

normalized = net.to_normalized()
dense = net.to_dense()       # needs powerio[matrix]
bprime = net.bprime()        # needs powerio[matrix]
graph = net.to_networkx()    # needs powerio[graph]
dist_graph = pio.dist.parse_file("feeder.dss").graph()
scopf = pio.parse_scopf(goc3_text, from_="goc3-json")
```

## Model names

`powerio.Network` is the existing balanced transmission handle. v0.4 also
exports `powerio.BalancedNetwork` as the long term family name for the same
handle.
The old `powerio.Case` compatibility alias was removed in v0.4.

For distribution models, use `powerio.dist.MulticonductorNetwork` or the
existing `powerio.dist.DistNetwork` handle name. The old
`powerio.dist.DistCase` alias was removed in v0.4. `dist_net.graph()` returns
the collapsed bus and terminal graph as Python data.

`parse_file(path, from_=None)` reads network case files (inferred from the
extension, or forced with `from_`); `parse_str(text, format)` reads in-memory
case text. Display artifacts are not network cases, so they use the separate
display API:

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

## Problem instances

`parse_scopf(text, from_="goc3-json")` assembles a matrix free SCOPF problem
instance and returns its versioned wire document as a Python dictionary. The
document declares its schema version and uses 1-based indices for language
compatibility. Source UIDs and source bus IDs remain separate from those
indices. Invalid JSON, duplicate identities, missing references, and period
length mismatches raise `PowerIOError` subclasses.

## PyPSA folders

PyPSA CSV folders are multi-file datasets, so they use explicit read and write
helpers instead of `Conversion.text`.

```python
import powerio as pio

case = pio.parse_file("case14.m")
out = case.write_pypsa_csv_folder("case14-pypsa")
round_trip = pio.read_pypsa_csv_folder(out["dir"])
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

`read_gridfm(dir, scenario=0)` rebuilds a `Network` from a dataset, the inverse
of `Network.write_gridfm`, returning a `GridfmRead(network, scenario, warnings)`
namedtuple. The read is lossy but recovers everything a power flow needs;
`warnings` lists what the gridfm schema couldn't round-trip (synthesized bus
ids, folded per bus load/shunt, dropped HVDC/storage, piecewise costs).
`read_gridfm_scenarios(dir)` returns one `GridfmRead` per scenario. `dir`
resolves the `raw/` leaf, a `<case>/` directory, or a parent with one `*/raw/`
child.

```python
import powerio as pio

out = pio.parse_file("case14.m").write_gridfm("out")
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

`powerio.Package` is the handle for `.pio.json` documents: it parses the
document metadata once and every accessor reuses the handle. `Package.from_file`
and `Package.from_str` build documents from case input, `Package.from_json`
reads document text, and `Package.from_balanced` /
`Package.from_multiconductor` wrap existing networks. `pkg.model_kind` names
the document family;
`pkg.as_balanced()` / `pkg.as_multiconductor()` rebuild typed network handles
from the model JSON.

`pkg.operating_points()` returns a Python dict for the replayable operating
point series, or `None`. `pkg.materialize_operating_point(i)` returns a new
static `Package` with one point applied; updates resolve by the model rows'
`uid` identities, and an unknown identity or a row that contradicts one raises
`ValueError`. GOC3 documents populate this series from the source time series
while the static model JSON holds the first interval. Network table dicts
(`net.buses`, `net.loads`, ...) expose each row's `uid`.
`pkg.study()` returns a Python dict for the package study block, or `None`;
`pkg.materialize_study_commit(i)` folds cumulative commits through `i` into a
new static package and clears both replay blocks.
`pkg.validate()`, `pkg.validation()`, and `pkg.diagnostics()` expose the
document validation profile, and multiconductor documents lower through
`pkg.multiconductor_to_balanced_preflight()` and
`pkg.lower_multiconductor_to_balanced()`.

```python
pkg = pio.Package.from_file("goc3_case.json", from_="goc3-json")
series = pkg.operating_points()
static_pkg = pkg.materialize_operating_point(0)
net = static_pkg.as_balanced()
```

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

The optional MCP server accepts local filesystem paths and `file://` URIs for
`path` and `out_path` arguments. Remote URI schemes are rejected. Deployments
that need filesystem containment can set `POWERIO_MCP_ALLOWED_ROOTS` to an
`os.pathsep` separated list of directories; all MCP reads and writes must
resolve under one of those roots. `POWERIO_MCP_ROOT` is accepted as a single
root alias.
