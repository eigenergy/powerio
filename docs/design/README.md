# Candidate PowerIO 1.0 design record

These four documents record the design that led to the PowerIO 0.10 beta and
the candidate 1.0 API now shipped for stabilization in 0.11. They are dated
design records, not current API authority: the released source, tests, the
[language API guide](../src/languages.md), the
[migration guide](../src/migration-0.11.md), and the changelog state the shipped
API. Read them for the reasoning behind a public name or meaning.

1. [Terminology](v1-terminology.md) fixes the public words and names.
2. [Rationale](v1-rationale.md) explains the alternatives and why the selected
   API won.
3. [Ontology](v1-ontology.md) fixes the public value types, source profiles,
   and allowed transformations.
4. [Architecture](v1-architecture.md) fixes ownership, schema, instance,
   solution, matrix, writer, crate, ABI, and binding semantics.

The record was written against the 0.10 beta. Where it names a beta operation
or type that 0.11 removed, the migration guide lists the replacement. The
compiling prototype crates and the issue audit that accompanied the record were
working evidence for that period and are kept only in the repository history.

Later focused design reviews live beside that 1.0 record:

- [Multiconductor LinDist3Flow](lindist3flow-multiconductor.md) records the
  pre-merge acceptance review, supported physical slice, crate boundaries and
  external OpenDSS validation for issue #151.
- [BMOPF 0.2.0](bmopf-0.2.0-notes.md) records what the Task Force had
  accepted, what remained proposal, and the release plan for BMOPF 0.2.0 in
  PowerIO 0.11.
