# Language APIs

PowerIO attempts to propose a canonical naming system for IO across Rust, Python, Julia, and the C
ABI while still using each language's own style. **PowerIO is under active development and this system is subject to change.**

Verb taxonomy:

- `parse_*`: bytes, paths, or text to `Network`
- `to_*`: `Network` to a new value
- `convert_file`: path to target text convenience
- `write_*`: filesystem outputs (`write_gridfm`, `write_pypsa_csv_folder`,
  `write_dcopf_bundle`); the Rust
  hub also keeps `write_as` and per-format `write_*` text builders, the
  internals behind `to_format` and the `to_*` writers, which the bindings do
  not mirror
- `read_*`: filesystem dataset inputs (`read_gridfm`, `read_pypsa_csv_folder`), the inverse of
  `write_*`. Datasets are multi-file directories, so they read and write;
  single documents parse and serialize (`parse_*`/`to_*`)
- `export_*`: handoff to external memory or interface protocols

| Concept | Rust | Python | Julia | C ABI |
|---|---|---|---|---|
| Parse path | `parse_file(path, from)` | `parse_file(path, from_=None)` | `parse_file(path; from=nothing)` | `pio_parse_file` |
| Parse text | `parse_str(text, format)` | `parse_str(text, format)` | `parse_str(text, format)` | `pio_parse_str` |
| Parse IO | n/a | file object later | `parse_file(io, format)` | n/a |
| JSON to Network | `Network::from_json` | `from_json` | `from_json` | `pio_parse_str` + `"powerio-json"` |
| File conversion | `convert_file(path, to, from)` | `convert_file(path, to, from_=None)` | `convert_file(path, to; from=nothing)` | `pio_convert_file` |
| Text conversion | `convert_str(text, to, format)` | `convert_str(text, to, format)` | planned | `pio_convert_str` |
| Parsed conversion | `net.to_format(to)` | `net.to_format(to)` | `to_format(net, to)` | `pio_to_format` |
| MATPOWER text | `net.to_matpower()` | `net.to_matpower()` | `to_matpower(net)` | `pio_to_format` + `"matpower"` |
| JSON text | `net.to_json()` | `net.to_json()` | `to_json(net)` | `pio_to_format` + `"powerio-json"` |
| Normalized copy | `net.to_normalized()` | `net.to_normalized()` | `to_normalized(net)` | `pio_normalize` |
| Dense tables | typed table API | `to_dense` | `to_dense` | `pio_*` extractors |
| PyPSA CSV folder | `read_pypsa_csv_folder` / `write_pypsa_csv_folder` | `read_pypsa_csv_folder` / `net.write_pypsa_csv_folder` | planned | `pio_parse_file` / `pio_write_dir` + `"pypsa-csv"` |
| gridfm read | `read_gridfm_dataset(dir, scenario)` | `read_gridfm(dir, scenario=0)` | `read_gridfm(dir; scenario=0)` (PR open) | `pio_read_dir` + `"gridfm"` |
| Arrow handoff | internal/C ABI | later | `to_arrow` | `pio_to_arrow` |

**Note:** the C ABI carries no per-format symbols: matpower, the powerio-json
snapshot, PyPSA CSV directories, and gridfm datasets are all format strings into
`pio_to_format` / `pio_parse_str` / `pio_write_dir` / `pio_read_dir`. The
language APIs keep their per-format conveniences (`to_matpower`, `from_json`,
...) as wrappers over the same paths.
