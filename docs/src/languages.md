# Language APIs

PowerIO uses the same IO vocabulary across Rust, Python, Julia, and the C ABI,
with language-specific spelling where needed. A new format or dataset should
appear as a format string or convenience wrapper, not as a new naming scheme.

Verb taxonomy:

- `parse_*`: bytes, paths, or text to typed parsed values. Transmission parsers
  return a balanced network handle; distribution parsers return a
  multiconductor network handle; display parsers return display data.
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
| `.pio.json` document JSON | `NetworkPackage::to_json()` | `Package` class / package transport | `to_package` / `write_package` | `pio_package_*` |
| `.pio.json` operating points | `pkg.operating_points()` | `pkg.operating_points()` | planned | `pio_package_operating_points_json` |
| Materialize operating point | `pkg.materialize_operating_point(i)` | `pkg.materialize_operating_point(i)` | planned | `pio_package_materialize_operating_point` |
| `.pio.json` study block | `pkg.study()` | `pkg.study()` | planned | `pio_package_study_json` |
| Materialize study commit | `pkg.materialize_study_commit(i)` | `pkg.materialize_study_commit(i)` | planned | `pio_package_materialize_study_commit` |
| Normalized copy | `net.to_normalized()` | `net.to_normalized()` | `to_normalized(net)` | `pio_normalize` |
| Dense tables | typed table API | `to_dense` | `to_dense` | `pio_*` extractors |
| PyPSA CSV folder | `read_pypsa_csv_folder` / `write_pypsa_csv_folder` | `read_pypsa_csv_folder` / `net.write_pypsa_csv_folder` | `parse_file(dir; from="pypsa-csv")` / `write_pypsa_csv_folder` | `pio_parse_file` / `pio_write_dir` + `"pypsa-csv"` |
| gridfm write | `write_gridfm_dataset` / `write_gridfm_batch` | `net.write_gridfm` / `write_gridfm_batch` | planned | planned |
| gridfm read | `read_gridfm_dataset(dir, scenario)` | `read_gridfm(dir, scenario=0)` | `read_gridfm(dir; scenario=0)` | `pio_read_dir` + `"gridfm"` |
| Arrow handoff | internal/C ABI | later | `to_arrow` | `pio_to_arrow` |

**Note:** the C ABI carries no per-format symbols: matpower, `powerio-json`,
PyPSA CSV directories, and gridfm datasets are all format strings into
`pio_to_format` / `pio_parse_str` / `pio_write_dir` / `pio_read_dir`. Removing
or changing a documented format token is a C behavior change even though the C
signature stays the same. The language APIs keep their per-format conveniences
(`to_matpower`, `from_json`, ...) as wrappers over the same paths.

## C ABI and binding compatibility

The C ABI is the stable boundary for non Rust callers. Handles own parsed
networks. `PioPackage` handles own `.pio.json` documents. Callers free
network handles with `pio_network_free`, package handles with
`pio_package_free`, free returned text with `pio_string_free`, size output
buffers before filling them, and treat every format name as a string routed
through the same parser and writer hub.

C ABI review points:

- null handles must return documented defaults or errors, not crash;
- optional output buffers must be safe to pass as null; required output structs
  such as Arrow exports must report an error when null;
- returned text and warning buffers must be NUL terminated when capacity permits;
- reported lengths must let callers allocate exact buffers;
- header declarations and exported Rust symbols must match;
- feature gated exports such as Arrow, GridFM, distribution, and packages must
  be additive;
- ownership rules must be documented in the header, README, and binding code.

Julia's `PowerIO.jl` uses the C ABI for handles, dense extractors, Arrow,
GridFM, PyPSA CSV folders, distribution conversion, and `.pio.json` document
construction. Programmatic whole-network JSON remains available through
`powerio-json`; file handoffs should use `.pio.json`. The Julia binding checks
`pio_abi_version()` against `PIO_ABI_VERSION` on first use. Distribution calls
also check `pio_dist_abi_version()`.

GOC3 document construction is the first `.pio.json` operating point path backed
by a source format. The static balanced model JSON carries the first interval;
the replayable series is exposed through the package APIs above.

During development, test the sibling Julia binding against the local C ABI
instead of an artifact:

```sh
cargo build -p powerio-capi --release --features arrow,matrix,gridfm,dist,pkg
POWERIO_CAPI=$PWD/target/release/libpowerio_capi.dylib \
  julia --project=../PowerIO.jl -e 'using Pkg; Pkg.test()'
```

Binding compatibility checks:

| surface | behavior |
| --- | --- |
| Python base import | `import powerio` does not import NumPy, SciPy, NetworkX, Polars, pandas, pyarrow, or the MCP SDK |
| Python optional paths | matrix, graph, GridFM inspection, pandas, MCP, and benchmark oracles live behind extras |
| C ABI | `pio_abi_version()` is the core compatibility check; optional symbols are additive and feature probed |
| Julia | `PowerIO.jl` checks the C ABI version before first use and checks `pio_dist_abi_version()` before distribution calls |
| Arrow | C returns Arrow C Data Interface structs; Julia's default `to_arrow` copies to owned vectors, while `copy=false` keeps the wrapper alive for zero copy reads |
| GridFM | Julia and C read GridFM through `pio_read_dir` / `"gridfm"` and surface schema losses as warnings |
| Distribution | Python, Julia, Rust, and C use separate distribution handles; transmission and distribution conversion paths do not mix |

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
| Graph projection | `net.graph()` | `case.graph()` | planned | `pio_dist_graph_json` |
