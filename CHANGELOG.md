# Changelog

## 0.2.0

The PowerWorld compatibility release.

- Full `.aux` fidelity (#95): the reader covers the complete aux grammar
  (legacy and concise headers, multiline field lists, SUBDATA and SCRIPT
  blocks retained verbatim) and all three field naming generations through
  Simulator 21+, merging the DATA sections real exports spread one object
  over; byte-exact echo, parity tested against sibling MATPOWER and PSS/E
  exports of the same cases.
- Read-only `.pwb` binary reader (#95, #102, #105): 2016 through 2022 era
  exports under the six decoded header constants 425, 483, 508, 537, 550,
  and 551 — every Texas7k save decodes — with the table search running
  faster than the aux text reader on every sibling pair; unsupported writer
  vintages are detected and rejected with the format constant named.
- Read-only `.pwd` display reader (#102): `powerworld::parse_pwd` extracts
  substation diagram coordinates, matched 1-1 against the aux substations
  on every probed save with a same-vintage aux (the v19 resave matches
  1248/1250 against the published case, a vintage skew).
- The decoded vintages and the per-field evidence behind the parity claims
  are documented in [docs/powerworld.md](docs/powerworld.md).

## 0.1.1

- File extension detection is case-insensitive (#97, #101): `parse_file`
  accepts `.RAW`/`.M`/`.JSON`/`.AUX` and any mixed case alongside the
  lowercase forms, and the CLI batch discovery and TUI file browser find
  such files too. Reported by @jd-foster.
- MCP server error hardening (#93): an unreadable input file surfaces as
  the documented ValueError shape instead of a raw `PermissionError`, with
  defensive guards on the JSON load and matrix dispatch paths.

## 0.1.0

- gridfm read path (#70): `read_gridfm_dataset` / `read_gridfm_scenarios` /
  `gridfm_base_case` in `powerio-matrix`, `pio_read_gridfm` /
  `pio_gridfm_scenario_ids` in the C ABI behind `--features gridfm`, and
  `powerio.read_gridfm` / `read_gridfm_scenarios` in Python. Release tarballs
  now build the C ABI with the gridfm feature, so the symbols ship to the
  Julia bindings.
- `convert_str` (#88): in-memory conversion through the hub in Rust and
  Python; the MCP server's inline conversion no longer stages temp files.
  Closes #66.
- The MCP server grows from two tools to eight (#90): `parse_case`,
  `normalize_case`, and `case_to_json` emit the JSON transport,
  `compute_matrix` returns nine sparse kinds in COO form, `dense_view`
  returns the dense table view, and `save_case` writes converted cases to
  disk; `convert_case` and `case_summary` are unchanged.
- Docs (#92): Pages landing page with the released/development split, guide
  links, and the logo; the crate homepage points at the docs site; release
  drafts carry the CHANGELOG section instead of a bare title.

## 0.0.1

First release.

- Parsers and writers for MATPOWER `.m`, PSS/E RAW, PowerWorld AUX,
  PowerModels JSON, and egret JSON; byte-exact same-format round trips,
  maximal-fidelity conversion between formats.
- `Network`, the one canonical model, with `to_normalized` deriving a
  per-unit / radian / filtered / reindexed view.
- C ABI (`powerio-capi`, ABI version 3): parse, query, convert, JSON
  transport, and Arrow C Data Interface export behind `--features arrow`;
  cbindgen-generated header, version handshake, panic-safe boundary.
- Python bindings (`pip install powerio`) with `matrix`, `graph`, and
  `gridfm` extras, plus an MCP convert/validate server.
- `powerio-matrix`: admittance and Laplacian builders over the parsed
  tables; gridfm Parquet export behind `--features gridfm`.
- `powerio-cli`: convert and validate from the shell.

The C ABI history (versions 1 through 3) is tracked in
[powerio-capi/README.md](powerio-capi/README.md).
