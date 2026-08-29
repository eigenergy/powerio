# Tellegen migration inventory for PowerIO 0.10

The consumed 0.9 surface below was measured from an isolated read only clone of eigenergy/tellegen at `01bfee8` (`Merge pull request #82`). Counts are call sites in `crates/`. The shared boundary document `docs/POWERIO_INTEGRATION.md` (untracked working copy) digests to SHA-256 `cecf96ff40669062d381a78a29eb3848776601f27a9f970fa6cda497b2a4f99a`. Tellegen itself is out of scope for this release candidate; this inventory states what its upgrade to 0.10 will touch.

## Dependency set

| 0.9 dependency | 0.10 disposition |
|---|---|
| `powerio = "0.9"` | `powerio = "0.10"` |
| `powerio-pkg = "0.9"` | crate retired; the stored document is `powerio::stored` (`read_module`/`write_module`), and released 0.9 packages upgrade one way on read |
| `powerio-dist = "0.9"` | `powerio-dist = "0.10"` |
| `powerio-prob = "0.9"` | `powerio-prob = "0.10"` |

## Consumed symbols

| 0.9 symbol (sites) | 0.10 disposition |
|---|---|
| `powerio::parse_str` (64) | retired; `powerio::parse(powerio::Source::from_bytes(name, bytes)?)` returns `PioModule<PioValue>`; narrow with `try_into_typed::<BalancedNetwork>` or read `module.value()` |
| `powerio::format::parse_file` (2) | retired test helper; `powerio::parse(powerio::Source::open(path)?)` |
| `powerio::parse_bytes` (3) | the geo layer's own entry where used; case parsing goes through `powerio::parse` |
| `powerio::BalancedNetwork::{from_json, from_json_bytes, in_memory}` (5) | unchanged |
| `powerio::BusId`, `BusType::Isolated`, `Winding::new`, `Transformer3W`, `Impedance::new`, `BranchCharging::new`, `Switch::new`, `Storage::new`, `Hvdc::new` (41) | unchanged model vocabulary |
| `powerio::TargetFormat`, `target_format_from_name`, `write_as` (4) | unchanged; the module level twin is `write_module_as`/`write_module_str` |
| `powerio::SourceFormat::Normalized` (1) | unchanged |
| `powerio::IndexedNetwork::new` (1) | unchanged (public derived index view) |
| `powerio::geo::{GeoLayer, GeoApplyReport, CoordsKind, pwd_mercator_to_lonlat}` (6) | unchanged |
| `powerio::format::routing::JsonClass`, `classify_json_bytes` (4) | unchanged, with the stored class label renamed `"package"` → `"module"` |
| `powerio::format::powerworld::PwdSubstation` (2) | unchanged |
| `powerio_pkg::ensure_payload_uids` (12) | retired with the crate; version 1 stored modules carry uid row identity natively, and the legacy stamping lives only in the private 0.9 compatibility layer |
| `powerio_pkg::NetworkPackage::{from_json, from_json_bytes, from_balanced, from_multiconductor}` (7) | retired; write `PioModule` values through `powerio::stored::write_module`, read any stored generation through `powerio::stored::read_module` |
| `powerio_pkg::ModelKind::{Balanced, Multiconductor}` (4) | retired; the module's value kind (`module.value().kind()`) states the family |
| `powerio_pkg::ElementUpdate::new` (1) | retired with the 0.9 study surface; materialize a state on the module surface instead |
| `powerio_dist::parse_str`, `MulticonductorNetwork::default`, `DistGraph`, `dist_target_from_name` (4) | `parse_str` goes through the one `powerio::parse` family; the rest unchanged |
| `powerio_prob::{DcOpfInstance, AcOpfInstance}` (2) | unchanged |

## Behavior notes for the upgrade

- Diagnostics arrive as structured records everywhere; code strings are stable; never match on message text.
- A `.pio.json` written by 0.9 (`NetworkPackage`) loads through the one way upgrade; 0.10 writes the version 1 module document, which 0.9 cannot read.
- Wire identity: `powerio_version` 0.10.x documents; the generic gate reads 0.10.x, and the released 0.9 stored lineage is readable only through the module upgrade path.
- DC susceptance sign, uid row identity, element id verbatim rules, and index space conventions are unchanged from 0.9 and restated in the producer receipt.
