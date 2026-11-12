# BMOPF schema archive

This exact schema document describes the one BMOPF PowerIO reads and writes.
The archive is available without fetching an upstream branch. `manifest.json`
records the document's SHA-256, embedded identifier, source revision, license,
and associated review discussion. The file retains its upstream bytes.

The schema is the one proposed in
[dsopt-schema#2](https://github.com/distribution-system-opt/dsopt-schema/pull/2),
vendored from the Task Force repository
[distribution-system-opt/dsopt-schema](https://github.com/distribution-system-opt/dsopt-schema)
at commit `6796394`. Supporting it in PowerIO does not establish Task Force
acceptance. Once the Task Force ratifies it, the pin moves to the release tag
its `$id` names. Changes to the proposal require a reviewed archive update.

The schema documents are copyright the IEEE PES Task Force on Benchmarking
Multiconductor OPF for Distribution Systems and contributors, under CC BY 4.0;
see `LICENSE`. Source and review contributions include Frederik Geth and Matt
Deakin; Samuel Talkington maintains the versioned integration and archive.
Attribution does not imply endorsement of PowerIO or approval of a proposal.
The repository's code license does not replace the schema license.

Upstream work and its contribution process are documented in
[dsopt-schema](https://github.com/distribution-system-opt/dsopt-schema) and the
[Task Force contribution guide](https://github.com/distribution-system-opt/math-and-data-model-specifications/blob/main/docs/src/contributing.md).

PowerIO still reads documents written against older BMOPF drafts (including
the early draft PowerIO 0.11 wrote) and reads them as BMOPF, so those schemas
are not shipped. The historical first draft schema remains under
`tests/data/dist/bmopf` as the test oracle for that older input.
