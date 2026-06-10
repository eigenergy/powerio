# Language APIs

PowerIO attempts to propose a canonical naming system for IO across Rust, Python, Julia, and the C
ABI while still using each language's own style. **PowerIO is under active development and this system is subject to change.**

Verb taxonomy:

- `parse_*`: bytes, paths, or text to `Network`
- `to_*`: `Network` to a new value
- `convert_file`: path to target text convenience
- `write_*`: filesystem outputs (`write_gridfm`, `write_dcopf_bundle`); the Rust
  hub also keeps `write_as` and per-format `write_*` text builders, the
  internals behind `to_format` and the `to_*` writers, which the bindings do
  not mirror
- `export_*`: handoff to external memory or interface protocols

| Concept | Rust | Python | Julia | C ABI |
|---|---|---|---|---|
| Parse path | `parse_file(path, from)` | `parse_file(path, from_=None)` | `parse_file(path; from=nothing)` | `pio_parse_file` |
| Parse text | `parse_str(text, format)` | `parse_str(text, format)` | `parse_str(text, format)` | `pio_parse_str` |
| Parse IO | n/a | file object later | `parse_file(io, format)` | n/a |
| JSON to Network | `Network::from_json` | `from_json` | `from_json` | `pio_from_json` |
| File conversion | `convert_file(path, to, from)` | `convert_file(path, to, from_=None)` | `convert_file(path, to; from=nothing)` | `pio_convert_file` |
| Parsed conversion | `net.to_format(to)` | `net.to_format(to)` | `to_format(net, to)` | `pio_to_format` |
| MATPOWER text | `net.to_matpower()` | `net.to_matpower()` | `to_matpower(net)` | `pio_to_matpower` |
| JSON text | `net.to_json()` | `net.to_json()` | `to_json(net)` | `pio_to_json` |
| Normalized copy | `net.to_normalized()` | `net.to_normalized()` | `to_normalized(net)` | `pio_to_normalized` |
| Dense tables | typed table API | `to_dense` | `to_dense` | `pio_*` extractors |
| Arrow handoff | internal/C ABI | later | `to_arrow` | `pio_export_arrow` |

**Note:** `pio_export_arrow` keeps `export` because it fills Arrow C Data Interface
structs with release callbacks. It is not an owned string or handle return like
the `to_*` functions.
