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
