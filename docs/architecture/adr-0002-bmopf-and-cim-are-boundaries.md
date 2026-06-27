# ADR 0002: BMOPF, CIM, and RAVENS are boundaries, not the internal IR

Status: accepted (v1 scaffolding).

## Context

BMOPF JSON is an important semantic target: it is the IEEE PES Task Force
benchmark format for up-to-four-wire distribution OPF, and its data model is the
most explicit of the distribution formats PowerIO reads. It would be tempting to
adopt the BMOPF JSON shape (or a CIM/MG-RAVENS graph) as PowerIO's internal
representation, since the benchmark and interoperability work targets them
directly.

Doing so would couple the internal IR to one external format's versioning, its
open discrepancies (the normative draft and BMOPFTools disagree in several
places; see the reconciliation table), and its benchmark-specific concerns
(slack synthesis, finding-code vocabulary, augmentation). It would also pull
CIM's terminal-expansion model into the core, which the project explicitly does
not want to implement internally.

## Decision

- `MulticonductorNetwork` is the internal distribution IR. BMOPF JSON is a
  frontend (reader) and an emission target (writer), reconciled field by field in
  `bmopf-powerio-reconciliation.md`.
- `MulticonductorNetwork -> BMOPF JSON` is treated as an emission/lowering pass
  with diagnostics, not plain serialization. Terminal naming, neutral detection,
  grounding, transformer subtype selection, switch extraction, capacitor typing,
  and defaulted-field handling are pass concerns that produce findings.
- Where the normative draft and BMOPFTools disagree, PowerIO records the
  divergence with a `PARTNER.BMOPF.*` diagnostic and keeps a reversible path,
  rather than hard-coding one interpretation.
- CIM / MG-RAVENS is an export boundary (`BalancedNetwork -> RAVENS JSON`), not
  the internal IR. CIM stays external.

## Ownership split

- **PowerIO** owns the OpenDSS parser, default materialization, terminal maps,
  the typed `MulticonductorNetwork`, the BMOPF reader/writer, source maps, and
  structured diagnostics.
- **BMOPFTools** owns schema conformance, the benchmark finding-code vocabulary,
  reports, repair/augmentation manifests (including slack synthesis), and OPF
  benchmark semantics. It consumes BMOPF JSON emitted by PowerIO and keeps its
  own `Dict` schema mirror at its boundary.
- **MG-RAVENS** owns CIM-like API/schema interoperability and external workflow
  exchange.

## Consequences

- PowerIO can track the BMOPF draft and BMOPFTools independently; a change in the
  benchmark format is a change to the reader/writer and the reconciliation table,
  not to the IR.
- Benchmark-readiness logic (slack, augmentation, finding codes specific to
  benchmark publication) does not enter PowerIO; it stays in BMOPFTools.
- Feature parity with BMOPF is pursued through the reconciliation table and
  targeted diagnostics, not by importing the BMOPF schema wholesale.
