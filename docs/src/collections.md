# Input collections

`Collection` acquires a ZIP or folder once and exposes independent selections.
Inspection suggests inputs without parsing electrical values. A file can be both
an independent input and a companion of several other inputs. Conventional names
such as `Master.dss` do not hide alternative masters.

```rust,ignore
use powerio::{Collection, CollectionLimits, CollectionSelection, ParseOptions};
let collection = Collection::from_directory("study", CollectionLimits::default())?;
for candidate in collection.inspect()? {
    println!("{:?}", candidate);
}
let selection = CollectionSelection::file("masters/weekday.dss").format("dss");
let result = collection.parse(&selection, &ParseOptions::default())?;
println!("observed dependencies: {:?}", result.dependencies);
# Ok::<(), powerio::Error>(())
```

A selection's `primary` is relative to its `root`. The default root is the
collection root, so a master under `masters/` can refer to `../shared/`.
A narrower root confines references. Each selection starts new parser state and
new dependency observations. Standalone formats receive only their selected
file. File names are portable, case sensitive, and never rewritten.

ZIP acquisition verifies all member sizes and CRCs under explicit input,
expansion, count, and ratio limits. Folder acquisition uses the core confined
reader, then retains a memory snapshot. Neither selection can subsequently read
unsupplied host files. Nested transport ZIPs remain opaque. An explicit `cgmes`
format sends a native profile ZIP intact to its existing adapter.

JSON inspection has per-file and cumulative probe budgets. A probe limit or
unknown format leaves the input available for explicit selection. `selectable`
means the suggested selection can be submitted; it does not certify the input,
its dependencies, or solver suitability. `CollectionResult::incomplete` reports
known missing-include warnings; false is not proof of study completeness.
Electrical diagnostics remain on the module.

PowerIO IR candidates use `deserialize`. Co-location never synthesizes a time
series or scenario set. A parser's existing internal time/scenario value stays
intact. Call `parse` independently for each selection and retain both successes
and errors when processing several inputs.

`original_zip()` returns original container bytes. Source-format emission has
the existing adapter's replay scope: OpenDSS retains companions, including unused
ones within the selected root. It is not a minimal project exporter. IR omits
retained source bytes as before.

Dataset discovery recognizes `network.csv` (PyPSA) and `bus_data.parquet`
(GridFM). Select the candidate's root with no primary and its explicit format.
Nested datasets remain separate, including during retained-source replay.
GridFM candidates remain visible with `unsupported-build` when the feature is
disabled. Missing required companion tables produce an `incomplete` candidate.

Use `CollectionSelection::group(format, members)` for `cgmes`, `cim-dist`,
or `goc3-json`. Members are relative to the selected root and only those
members reach the adapter. GO problem/solution roles are validated by the
existing reader; discovery never pairs them automatically. CIM members must be
top-level XML files under their selected root. CGMES also accepts explicitly
selected profile ZIPs, with its existing validation and echo behavior.

Select a `.sinx` primary with `sincal-balanced` or `sincal-multiconductor` and
pass the native options through `ParseOptions`. The container reaches SINCAL
intact; outer collection members cannot satisfy its internal references.

## Command line

```sh
powerio collection inspect study.zip
powerio collection parse study.zip --entry masters/day.dss --entry masters/night.dss --from dss -o modules
powerio collection parse datasets --root day --from pypsa-csv
powerio collection parse study.zip --root go --member problem.json --member solution.json --from goc3-json
```

Inspection and parse reports are JSON. Batch results use stable positional IDs
(`case-0001`, etc.) in the requested entry order. Every entry is attempted;
errors and incomplete dependencies produce a nonzero exit after the full report.
`--output` serializes successful modules to those IDs with `.pio.json` suffixes.
Native SINCAL selection flags are accepted with the explicit native profile.

## Python

```python
from powerio import Collection
with Collection.open("study.zip") as collection:
    candidates = collection.inspect()["candidates"]
    result = collection.parse(candidates[0]["selection"])
module = result.module  # still valid after closing the collection
```

`from_zip(bytes)` and `from_files({path: bytes})` use the same inventory and
selection behavior. `max_bytes` and `max_files` bound admission before copying
input payloads. Native SINCAL options can be passed to `parse`.

## C and Julia

