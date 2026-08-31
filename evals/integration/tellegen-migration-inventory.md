# Tellegen migration inventory: PowerIO 0.9 input surface

The consumed 0.9 surface below was measured at Tellegen commit `01bfee8`
(`Merge pull request #82`). Counts are call sites in `crates/`. This record
preserves the measured input inventory. The disposition column records the
current 1.0 candidate rather than an intermediate 0.10 API.

## Dependency set

| 0.9 dependency | 1.0 candidate disposition |
|---|---|
| `powerio = "0.9"` | `powerio = "1"` |
| `powerio-pkg = "0.9"` | crate retired; the stored document is `powerio::stored` (`read_module`/`emit_module`), and released 0.9 packages upgrade one way on read |
| `powerio-dist = "0.9"` | `powerio-dist = "1"` |
| `powerio-prob = "0.9"` | `powerio-prob = "1"` |

## Consumed symbols

| 0.9 symbol (sites) | 1.0 candidate disposition |
|---|---|
| `powerio::parse_str` (64) | retired; `powerio::parse_text(name, text, format)` returns `PioModule<PioValue>` with `value()` and `diagnostics()` accessors; owned typed narrowing is `module.into_typed::<BalancedNetwork>()` |
| `powerio::format::parse_file` (2) | replaced by `powerio::parse_file(path)` |
| `powerio::parse_bytes` (3) | retired; UTF-8 case input uses `parse_text`, while binary or externally owned input uses `parse(Source::from_bytes(name, bytes)?)` |
| `powerio::BalancedNetwork::{from_json, from_json_bytes, in_memory}` (5) | `from_json` and `in_memory` remain; `from_json_bytes` is retired in favor of UTF-8 `from_json` |
| `powerio::BusId`, `BusType::Isolated`, `Winding::new`, `Transformer3W`, `Impedance::new`, `BranchCharging::new`, `Switch::new`, `Storage::new`, `Hvdc::new` (41) | unchanged model vocabulary |
| 0.9 target format selection and output calls (4) | the facade emits with `powerio::emit(&module, format, destination)`; component code uses `powerio_tx::TargetFormat`, `parse_target_format`, and `powerio_tx::emit` |
| `powerio::SourceFormat::Normalized` (1) | unchanged |
| `powerio::IndexedNetwork::new` (1) | unchanged (public derived index view) |
| `powerio::geo::{GeoLayer, GeoApplyReport, CoordsKind, pwd_mercator_to_lonlat}` (6) | the types remain; the transform is `to_lonlat_from_pwd_mercator` |
| `powerio::format::routing::JsonClass`, `classify_json_bytes` (4) | unchanged, with the stored class label renamed `"package"` → `"module"` |
| `powerio::format::powerworld::PwdSubstation` (2) | unchanged |
| `powerio_pkg::ensure_payload_uids` (12) | retired with the crate; version 1 stored modules carry uid row identity natively, and the legacy stamping lives only in the private 0.9 compatibility layer |
| `powerio_pkg::NetworkPackage::{from_json, from_json_bytes, from_balanced, from_multiconductor}` (7) | retired; construct `PioModule` values directly, emit stored documents as `pio-json`, and parse any stored generation through `parse_file` or `parse_text` |
| `powerio_pkg::ModelKind::{Balanced, Multiconductor}` (4) | retired; the module's value kind (`module.value().kind()`) states the family |
| `powerio_pkg::ElementUpdate::new` (1) | retired with the 0.9 study surface; materialize a state on the module surface instead |
| `powerio_dist::parse_str`, `MulticonductorNetwork::default`, `DistGraph`, `dist_target_from_name` (4) | text goes through `powerio::parse_text`; `MulticonductorNetwork::default` and `DistGraph` remain, and target lookup is `powerio_dist::parse_dist_target_format` |
| `powerio_prob::{DcOpfInstance, AcOpfInstance}` (2) | unchanged |

## Historical behavior notes

- Diagnostics arrive as structured records everywhere; code strings are stable; never match on message text.
- A `.pio.json` emitted by 0.9 (`NetworkPackage`) loads through the one way upgrade; 1.0 emits the `powerio.module/1` document, which 0.9 cannot read.
- Wire identity is the `powerio.module/1` schema plus its producing `powerio_version`; the released 0.9 stored lineage is readable only through the module upgrade path.
- DC susceptance sign, uid row identity, element id verbatim rules, and index space conventions are unchanged from 0.9 and restated in the producer receipt.
