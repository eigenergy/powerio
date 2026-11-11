# BMOPF schema archive

This exact schema document describes the one BMOPF version PowerIO writes.
The archive is available without fetching an upstream branch. `manifest.json`
records the document's SHA-256, embedded identifier, source revision, license,
and associated review discussion. The file retains its upstream bytes.

`0.2.0` is the version proposed in
[dsopt-schema#2](https://github.com/distribution-system-opt/dsopt-schema/pull/2),
vendored from its commit `6796394`. Supporting it in PowerIO does not establish
Task Force acceptance. Once the Task Force ratifies it, the pin moves to the
`schema-v0.2.0` tag. Changes to the proposal require a reviewed archive update.

The schema documents are copyright the IEEE PES Task Force on Benchmarking
Multiconductor OPF for Distribution Systems and contributors, under CC BY 4.0;
see `LICENSE`. Source and review contributions include Frederik Geth and Matt
Deakin; Samuel Talkington maintains the versioned integration and archive.
Attribution does not imply endorsement of PowerIO or approval of a proposal.
The repository's code license does not replace the schema license.

Upstream work and its contribution process are documented in
[dsopt-schema](https://github.com/distribution-system-opt/dsopt-schema) and the
[Task Force contribution guide](https://github.com/distribution-system-opt/math-and-data-model-specifications/blob/main/docs/src/contributing.md).

PowerIO still reads documents written against earlier versions (0.1.0, 0.1.1,
and the 0.2.0 draft PowerIO 0.11 wrote) and upgrades them to 0.2.0, so those
schemas are not shipped. The unversioned historical 0.1.0 draft schema remains
under `tests/data/dist/bmopf` as the test oracle for that older input.
