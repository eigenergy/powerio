# Language APIs

PowerIO keeps one canonical naming system across Rust, Python, Julia, and the C
ABI while still using each language's own style.

Verb taxonomy:

- `parse_*`: bytes, paths, or text to `Network`
- `to_*`: `Network` to a new value
- `convert_file`: path to target text convenience
- `write_*`: filesystem side effects only
- `export_*`: handoff to external memory or interface protocols

Do not use Julia `convert` / `convert!` for format conversion. `Base.convert`
means type conversion, and `!` means mutating an argument. PowerIO format
conversion returns new values.

| Concept | Rust | Python | Julia | C ABI |
|---|---|---|---|---|
| Parse path | `parse_file(path)` | `parse_file(path, from_=None)` | `parse_file(path; from=nothing)` | `pio_parse_file` |
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

`pio_export_arrow` keeps `export` because it fills Arrow C Data Interface
structs with release callbacks. It is not an owned string or handle return like
the `to_*` functions.

Aliases can exist where they improve interoperability with downstream parser
surfaces, but docs should present the table above as the canonical API.
