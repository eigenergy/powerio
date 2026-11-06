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
