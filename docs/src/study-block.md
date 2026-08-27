# Study blocks (0.9, upgrade only)

A `study` block stored cumulative edits to a released 0.9 `.pio.json`
package. The 1.0 stored module carries no study block: the upgrade reader
refuses a 0.9 package whose study is nonempty and directs the producer to
materialize a revision first, so this chapter documents the 0.9 lineage the
upgrade specification refers to.

Each study commit applies after every preceding commit. Materializing commit
`k` applies commits 0 through `k` to a fresh copy of the base payload. This
avoids numerical drift from repeatedly modifying an already materialized
network.

## Study commits and operating points

Operating points and study commits have different update rules:

- An operating point independently overwrites fields on existing payload rows.
  Materializing point `k` ignores every other point.
- A study commit applies deltas and field updates after all preceding commits.
  A demand delta can address a bus that has no load row.

The two blocks share `ElementRef` identity resolution. They do not share time
axis or accumulation semantics.

## Document shape

```json
"study": {
  "label": "congestion sweep",
  "created_at": "2026-07-03T18:20:00Z",
  "commits": [
    { "edits": [
        { "kind": "demand_delta",
          "bus": { "table": "buses", "source_uid": "buses:1" }, "p_mw": 50.0 },
        { "kind": "rating_delta",
          "branch": { "table": "branches", "source_uid": "branches:2" },
          "delta_mva": -210.0 }
    ]}
  ],
  "app": { "tellegen": { "formulation": "dcopf", "options": { "shed": false } } }
}
```

`StudyBlock` contains optional `label`, `author`, `created_at`, and
`base_operating_point` fields; an ordered `commits` array; an `app` map; and
free form `metadata`. `base_operating_point` selects a snapshot from the
package's operating point series before applying the first commit.

Each `StudyCommit` contains optional `label` and `created_at` fields, its
`edits`, and free form `metadata`. Packages without a `study` block retain their
existing behavior and metadata schema version.

## Edit kinds

`StudyEdit` supports these tagged variants:

- `demand_delta { bus, p_mw, q_mvar? }` adds demand at a bus, including a bus
  with no load rows.
- `rating_delta { branch, delta_mva }` adds to a branch thermal rating.
- `set_fields { update }` wraps an `ElementUpdate` and overwrites fields on one
  payload row.
- An unknown `kind` is retained during parsing. Materialization returns an
  error rather than ignoring the edit.

References resolve by identity before row number, as operating point updates
do. Producers should set `source_uid`; `row` remains a compatibility fallback
and consistency check. Package validation resolves every reference in every
commit without modifying the package.

## Materialization

The 0.9 materialization (`materialize_study_commit(k)`) applied commits 0 through `k` to
a copy of the model payload. It removes the study and operating point blocks
from the result and records the operation in `lowering_history`. The returned
package is static and can be converted, projected into matrices, or passed to a
problem instance builder.

A demand delta is divided among the in service load rows at its bus in
proportion to their existing demand. This preserves each load's share and power
factor. If the bus has no in service load or its total demand is zero,
materialization appends a synthetic load with UID `study:load:{bus_uid}` and
marks it as synthetic in metadata.

Study materialization accepts balanced model payloads. A multiconductor payload
returns a validation error.

## Application metadata

The `app` map stores application specific data that PowerIO retains without
validation. A solver can keep its formulation and options under a private key,
for example `app["tellegen"]`. Consumers that only need the network states can
ignore this map.

## Row identity

Study edits address payload rows by UID:

- Parsing the same source bytes produces the same UIDs.
- Generated UIDs use `{table}:{row}` when the source format has no UID.
- A source UID is retained rather than replaced by a generated value.
- UIDs survive a network JSON round trip.

`ensure_payload_uids(&mut Network)` adds missing UIDs before a consumer builds
its own edit state. Use UIDs for stored references and row order for display.

The 0.9 package APIs that read and materialized study blocks are gone at
1.0; the rules above remain as the record of what those releases wrote. The
block's history is tracked in
[#181](https://github.com/eigenergy/powerio/issues/181).
