# powerio-archive

Bounded in-memory ZIP indexing and member reads. This crate depends on
`powerio-core` and ZIP support, with no electrical model or format discovery.

`Archive::open` validates the complete index, namespace, local headers, and
declared expansion budgets before any member is decompressed. `Archive::read`
validates the inflated size and CRC before caching bytes. Repeated reads share
one immutable buffer. Input and limits are explicit; no filesystem, environment,
network, or extraction operation is used.

The supported profile is single-volume ZIP32/ZIP64 with UTF-8 paths and stored
or deflated members. Self-extracting prefixes, ambiguous end records in metadata,
encrypted entries, special files, path collisions, and unsafe paths are refused.
Nested containers remain opaque bytes. This crate neither recurses into them nor
selects case files. Index validation does not certify unread payload CRCs.

Names preserve exact UTF-8 bytes with slash separators; Unicode normalization
and OS path lookup are not performed. Lowercased name/ancestor collisions,
file/directory conflicts, Windows device names, forbidden characters and
trailing dots/spaces are refused. Paths are capped at 4096 bytes and 64 segments
(with core's per-segment bound). Export to a filesystem still needs the
destination's own validation.

Defaults cap compressed input and cumulative declared expansion at 64 MiB each,
entries at 4096, and each member's expansion ratio at 200:1. Callers can supply
other positive platform-sized limits and a smaller per-read cap. Cached bytes
cannot exceed the declared expansion cap; transient decoder/index allocations
are additionally bounded by input and entry counts. These caps do not bound
downstream decoded tables, models, or repeated parser work.
