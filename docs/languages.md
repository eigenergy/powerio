# Language APIs

PowerIO keeps the same IO vocabulary across Rust, Python, Julia, and the C ABI
while using each language's own style. The goal is that a new format or dataset
appears as a format string or convenience wrapper, not as a new naming scheme.

Verb taxonomy:

- `parse_*`: bytes, paths, or text to typed parsed values. Case parsers return
  `Network`; display parsers return display data.
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
| Parse display path | `parse_display_file(path, from)` | `parse_display_file(path, from_=None)` | planned | n/a |
| Parse display bytes | `parse_display_bytes(bytes, format)` | `parse_display_bytes(data, format)` | planned | n/a |
| Parse IO | n/a | file object later | `parse_file(io, format)` | n/a |
| JSON to Network | `Network::from_json` | `from_json` | `from_json` | `pio_parse_str` + `"powerio-json"` |
| File conversion | `convert_file(path, to, from)` | `convert_file(path, to, from_=None)` | `convert_file(path, to; from=nothing)` | `pio_convert_file` |
| Text conversion | `convert_str(text, to, format)` | `convert_str(text, to, format)` | `convert_str(text, to; from=format)` | `pio_convert_str` |
| Parsed conversion | `net.to_format(to)` | `net.to_format(to)` | `to_format(net, to)` | `pio_to_format` |
| MATPOWER text | `net.to_matpower()` | `net.to_matpower()` | `to_matpower(net)` | `pio_to_format` + `"matpower"` |
| JSON text | `net.to_json()` | `net.to_json()` | `to_json(net)` | `pio_to_format` + `"powerio-json"` |
| Normalized copy | `net.to_normalized()` | `net.to_normalized()` | `to_normalized(net)` | `pio_normalize` |
| Dense tables | typed table API | `to_dense` | `to_dense` | `pio_*` extractors |
| PyPSA CSV folder | `read_pypsa_csv_folder` / `write_pypsa_csv_folder` | `read_pypsa_csv_folder` / `net.write_pypsa_csv_folder` | `parse_file(dir; from="pypsa-csv")` / `write_pypsa_csv_folder` | `pio_parse_file` / `pio_write_dir` + `"pypsa-csv"` |
| gridfm read | `read_gridfm_dataset(dir, scenario)` | `read_gridfm(dir, scenario=0)` | `read_gridfm(dir; scenario=0)` | `pio_read_dir` + `"gridfm"` |
| Arrow handoff | internal/C ABI | later | `to_arrow` | `pio_to_arrow` |

**Note:** the C ABI carries no per-format symbols: matpower, the powerio-json
snapshot, PyPSA CSV directories, and gridfm datasets are all format strings into
`pio_to_format` / `pio_parse_str` / `pio_write_dir` / `pio_read_dir`. The
language APIs keep their per-format conveniences (`to_matpower`, `from_json`,
...) as wrappers over the same paths.

## Distribution surface (`powerio-dist`)

The multiconductor distribution model follows the same taxonomy under its own
handle type; the two families do not mix. The C distribution surface ships
behind the optional `dist` feature (`PIO_DIST`); a consumer probes it with
`pio_has_feature("dist")`, then checks `pio_dist_abi_version()` against
`PIO_DIST_ABI_VERSION`. PowerIO.jl uses the same runtime check before calling
the distribution C conversion helpers.

| Concept | Rust | Python | Julia | C ABI |
|---|---|---|---|---|
| Parse path | `powerio_dist::parse_file(path, from)` | `dist.parse_file(path, from_=None)` | `parse_file(DistNetwork, path; from=nothing)` | `pio_dist_parse_file` |
| Parse text | `powerio_dist::parse_str(text, format)` | `dist.parse_str(text, format)` | `parse_str(DistNetwork, text, format)` | `pio_dist_parse_str` |
| File conversion | `powerio_dist::convert_file(path, to, from)` | `dist.convert_file(path, to, from_=None)` | `convert_file(DistNetwork, path, to; from=nothing)` | `pio_dist_convert_file(path, from, to, ...)` |
| Target format type | `DistTargetFormat` (`FromStr`, `name()`) | format name strings | `DistNetwork` plus format strings | format name strings |
| Text conversion | `powerio_dist::convert_str(text, to, format)` | `dist.convert_str(text, to, format)` | `convert_str(DistNetwork, text, to, format)` | `pio_dist_convert_str(text, from, to, ...)` |
| Parsed conversion | `net.to_format(to)` | `case.to_format(to)` | `to_format(net, to)` | `pio_dist_to_format` |
| Parse warnings | `net.warnings` | `case.warnings` | `warnings(net)` | `pio_dist_warnings` |
