# Python API

Install the base package for parsing, writing, JSON transport, and file
conversion:

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

Shorthand aliases:

```python
pio.parse("case9.m")
pio.convert("case9.m", "psse")
pio.parse_matpower("case9.m")
pio.parse_matpower_string(text)
```

## GridFM reads

The native wheel includes the GridFM Parquet writer. The preferred Python read
extra is Polars:

```python
import polars as pl
import powerio as pio

out = pio.parse_file("case14.m").write_gridfm("out")
bus = pl.read_parquet(f"{out['dir']}/bus_data.parquet")
```

Use `powerio[pandas]` only for downstream code that expects pandas DataFrames.

## Release to PyPI

Publishing uses PyPI trusted publishing from the `publish` job in
`.github/workflows/python.yml`.

Before launch:

- Create the PyPI project and configure a trusted publisher for
  `eigenergy/powerio`, workflow `python.yml`, environment `pypi`.
- Protect the GitHub `pypi` environment with required reviewers, so a GitHub
  release cannot upload without approval.
- Confirm all wheel artifacts and the sdist pass `twine check`; the workflow
  runs this before upload.
- Publish only from a GitHub release after explicit approval.
- Do not store a PyPI token in GitHub; the job uses the OIDC `id-token`
  permission.