C callers own a `PioCollection` from `pio_collection_open`,
`pio_collection_from_zip`, or `pio_collection_from_files`, with explicit byte
and file limits. `pio_collection_inspect` returns metadata JSON. Pass a selection
JSON object to `pio_collection_parse` to obtain the usual typed `PioModule`;
its optional report contains observed dependencies and the incomplete flag.
Release returned strings and handles with their matching functions. Closing the
collection does not invalidate parsed modules. Existing typed parser options
remain available, but collection parsing refuses host acquisition roots.

The companion Julia API uses `InputCollection(path)`, `InputCollection(bytes)`
for ZIP bytes, or `InputCollection(Dict(path => bytes))`. `inspect_collection`
returns metadata; `parse(collection, selection)` returns `CollectionResult`
with a typed `PioModule{T}`. `close(collection)` releases its handle and `copy`
retains an independent handle. These additions change no existing C ABI layout and do not serialize
electrical values to provide typed access.

## Explicit nested ZIPs and session budgets

`collection.open_nested("child.zip")` opens exactly one child. Discovery never
recurses automatically. The child has its own file namespace, so it cannot find
a missing dependency in its parent or a sibling. The default is two ZIP decoding
layers, including a native `.sinx` or CGMES ZIP decoded by its adapter. A memory
folder starts at zero layers; opening the outer transport ZIP consumes one.
Native interpretation stays with the existing adapter, with a bounded payload
verification pass before delegation.

Cloned handles and all descendants share monotonic entry and expansion
counters for the archive content admitted into the session: the acquired tree
plus every nested child opened from it. The byte/file caps cover that content
for the whole session rather than resetting in each child, so reopening a
child counts again; a child that fails verification retains nothing and
releases its reservation. `work_bytes` (default 256 MiB) bounds one operation:
the scoped parser inputs, the decompression a native container's verification
performs, and a nested open's container and declared expansion, charged
conservatively. Every operation starts from zero, so parsing the same
selection repeatedly, as a batch run or a converter session does, never fails
because of earlier operations. It is not a measure of model/Arrow memory or a
replacement for parser limits. Set `CollectionLimits` before acquisition to
tighten either bound.

`Cancellation` can be supplied to `from_zip_cancellable` and obtained through
`cancellation()`. It is checked during discovery and between decompressor reads.
Already returned modules remain valid. Synchronous electrical parsing retains
its existing bounds; browser callers can terminate the worker to interrupt it.
The initial Python/C/Julia APIs retain their bounded defaults; nested inspection
is exposed first through the Rust collection API.

## Manifests and source export

A version 1 manifest records explicit selections without becoming an electrical
`PioValue`. For example:

```json
{
  "schema": "powerio-collection",
  "version": 1,
  "cases": [{
    "id": "weekday",
    "selection": {"primary": "masters/day.dss", "format": "dss"},
    "attachments": ["notes.txt"]
  }]
}
```

`read_manifest` accepts at most 1 MiB and the host inspection budget. It rejects
unknown fields, duplicate IDs, escaping paths, unsupported identity/version, and
excessive case/member lists. It checks ordinary selected sources and attachment
existence without electrical parsing. A case can name a `containers` array of
explicit child ZIP members; those children are acquired only by
`parse_manifest_case(manifest, id)`, under the same session limits. Optional
`reader_options` records SINCAL variant, snapshot, acquired-table name, and the
multiconductor inactive-control assumption. It cannot choose a host acquisition
root or raise resource limits. Attachment associations remain metadata.

`source_bundle(selection, scope)` requires an explicit export policy:

- `SelectedMembers` exports only the primary or named document group. It makes
  no claim that a DSS primary alone is a complete editable project.
- `Project` exports every file inside the selection's permitted root, including
  unused companions and other cases in that root. Choose a narrower root when
  the project has one; parent-relative references must still fit that scope.
- `ExplicitMembers(names)` exports exactly the listed root-relative files.
  Observed dependency names can inform this list, but do not certify completeness.

A bundle shares exact leaf bytes and owns them independently of the collection.
`deterministic_zip(ArchiveLimits)` produces a **new** archive with sorted names,
fixed metadata and stored payloads, bounded including ZIP overhead. It does not
rewrite references or claim original-container fidelity. `original_zip()` is
still the distinct exact-byte download of the acquired archive, if retained.
The writer also accepts explicit named output files for converted-case packaging.

`origin()` and `selection_origin()` separate the original collection name,
container-member chain, and selected root/primary/group into structured fields.
They never parse punctuation in a display name. These metadata are held outside
network values; PowerIO IR still omits retained source payloads. Reproducible
selection metadata alone is not a source-preserving IR document.
