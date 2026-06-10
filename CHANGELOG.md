# Changelog

## 0.0.1

First release.

- Parsers and writers for MATPOWER `.m`, PSS/E RAW, PowerWorld AUX,
  PowerModels JSON, and egret JSON; byte-exact same-format round trips,
  maximal-fidelity conversion between formats.
- `Network`, the one canonical model, with `to_normalized` deriving a
  per-unit / radian / filtered / reindexed view.
- C ABI (`powerio-capi`, ABI version 3): parse, query, convert, JSON
  transport, an Arrow C Data Interface export behind `--features arrow`, and a
  gridfm-datakit Parquet reader behind `--features gridfm`; cbindgen-generated
  header, version handshake, panic-safe boundary.
- Python bindings (`pip install powerio`) with `matrix`, `graph`, and
  `gridfm` extras, plus an MCP convert/validate server.
- `powerio-matrix`: admittance and Laplacian builders over the parsed
  tables; gridfm-datakit Parquet export and a lossy, power-flow-complete
  reader behind `--features gridfm`.
- `powerio-cli`: convert and validate from the shell.

The C ABI history (versions 1 through 3) is tracked in
[powerio-capi/README.md](powerio-capi/README.md).
