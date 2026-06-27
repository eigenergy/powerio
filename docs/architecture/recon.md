# Reconnaissance: compiler-IR architecture work

> Note: this is the original first-pass reconnaissance, written before PR #143
> ("rich data model parity") landed. It describes the pre-#143 repository and the
> draft change set. PR #143 since introduced the `BalancedNetwork` /
> `MulticonductorNetwork` aliases and enlarged both models. For the landed
> reconciliation and what this PR actually carries, see
> `pr143-reconciliation.md`. This note is kept as a historical record.

A record of what the repository looked like before the v1 compiler-IR work and
what the first draft change set did. It pairs with the design docs in this
directory and the BMOPF reconciliation table.

## What exists today

Two concrete electrical models, in separate crates that share conventions but
not types:

- `powerio::Network` (`powerio/src/network.rs`): the scalar positive-sequence
  transmission model. Every electrical quantity is one `f64`; there is no phase
  or conductor dimension. Already derives `Serialize`/`Deserialize`; the
  retained source text is `#[serde(skip)]` so a JSON round trip drops it (kept
  out partly to avoid serde's `rc` feature). Carries a repair-oriented
  `Diagnostic` struct (`element`, `field`, `old`, `new`, `reason`) and a
  `SourceFormat` enum.
- `powerio_dist::DistNetwork` (`powerio-dist/src/model.rs`): the wire-coordinate
  distribution model. String bus ids, ordered string terminals per bus, explicit
  grounding, terminal maps on every element, SI units and radians, retained
  source for a byte-exact echo, `defaulted` provenance for materialized format
  defaults, untyped-object preservation. Already implements the four BMOPF
  voltage-bound families on the bus (`v_min`/`v_max`, `vpn_*`, `vpp_*`,
  `vsym_*`). Before this change it did **not** derive serde.

Diagnostics on both surfaces are plain `Vec<String>` warnings
(`DistNetwork::warnings`, `Conversion::warnings`). There are no stable codes,
severities, or stages.

There was no compiler-package envelope, no `.pio.json`, and no
`docs/architecture/` or `docs/adr/` directory. The repository had just finished
a `Case -> Network` / `DistCase -> DistNetwork` rename (the `Case`/`DistCase`
aliases are scheduled for removal in 0.4.0). Workspace version is `0.3.3`,
edition 2024.

The naming spread across binding surfaces (relevant to the migration table):

| surface | balanced | multiconductor |
|---|---|---|
| Rust | `powerio::Network` | `powerio_dist::DistNetwork` |
| C ABI | `PioNetwork` | `PioDistNetwork` |
| PyO3 | `PyNetwork` | `PyDistNetwork` (Python `_DistNetwork`) |
| Python pkg | `powerio.Network` | `powerio.dist.DistNetwork` |

## Key findings

1. The distribution model is already close to BMOPF semantics. The gap is not
   the core wire-coordinate layout; it is provenance, structured diagnostics,
   and a handful of field-level mismatches (per-phase generator cost flattened
   to a scalar, scalar bus `v_min`/`v_max` vs BMOPF's per-terminal vectors,
   BMOPF's four typed transformer subtypes carried as an `extras` tag on a
   generic OpenDSS winding model). See `bmopf-powerio-reconciliation.md`.
2. The normative BMOPF draft and the executable BMOPFTools interpretation
   disagree in several places (scalar vs array `v_min`, `vsym` vs `vpos`,
   transformer subtype set, delta-capacitor admittance). These must be
   documented and handled with diagnostics, not hard-coded irreversibly.
3. `powerio-dist` does not depend on `powerio`. A payload enum holding both
   networks therefore belongs in a new crate that sits above both, not inside
   either.
4. `DistNetwork` had two fields that block serde `Deserialize`: `defaulted`
   (`Vec<&'static str>`) and `source` (`Arc<String>`, which would need serde's
   `rc` feature). Both are parser bookkeeping that the compiler package
   re-surfaces through provenance, so both are skipped in the payload.

## What this change set does

- Adds `powerio::BalancedNetwork` (alias of `Network`) and
  `powerio_dist::MulticonductorNetwork` (alias of `DistNetwork`). No struct is
  renamed; the historical names stay. (Phase 3.)
- Makes `DistNetwork` and its element types serde-serializable, with `source`
  and `defaulted` marked `#[serde(skip)]`. Purely additive; no parser behavior
  changes and no existing test changes.
- Adds the `powerio-pkg` crate: the `CompilerPackage` (`.pio.json`) envelope
  with explicit `ModelKind`, a tagged `ModelPayload`, `Producer`, `Origin`,
  `SourceDescriptor`/`SourceRef`/`SourceMapEntry`, `StructuredDiagnostic` with
  `DiagnosticSeverity`/`DiagnosticStage`/`DiagnosticCode`, `ValidationSummary`,
  `LoweringRecord`, and `ObjectSummary`. JSON only; binary `.pio` is out of
  scope. (Phase 2.)
- Lifts `DistNetwork` parse warnings into structured diagnostics and `defaulted`
  fields into source maps (`mapping_kind = defaulted`) when building a package,
  so the provenance that the skipped fields held is preserved in the envelope.
- Adds `docs/architecture/` with this note, `compiler-ir.md`,
  `bmopf-powerio-reconciliation.md`, two ADRs, and `migration-v1.md`.

## What this change set does not do

- It does not add or change any parser, reader, or writer.
- It does not rename any existing public type or remove any name.
- It does not wire the package into the CLI, C ABI, Python, or MCP surfaces;
  those adopt it in later PRs (see the next-steps section of the final report
  and the adapter notes in `compiler-ir.md`).
- It does not implement the multiconductor-to-balanced lowering pass; only the
  `LoweringRecord` shape that such a pass will produce.
