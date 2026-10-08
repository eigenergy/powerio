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
| `GraphicBucklePoint.GraphicTerminal_ID` | Ordered bends attached to their port | Contiguous `NoPoint=1..n`; port 1 reversed, port 2 ascending; duplicates/gaps omit the route with a diagnostic |
| `GraphicAreaTile` | Schematic classification and retained view metadata | `Flag=2` maps to Diagram; `Flag=1` stays Unknown until CRS is verified |
| Selected variant, `Flag_Variant=1` | Active drawing rows | One unambiguous graphic area; conflicting node positions and duplicate line/port identities are omitted |

Coordinates retain their native axes and values, including legitimate zeros.
The network space is **Diagram** for a declared schematic view (`Flag=2`) and
**Unknown** otherwise, never inferred WGS84. Point locations and
route vertices use `Source` provenance; busbar representatives use `Derived`.
The bounded geometry diagnostic reports counts, area and omitted-record reasons
with at most eight sample IDs per reason. Transport/query/row-budget errors
remain errors; malformed optional geometry leaves electrical parsing usable.
Each graphic table has a 100,000-row cap within the existing acquisition budgets.

View identity, name, origin, page dimensions and scales are retained in the module
extension `powerio.sincal.graphic_view`, together with optional `CoordSys` and
reference-coordinate fields when present. IR preserves this extension. Native
values are not transformed: page dimensions are centimetres, while origin
offsets are metres, so page dimensions do not become a coordinate-space canvas.
Newer metadata fields do not admit newer electrical schemas.

Access acquisition adds the opt-in `import_access.py --include-graphics` flag.
Old electrical-only companions remain valid; an excluded `GraphicNode` table
produces a `not_acquired` finding. Other required electrical tables still need
the same explicit selection as before (notably CSIRO19 network groups).
The library never invokes external acquisition tools.

Unchanged native emission remains byte-exact. IR preserves typed coordinates,
routes and busbar provenance but does not serialize retained source bytes.
Canonical geo JSON preserves geometry, space and source/derived kinds; its
existing parser trims display-name whitespace. It is not an earth-referenced GIS
layer when space is Diagram or Unknown. Retention inventory identifies graphic
tables as partially mapped; styles, unsupported drawings and other fields remain source-only.
Fresh experimental SINCAL writers continue to report geometry omission.

## Evidence and completed validation

The [reproducible checker](../../evals/sincal/verify_geometry.py) compares the
public reader with native SQLite rows or independent `mdb-export` CSV rows.
The report contains source hashes and aggregates, not redistributed model data:
[geometry-validation.json](../../evals/sincal/geometry-validation.json).

| Case | Family | Typed/native graphic nodes | Line routes | Interior bends | Space |
| --- | --- | ---: | ---: | ---: | --- |
| Licensed SimBench rural1 | Balanced | 15/15 | 13 | 0 | Unknown |
| CSIRO09 | Distribution | 621/621 | 472 | 0 | Diagram |
| CSIRO12, existing explicit compatibility option | Distribution | 188/191 | 153 | 0 | Diagram |
| CSIRO19 | Balanced | 26/26 | 25 | 0 | Diagram |
| Truong 12-bus | Distribution | 12/12 | 11 | 3 | Diagram |

CSIRO09 includes four isolated native buses; preserving their drawing positions
does not remove the existing generic PF readiness limitation. CSIRO12's three
nodes without equipment conductors remain in `sincal_unconnected_nodes`, not
fabricated electrical buses. The Truong case has 12 derived busbar midpoints;
CSIRO19 has nine. Their route endpoints retain actual terminal attachment points.

