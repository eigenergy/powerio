# ADR 0001: A compiler package wraps distinct model families

Status: accepted (v1 scaffolding). Supersedes nothing.

## Context

PowerIO parses many transmission and distribution formats and converts between
them. As more formats and consumers arrive (BMOPF, PowerMCP, ExaModelsPower,
MG-RAVENS), two pressures appear:

- the temptation to grow one universal `Network` struct that carries every field
  every format might have, balanced and multiconductor alike;
- the need for a single object that records provenance, diagnostics, and lowering
  history, so a downstream tool can trust how a source was interpreted.

The two electrical models already in the tree are structurally different.
`powerio::Network` is scalar positive sequence; `powerio_dist::DistNetwork` is
wire-coordinate with terminals, grounding, and per-conductor matrices. They share
conventions, not types, and `powerio-dist` does not depend on `powerio`.

## Decision

1. Keep the two model families distinct, named explicitly by `BalancedNetwork`
   (alias of `Network`) and `MulticonductorNetwork` (alias of `DistNetwork`),
   without merging them and without renaming the existing structs. (These aliases
   were added by PR #143; this PR builds the envelope on top of them.)

2. Add a compiler-package envelope, `powerio_pkg::CompilerPackage`, that wraps
   exactly one typed payload at a time. It carries an explicit `model_kind`, a
   tagged `ModelPayload`, producer and origin metadata, source maps, structured
   diagnostics, a validation summary, and lowering history. It serializes to
   `.pio.json`. The package lives in a new crate that depends on both IR crates,
   because neither IR crate should depend on the other.

3. `model_kind` is authoritative and never inferred from which payload field is
   present. The payload is additionally self-describing (tagged by `kind`);
   consistency between the two is asserted.

4. Parser bookkeeping that cannot or should not live in the IR payload is lifted
   into the envelope. `DistNetwork::source` (retained text) becomes
   `Origin::File { retained_source }`; `DistNetwork::defaulted` becomes source-map
   entries with `mapping_kind = defaulted`. Both are `#[serde(skip)]` in the
   payload: `source` is an `Arc<String>` that would otherwise pull in serde's
   `rc` feature (the same reason `Network::source` is skipped), and `defaulted`
   holds `&'static str`, which cannot `Deserialize`.

## Consequences

- Making `DistNetwork` serde-serializable is additive: no parser, reader, or
  writer changes, and no existing test changes, because the two non-serde fields
  are skipped.
- A consumer that needs both balanced and multiconductor models holds a
  `CompilerPackage` and matches on `model_kind`, rather than reaching into a
  union struct.
- Binary `.pio` is deferred until the JSON schema stabilizes; the package is JSON
  only for now.
- The top-level `Network` identifier is not repurposed as the envelope yet,
  because `powerio::Network` still means the balanced model and repurposing it
  would break callers. `CompilerPackage` is the transitional public name.
- New model families (dynamics, time series, results) get their own package
  families later; they do not enlarge the static-grid payload.
