# SINCAL coordinate and geometry reader

Implemented scope, 2026-10-08. [PR #582](https://github.com/eigenergy/powerio/pull/582)
is the sixth SINCAL PR, based on [experimental writer PR #568](https://github.com/eigenergy/powerio/pull/568).
The initial electrical support closure reference for issue #556 stays on #568.
This follow-up reads a bounded drawing profile in both network families.
It does not establish geographic coordinates or add fresh geometry writing.

## Delivered profile

The shared `powerio-sincal` decoder remains model-neutral. The balanced adapter
populates `Bus.location` and `Branch.route`; the distribution adapter populates
`DistBus.location` and `DistLine.route`. Both use existing geo metadata, IR,
and facade geographic-layer extraction. No IR schema, ABI or network type changes.

| Native record | Mapping | Boundary |
| --- | --- | --- |
| `GraphicNode.Node_ID` | Native bus identity; point if endpoints equal, otherwise derived midpoint | Preserve both endpoints in `extras.sincal_geometry`; generated auxiliary buses stay unlocated |
| `GraphicElement.Element_ID` | Native line identity | No name, endpoint-pair or row-order matching; parallel lines remain distinct |
| `GraphicTerminal.Terminal_ID` | Native line port 1 or 2 and terminal position | Terminal must belong to the same element and graphic area; route endpoint is the terminal point, not a busbar midpoint |
| `GraphicBucklePoint.GraphicTerminal_ID` | Optional bend attached to its port | At most one bend per port, `NoPoint=1`; multiple bends or other numbering omit that route with a diagnostic |
| Selected variant, `Flag_Variant=1` | Active drawing rows | One unambiguous graphic area; conflicting node positions and duplicate line/port identities are omitted |

Coordinates retain their native axes and values, including legitimate zeros.
The network space is **Unknown**, never inferred WGS84. Point locations and
route vertices use `Source` provenance; busbar representatives use `Derived`.
The bounded geometry diagnostic reports counts, area and omitted-record reasons
with at most eight sample IDs per reason. Transport/query/row-budget errors
remain errors; malformed optional geometry leaves electrical parsing usable.
Each graphic table has a 100,000-row cap within the existing acquisition budgets.

Access acquisition adds the opt-in `import_access.py --include-graphics` flag.
Old electrical-only companions remain valid; an excluded `GraphicNode` table
produces a `not_acquired` finding. Other required electrical tables still need
the same explicit selection as before (notably CSIRO19 network groups).
The library never invokes external acquisition tools.

Unchanged native emission remains byte-exact. IR preserves typed coordinates,
routes and busbar provenance but does not serialize retained source bytes.
Canonical geo JSON preserves geometry, space and source/derived kinds; its
existing parser trims display-name whitespace. It is not WGS84 GeoJSON when
space is Unknown. Retention inventory identifies graphic tables as partially
mapped; styles, unsupported drawings and other fields remain source-only.
Fresh experimental SINCAL writers continue to report geometry omission.

## Evidence and completed validation

The [reproducible checker](../../evals/sincal/verify_geometry.py) compares the
public reader with native SQLite rows or independent `mdb-export` CSV rows.
The report contains source hashes and aggregates, not redistributed model data:
[geometry-validation.json](../../evals/sincal/geometry-validation.json).

| Case | Family | Typed/native graphic nodes | Line routes | Interior bends |
| --- | --- | ---: | ---: | ---: |
| Licensed SimBench rural1 | Balanced | 15/15 | 13 | 0 |
| CSIRO09 | Distribution | 621/621 | 472 | 0 |
| CSIRO12, existing explicit compatibility option | Distribution | 188/191 | 153 | 0 |
| CSIRO19 | Balanced | 26/26 | 25 | 0 |
| Truong 12-bus | Distribution | 12/12 | 11 | 3 |

CSIRO09 includes four isolated native buses; preserving their drawing positions
does not remove the existing generic PF readiness limitation. CSIRO12's three
nodes without equipment conductors remain in `sincal_unconnected_nodes`, not
fabricated electrical buses. The Truong case has 12 derived busbar midpoints;
CSIRO19 has nine. Their route endpoints retain actual terminal attachment points.

All five pass native echo, typed IR and canonical geometry export/reparse.
Deleting only graphic tables in temporary validation copies produces identical
electrical fields/topology (the temporary input name is excluded from comparison).
Axis, node-identity and route-vertex negative controls are detected in each case.
Numerical checks reuse the existing independent OpenDSS/pandapower oracles with
fresh geometry-bearing output; these are regression checks, not new desktop
acceptance or new claims about native NULL semantics.

Synthetic and public integration tests cover zero, NULL/infinite positions,
unresolved identities, duplicates, variants, multiple areas, busbar provenance,
parallel lines, malformed ports, unsupported bend numbering and missing tables.
The existing licensed 88,876-byte archive is reused. No new third-party fixture
is added; CSIRO stays external and the unlicensed Truong database is never vendored.

## Deferred implementation and external acceptance gates

**Further implementation, only with evidence:** support multiple bends per port
once database numbering/direction is independently established; explicit selection
among multiple areas if demanded by real users; geographic/projected node fields
once versioned selectors, axes, units and CRS are verified; fresh writer geometry.
Physical `Route`/`RouteNode`/`RouteRel`, device artwork, labels, visibility/style
layers, backgrounds and result diagrams remain source-only.

**External acceptance:** a non-placeholder case with declared CRS and independent
GIS coordinates is needed to claim georeferencing. SINCAL desktop visual and
open/save acceptance is a separate gate; no desktop execution has occurred.
Neither gate blocks the useful, explicitly Unknown drawing profile above.

The already collected Siemens April 2014 Database Description specifies graphic
node endpoints, terminal positions and bend-point references. Its File Formats
section 3.1.9 distinguishes symbol centers from route points, but XML descriptions
do not establish database multi-bend ordering. See the
[research catalog](../../evals/sincal/research-catalog.md) for provenance.
The manuals remain external reference material.

The broader collected inventory confirms why no geographic inference is safe:
CSIRO10/11 contain `lat/lon` values with planar-scale magnitudes, while other cases
use `hr/hh` or NULL/zero position fields. SimBench's direct positions are all zero
with `Flag_Pos=2`. Field labels and numeric ranges alone do not identify a CRS.