All five pass native echo, typed IR (including view metadata) and canonical
geometry export/reparse. Native view metadata is checked independently too.
Coordinate comparison uses relative tolerance 2e-15 and absolute tolerance
1e-12 for the few-ULP differences between independent MDB CSV/JSON exports;
identity sets and vertex counts remain exact.
Deleting only graphic tables in temporary validation copies produces identical
electrical fields/topology (the temporary input name is excluded from comparison).
Axis, node-identity and route-vertex negative controls are detected in each case.
Numerical checks reuse the existing independent OpenDSS/pandapower oracles with
fresh geometry-bearing output; these are regression checks, not new desktop
acceptance or new claims about native NULL semantics.

Synthetic and public integration tests cover zero, NULL/infinite positions,
unresolved identities, duplicates, variants, multiple areas, busbar provenance,
parallel lines, malformed ports, gapped/duplicate bend numbering, shuffled
multi-bend rows, view classification, optional CRS retention and missing tables. Eleven shared geometry tests and
three public integration tests cover these behaviors.
The existing licensed 88,876-byte archive is reused. No new third-party fixture
is added; CSIRO stays external and the unlicensed Truong database is never vendored.

## Additional drawing-only validation

The shared decoder also passes independent native-table checks on **CSIRO03**
(785 graphic nodes, 774 routes, 1,973 interior bend vertices, longest route 40
vertices) and **CSIRO13** (1,769 graphic nodes, 874 nondegenerate routes, 23
interior bend vertices). Both detect a deliberately swapped native bend order.
CSIRO13 has 889 collapsed routes omitted with diagnostics; most native bends
repeat existing positions. CSIRO03 remains Unknown space; CSIRO13 is Diagram.
These checks validate drawing decoding, **not complete electrical support** for
those two cases, and make no public echo/IR or solver claim for them.

## Manual evidence and remaining work

- Siemens *Database Description*, April 2014, printed page 274, documents
  `GraphicAreaTile.Flag` (geographical/schematic), metre origin offsets and
  centimetre page dimensions. Its File Formats section 3.1.9 distinguishes
  symbol centers from line-contour points.
  [Archived manual](https://raw.githubusercontent.com/benediktibk/loadflow/2d95494327123de7eaa9c6fe0bc09990038ee364/documents/external/sincal/Datenbankbeschreibung.pdf).
- Siemens *Database Interface and Automation*, April 2015, section 2.5.2,
  explicitly orders `NoPoint` from the element symbol toward the node/busbar.
  This resolves the earlier multi-bend uncertainty; ordered bends are now
  implemented. [Manual](https://manualzz.com/doc/2231110/pss-sincal-database-interface-and-automation).
- Siemens [22.0 release notes](https://sincal.s3.amazonaws.com/22.0/ReleaseNotes-Eng.pdf),
  pages 9–12 and 41, describe projection handling and new `CoordSys`, `RefLat`,
  `RefLon`, `RefPosX` and `RefPosY` fields. They do not establish the serialized
  `CoordSys` syntax in our collected native files. Page 3 identifies bundled
  Example Imp Excel, Example NMM and Example Gas models as potential future
  geographic samples. A targeted public search found documentation but did not
  locate a usable standalone native sample with a declared CRS. No such sample
  has been acquired or redistributed.

**Further implementation:** explicit selection among multiple areas if demanded
by users; geographic/projected node fields after verifying versioned selectors,
axes, units and CRS; fresh writer geometry. Physical `Route`/`RouteNode`/`RouteRel`,
device artwork, labels, visibility/style layers, backgrounds and result diagrams
remain source-only.

**External acceptance:** a non-placeholder native case with declared CRS and
independent GIS coordinates is needed to claim georeferencing. SINCAL desktop
visual and open/save acceptance is a separate gate; no desktop execution has
occurred. Neither gate blocks the documented drawing profile above.

The broader collected inventory confirms why no geographic inference is safe:
CSIRO10/11 contain `lat/lon` values with planar-scale magnitudes, while other cases
use `hr/hh` or NULL/zero position fields. SimBench's direct positions are all zero
with `Flag_Pos=2`. Field labels and numeric ranges alone do not identify a CRS.
