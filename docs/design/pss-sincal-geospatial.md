# SINCAL coordinate and geometry reader plan

Planning status, 2026-10-08. This is the sixth draft PR in the SINCAL stack,
based on [experimental writer PR #568](https://github.com/eigenergy/powerio/pull/568).
No geometry implementation is claimed by this plan. The intended implementation
covers both existing reader families; geometry writing remains outside this
increment. The five electrical PRs can be reviewed and merged independently
of this follow-up. The initial-support closure reference for issue #556 stays
on PR #568.

## Outcome and existing infrastructure

A native SINCAL parse should populate source-backed bus locations and verified
line routes, carry the coordinate space and provenance through PowerIO IR, and
support existing geographic-layer extraction. Balanced input remains
`BalancedNetwork`; conductor-resolved input remains `MulticonductorNetwork`.
Electrical values, terminal identities, topology and calculation results must
be identical with and without optional geometry.

Reuse `Bus.location`/`Branch.route`/`GeoMeta` and
`DistBus.location`/`DistLine.route`/`DistGeoMeta`. The facade already bridges both
families to `GeoLayer`. Keep the shared decoder model-neutral in
`powerio-sincal`; adapters perform the joins onto their own typed models.
Do not add a distribution-to-transmission dependency or another network type.

## Evidence inspected during planning

| Source | Observed data | Implication |
| --- | --- | --- |
| Existing licensed SimBench schema-14.8 archive | 15 `GraphicNode`, 32 `GraphicElement`, 46 `GraphicTerminal`; no bend points; all `Node.lat/lon/hr/hh` values zero, `Flag_Pos=2` | Reuse the fixture for graphics mapping and zero-placeholder regressions; it does not establish geographic positions |
| External CSIRO09 | 621 native nodes and 621 graphic nodes; `hr/hh` all zero, `lat/lon` absent values, `Flag_Pos=1`; no bend points | Large distribution drawing-coverage candidate; no validated georeference |
| External CSIRO12 | 191 native and graphic nodes; node-position values NULL, `Flag_Pos=1`; no bend points | Test missing geographic positions independently of usable drawing data |
| External CSIRO19 | 26 native and graphic nodes; `hr/hh` all zero, `Flag_Pos=1`; no bend points | Balanced Access transport and graphics-coverage candidate |
| Siemens April 2014 Database Description, Node and network-graphics sections | `hr/hh` are described as longitude/latitude; graphic node endpoints, terminal positions and bend points have their own meanings | Field meanings are version-dependent; later `lat/lon` and `Flag_Pos` precedence still need verification |
| Siemens April 2014 File Formats, section 3.1.9 | Node graphics have start/end points; element graphics distinguish symbol centers and ordered geometry | A symbol center is not automatically a line waypoint; XML descriptions do not alone prove database route assembly |

The manuals are already held externally from the source recorded in the
[research catalog](../../evals/sincal/research-catalog.md). They are reference
material, not redistributable fixtures. These observations do not establish
WGS84, a projected CRS, or the semantics of `Flag_Pos=1/2` for every schema.
Zero can be a valid coordinate: do not implement a universal `(0,0) = missing`
rule, or a numeric-range heuristic that turns drawings into latitude/longitude.

## Implementation milestones

### 1. Pin the coordinate profile

Inventory the position selectors and metadata in the already acquired cases.
Resolve, per admitted schema, the active pair among `hr/hh`, `lat/lon`, and
any documented projected values. Confirm presence/placeholder semantics,
axis order, angular units, CRS/datum and any transform stored outside the main
database. Record source/version evidence rather than guessing from field names.

Independently describe the graphics joins:

- `GraphicNode.Node_ID` to the selected native node;
- `GraphicElement.Element_ID` to equipment;
- `GraphicTerminal.Terminal_ID` and its graphic-element identity to ports;
- `GraphicBucklePoint.GraphicTerminal_ID`, ordered by `NoPoint`;
- selected variant, graphic area and layer identities.

The first deliverable is a short checked mapping table and tiny synthetic
records. Do not begin a broad new corpus search. If authoritative geographic
semantics remain unavailable, proceed with clearly labelled drawing/unknown
coordinates and retain geographic fields as source-only; report the narrower
supported profile explicitly.

### 2. Read bus locations through both backends

Add bounded shared coordinate records and adapters for balanced and distribution
buses. Read direct node-position fields already present in acquired `Node`
records. Add a documented, opt-in graphics table set to the Access acquisition
helper; old electrical-only acquired companions must remain valid and warn
that optional graphics were not acquired. No implicit external-tool invocation.
Existing archive namespaces and byte/query/row limits remain authoritative.

Use these initial selection rules:

1. A verified, declared geographic/projected set takes precedence.
2. Otherwise use one unambiguous graphic area with consistent representations.
   Merge identical duplicates; conflicting representations are diagnosed and
   left unmapped rather than selecting the first row.
3. A network has one coordinate space. Never fill missing geographic positions
   with drawing positions or mix areas/CRSs in one layer.
4. Without verified earth referencing, preserve coordinates as `Unknown`, or
   `Diagram` when the evidence establishes drawing space. Never set geographic
   space with an absent CRS unless WGS84 is actually justified: PowerIO treats
   that omission as EPSG:4326.

Do not introduce new public selection options in the first increment. If real
cases require explicit choice among areas/representations, use a small later
extension. Do not enlarge published C option structs to add it.

For a point-shaped graphic node, preserve its point. For an extended busbar,
use a documented midpoint only as a derived representative location, retain the
two source endpoints in provenance and report the approximation. Source points
use `Source` origin; computed representatives use `Derived`. Generated
auxiliary buses do not acquire fabricated source positions. Preserve native
identity in joins, including inactive/open equipment.

Missing or malformed optional geometry should leave the electrical model usable:
attach structured diagnostics for bad references, NULL/nonfinite/incomplete
pairs, unsupported selectors, conflicting locations and omitted areas. Preserve
original bytes. Transport/resource-limit violations still follow existing
acquisition failure rules. Explicit geographic input outside its valid range
is invalid geometry; a planar coordinate of the same magnitude is not.

### 3. Add line routes and existing geo workflows

Assemble ordered line polylines only after confirming the database's terminal
and bend-point direction conventions. Preserve bends and open-end locations;
never sort by physical distance, insert a symbol center by default, or turn
parallel lines into an endpoint-pair match. Join using native element identity.
Validate ordering, port orientation and variant/area membership. Missing route
data stays absent; endpoint-to-endpoint rendering already exists in consumers.

Keep physical `Route`/`RouteNode`/`RouteRel` tables outside the initial mapping
unless their relationship to electrical lines and coordinate semantics are
independently established. Do not mistake them for generic graphical bends.
Transformer/device artwork, labels, styles, background maps and result diagrams
remain source-only.

Expose mapped geometry through existing Rust, CLI, Python, C and Julia location
and geo-layer paths. Verify the actual bindings before promising route parity;
add narrowly scoped accessors only if an existing surface lacks them. No new
IR version should be needed for filling existing coordinate fields.

Update fidelity reporting: promoted geometry is no longer wholly source-only,
but styles, unselected drawings and unsupported graphic records still are.
Native byte echo remains exact. IR and supported geo output retain typed
geometry; formats without geometry issue the existing loss diagnostics.
Fresh SINCAL writing continues to report geometry loss.

## Validation and completion criteria

1. **Synthetic mapping tests:** explicit geographic, projected, diagram and
   unknown spaces; legitimate zero positions versus documented placeholders;
   NULL/NaN/infinite values; reversed axes; duplicate graphics; multiple areas;
   variants; extended busbars; duplicate/gapped bend ordering; dangling
   references; open terminals and parallel lines. Each failure must exercise
   the defined diagnostic/omission policy without corrupting electrical data.
2. **Existing authentic fixtures:** independently extract native graphic node
   rows from the small SimBench archive and compare every mapped native node.
   Check at least one large distribution case (CSIRO09) and balanced Access
   case (CSIRO19) through the public readers. Account separately for native
   nodes without equipment conductors and generated auxiliary buses.
3. **Routes:** find one existing authentic model with nonempty bends and
   independently reproduce its ordered route. Current inspected cases have no
   bend points, so they cannot validate bending/order. Synthetic route tests
   may ship first only with the limited evidence clearly stated.
4. **Real georeferencing:** require a non-placeholder native case with a
   declared CRS and an independent coordinate reference (paired GIS export,
   documented control points or equivalent). Compare values within a justified
   precision tolerance. Without that evidence, claim layout extraction only,
   not verified geographic interoperability. No SINCAL desktop is required
   for source-table checks; native visual acceptance remains an external gate.
5. **End to end:** native parse -> typed locations/routes -> PowerIO IR ->
   restore -> `GeoLayer`/canonical geo JSON -> reparse. Assert geometry,
   coordinate space and source/derived provenance. Check unchanged native echo,
   cross-format geometry-loss reporting, and a binding/CLI extraction path for
   both families. Reject altered axis, identity and bend-order negative controls.
6. **Electrical non-regression:** compare all electrical fields and topology
   with the pre-feature results; rerun affected tests and the existing successful
   native-case validation. Geometry must not change admittance or PF inputs.
   Run the full required clippy matrix before pushing implementation changes.

Use only small synthetic additions plus the existing licensed archive in unit
tests. Keep CSIRO models external, with attribution and source hashes in the
validation report. No new fixture over 100 KiB without the repository's exact-file
approval, and no unlicensed model data in Git.

## Delivery boundary

One follow-up PR should contain shared decoding, both adapter mappings,
diagnostics, integration and evidence. Review bus points first, then verified
routes; do not create one PR per data source. A later experimental writer
increment can emit verified coordinate tables after the reader conventions are
stable. Automatic reprojection, multiple simultaneous drawings, GIS imagery,
full SINCAL UI reconstruction and native desktop acceptance are separate work.

The practical first success is useful, honestly classified locations for both
families, including a large feeder, with electrical results unchanged. Geographic
CRS proof and authentic bend geometry are specific evidence gaps, not reasons
to rebuild PowerIO's existing geographic model or block the electrical PR stack.
