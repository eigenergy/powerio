# Reconciliation with PR #143

The compiler-package work was first drafted before PR #143 ("rich data model
parity") landed. #143 changed `Network` and `DistNetwork` and introduced the v1
model-family aliases, so the package work was rebuilt on top of it: salvage the
unique parts, drop anything #143 already owns, and re-fit the payload to the
richer model. This note records that reconciliation.

The pre-#143 draft is preserved on the branch `pkg/pio-json-envelope-pre143` as a
single WIP commit. It is salvage material, not a branch to merge.

## 1. What PR #143 already implemented

- The v1 model-family aliases, in Rust: `powerio::BalancedNetwork = Network`
  (`powerio/src/network.rs`) and `powerio_dist::MulticonductorNetwork =
  DistNetwork` (`powerio-dist/src/model.rs`), both re-exported from their crate
  roots.
- Richer balanced model (`Network`): typed branch terminal charging/admittance,
  switches, branch current ratings, branch solution values, HVDC cost, storage
  current rating, plus richer PowerModels / PSS/E / pandapower / PyPSA field
  mapping.
- Richer multiconductor model (`DistNetwork`): `DistLoad::voltage_model` with a
  new `DistLoadVoltageModel` enum (`ConstantPower`, `ConstantCurrent`,
  `ConstantImpedance`, `Zip`, `Exponential`), plus richer BMOPF / PMD / OpenDSS
  mapping.
- A rich validation tier under `benchmarks/` (fixtures, the PowerModels oracle,
  opt-in local corpus reports).

What #143 did **not** add: serde on `DistNetwork` (it still derives only `Clone,
Debug`), and any compiler package / `.pio.json`.

## 2. WIP changes that were duplicates, intentionally dropped

- `pub type BalancedNetwork = Network;` (network.rs) — #143 owns it.
- `pub type MulticonductorNetwork = DistNetwork;` (model.rs) — #143 owns it.

These two alias lines were the only direct textual collision; reintroducing them
would be a duplicate-definition compile error. They are not carried forward.

The WIP also added Python aliases (`powerio.BalancedNetwork`,
`powerio.dist.MulticonductorNetwork`) and migration-table rows for them. #143
did not add the Python aliases, but they are a naming-migration concern, not an
envelope concern. To keep this PR narrow, the Python/Julia/C ABI v1 aliases are
deferred to a separate naming PR; only the Rust aliases (already #143's) exist
today. See the deferred section.

## 3. WIP changes that are unique, carried forward

- The `powerio-pkg` crate: `CompilerPackage` (the `.pio.json` envelope) and its
  supporting types (`ModelKind`, `ModelPayload`, `Producer`, `Origin`,
  `SourceDescriptor`, `SourceRef`, `SourceMapEntry`, `MappingKind`, `Confidence`,
  `StructuredDiagnostic`, `DiagnosticSeverity`, `DiagnosticStage`,
  `DiagnosticCode`, `ValidationSummary`, `LoweringRecord`, `ObjectSummary`,
  `DerivedMetadata`). It compiles unchanged against the post-#143 models, because
  the package serializes whole networks and the summary helpers touch only fields
  #143 kept.
- serde derives on `DistNetwork` and its element types, with `source` and
  `defaulted` skipped (the two fields that block `Deserialize`). Re-applied to the
  post-#143 model, including the new `DistLoadVoltageModel` enum. This is not a
  duplicate of #143; #143 left the dist model without serde.
- The architecture docs under `docs/architecture/`, updated for #143.
- The package round-trip tests, plus a new test
  (`post143_load_voltage_model_survives_package_roundtrip`).

## 4. Payload-schema docs and tests updated because #143 changed the models

Because `.pio.json` embeds a serde snapshot of the live Rust IR, #143's field
additions flow into the payload automatically. The following were updated to
reflect that:

- `bmopf-powerio-reconciliation.md`: the load row now records that BMOPF load
  models map to the typed `DistLoad::voltage_model` (`ConstantPower` /
  `ConstantCurrent` / `ConstantImpedance` / `Zip` / `Exponential`) rather than
  landing in `extras`. That gap is closed by #143.
- A new test, `post143_load_voltage_model_survives_package_roundtrip`, builds a
  load with a ZIP voltage model, packages it, and asserts the model round-trips
  through `.pio.json` (including its internal `"model": "zip"` tag).
- `pio-json-schema.md` (new) documents the policy: the envelope is versioned, but
  the nested payloads are experimental IR snapshots that grow with the IR (as
  they did with #143).

## 5. Deferred

- CLI `package` / `inspect-package` commands (next PR).
- BMOPF dropped-field diagnostics, the multiconductor-to-balanced lowering pass,
  MCP package transport, and C ABI package handles.
- Python / Julia / C ABI v1 name aliases (Rust aliases are #143's; the rest is a
  separate naming PR).
- A separately versioned, stable `.pio.json` payload schema. Until then the
  payloads stay experimental (see `pio-json-schema.md`).
- Distribution generator per-phase cost: `DistGenerator::cost` is still
  `Option<f64>` (scalar) after #143; the per-phase reconciliation gap stands.
- Precise JSON-pointer `element_path` in lifted source maps (best-effort locator
  for now).
