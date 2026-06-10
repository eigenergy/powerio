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

`import powerio`, `parse_file`, `parse_str`, `convert_file`, `to_matpower`, and
`to_json` do not import NumPy, SciPy, NetworkX, Polars, pandas, or pyarrow.

## Canonical use

```python
import powerio as pio

net = pio.parse_file("case9.m")
same_text = net.to_matpower()
json_text = net.to_json()
pm = net.to_format("powermodels-json")
raw = pio.convert_file("case9.m", "psse")

normalized = net.to_normalized()
dense = net.to_dense()       # needs powerio[matrix]
bprime = net.bprime()        # needs powerio[matrix]
graph = net.to_networkx()    # needs powerio[graph]
```

`parse_file(path, from_=None)` reads any format (inferred from the extension, or
forced with `from_`); `parse_str(text, format)` reads in-memory text.

## GridFM reads

The native wheel includes the GridFM Parquet writer and reader.

`read_gridfm(dir, scenario=0)` rebuilds a `Network` from a dataset, the inverse
of `Network.write_gridfm`, returning a `GridfmRead(network, scenario, warnings)`
namedtuple. The read is lossy but recovers everything a power flow needs;
`warnings` lists what the gridfm schema couldn't round-trip (synthesized bus
ids, folded per-bus load/shunt, dropped HVDC/storage, piecewise costs).
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
