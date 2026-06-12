# Python API

Install the base package for parsing, writing, JSON transport, and file
conversion with zero dependencies:

```bash
pip install powerio
```

Install extras only for the views that need them:

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

normalized = net.to_normalized()
dense = net.to_dense()       # needs powerio[matrix]
bprime = net.bprime()        # needs powerio[matrix]
graph = net.to_networkx()    # needs powerio[graph]
```

`parse_file(path, from_=None)` reads any format (inferred from the extension, or
forced with `from_`); `parse_str(text, format)` reads in-memory text.

## PyPSA folders

PyPSA CSV folders are multi-file datasets, so they use explicit read/write
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
Time series scenarios in NetCDF/HDF5 are out of scope for now; support is
tracked in [#107](https://github.com/eigenergy/powerio/issues/107).

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
