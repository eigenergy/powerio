# The study block

> **Status: design.** Nothing in this chapter ships in 0.5.x. The tracking
> issues are linked at the end.

Interactive tools hold a base case plus an ordered log of edits, and replay the
log to reconstruct the current operating point. tellegen's `Study` works this
way: every commit re-solves from a fresh copy of the base, so the state is
always the base plus the whole log, never accumulated drift. That object has
no on disk form today. This chapter specifies one: an additive `study` block in
the `.pio.json` envelope, so a saved study is an ordinary package that any
PowerIO consumer can validate, replay, materialize, and convert.

## Why not operating points

The package already carries `operating_points`, and the two mechanisms look
similar from a distance. They differ in both axes that matter:

- an `ElementUpdate` overwrites fields on one existing payload row, while an
  interactive edit is a delta, and the most common one (add demand at a bus) is
  legal at a bus with no load rows at all;
- operating points are independent overlays on the base (materializing point k
  ignores point k−1), while commits are cumulative (state k is the base plus
  commits 0 through k).

Retrofitting deltas and a cumulative axis onto the series would change a
contract GO Challenge 3 consumers already rely on. The study block is its own
envelope field and reuses the operating point machinery where it fits: element
addressing and identity resolution through `ElementRef`.

## Shape

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
          "delta_mw": -210.0 }
    ]}
  ],
  "app": { "tellegen": { "formulation": "dcopf", "options": { "shed": false } } }
}
```

`StudyBlock` carries optional `label`, `author`, `created_at`, an optional
`base_operating_point` (a study over a materialized snapshot of this package's
series), the ordered `commits`, an `app` map, and free `metadata`. Each
`StudyCommit` carries optional `label` and `created_at`, its `edits`, and
`metadata`. The block is additive: packages without one are unchanged, and the
envelope version does not move, the same rule `operating_points` landed under.

## The edit vocabulary

`StudyEdit` is a tagged enum that grows additively:

- `demand_delta { bus, p_mw, q_mvar? }` adds to the total demand at a bus.
  Bus level on purpose: it is defined at a bus with zero load rows.
- `rating_delta { branch, delta_mw }` adds to a branch thermal rating.
- `set_fields { update }` wraps an `ElementUpdate` for absolute overwrites of
  one row, the escape hatch for future edits that are naturally absolute
  (service status, setpoints).
- An unrecognized `kind` is preserved verbatim on read and refused loudly at
  materialization. Reading never fails on an unknown edit; silently dropping
  one would silently change the operating point.

Element references are `ElementRef`s resolved identity first, exactly as in
operating point updates. Producers should write `source_uid` and let `row` be
the legacy fallback. Package validation dry runs every reference in every
commit, mirroring the operating point identity check.

## Materialization

`NetworkPackage::materialize_study_commit(k)` folds commits 0 through k into
cumulative deltas, applies them to a clone of the payload, clears the block,
and records the transformation in `lowering_history`. The result is a static
package: convert it, build matrices from it, or hand it to a solver.

Lowering `demand_delta` to concrete load rows is the one semantic decision.
The delta distributes proportionally across the in service load rows at the
bus, preserving each load's share and power factor. At a bus with no in
service load (or zero total demand) a synthetic load row is appended with
uid `study:load:{bus_uid}` and metadata marking it synthetic.

The v1 block is defined for balanced payloads; a study on a multiconductor
payload is a clean validation error until there is a consumer.

## The app namespace

A study is replayed by a solver, and solver vocabulary (formulation names,
options) is not PowerIO's to define or validate. The `app` map gives each tool
a namespace that PowerIO round trips verbatim: tellegen keeps its formulation
and solve options under `app["tellegen"]`. A consumer that only wants the
network states ignores the map entirely.

## Identity

Edits address rows by payload `uid`, so the uid contract tightens from
implementation detail to public guarantee:

- parsing the same bytes yields the same uids;
- synthesized uids are `{table}:{row}` at package build, and source defined
  uids are never overwritten;
- uids survive the network JSON round trip.

`ensure_payload_uids` becomes public so a consumer can stamp uids on a parsed
network before building its own edit state, instead of inventing positional
ids. Consumers should key interactive state by uid and treat row order as a
display concern.

## API surface

- `NetworkPackage::study()`, `with_study`, `clear_study`,
  `materialize_study_commit(k)`;
- `powerio study materialize --commit k --to <format>` in the CLI;
- C ABI: `pio_package_study_json` and `pio_package_materialize_study_commit`,
  additive symbols beside the operating point calls, no ABI bump.

Tracking issues: [#181](https://github.com/eigenergy/powerio/issues/181)
(block, materialization, uid contract),
[#185](https://github.com/eigenergy/powerio/issues/185) (bindings).
