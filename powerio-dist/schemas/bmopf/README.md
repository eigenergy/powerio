# BMOPF schema archive

These exact schema documents describe the BMOPF profiles supported by PowerIO.
The archive is available without fetching an upstream branch. `manifest.json`
records each document's SHA-256, embedded identifier, source revision, license,
and associated review discussion. Files retain their upstream bytes.

- `0.1.0` is the historical baseline, with its versioned schema identifier.
- `0.2.0` is a proposal. Supporting it in PowerIO does not establish Task Force
  acceptance. Changes to the proposal require a reviewed archive update.

The schema documents are copyright the IEEE PES Task Force on Benchmarking
Multiconductor OPF for Distribution Systems and contributors, under CC BY 4.0;
see `LICENSE`. Source and review contributions include Frederik Geth and Matt
Deakin; Samuel Talkington maintains the versioned integration and archive.
Attribution does not imply endorsement of PowerIO or approval of a proposal.
The repository's code license does not replace the schema license.

Upstream work and its contribution process are documented in
[dsopt-schema](https://github.com/distribution-system-opt/dsopt-schema) and the
[Task Force contribution guide](https://github.com/distribution-system-opt/math-and-data-model-specifications/blob/main/docs/src/contributing.md).

The unversioned historical test fixture remains under `tests/data/dist/bmopf`
for source-fidelity tests. Its validation rules match this archive's `0.1.0`;
its embedded identifier differs.
