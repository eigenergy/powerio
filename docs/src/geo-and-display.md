# Geographic and display data

> **Status: partial.** The distribution graph projection from #182 ships in
> 0.6.1 as `DistNetwork::graph()`. The coordinate model and GeoLayer
> interchange remain design work.

Power system case files disagree about where equipment sits on a map, and most
say nothing at all. PowerWorld aux exports carry substation latitude and
longitude; OpenDSS distributes coordinates in a separate `Buscoords` file;
pandapower has a `geo` column; MATPOWER, PSS/E, and the BMOPF exchange schema
carry no geometry. Today PowerIO keeps what it happens to see in per element
`extras` maps and loses it at most writers. Consumers that render networks
re-derive coordinates from those maps per source format, which is parsing work
that belongs here.

This chapter specifies a canonical coordinate model in two layers: typed
optional fields on the network models, and a standalone geographic document for
interchange. Both are optional everywhere. A case without coordinates
serializes byte identically to today, and no writer invents geometry.

## Layer 1: typed model fields

Both model families gain the same optional fields:

- `Bus.location: Option<Location>` and `DistBus.location: Option<Location>`;
- `Network.geo: Option<GeoMeta>` and `DistNetwork.geo: Option<GeoMeta>`;
- later, polyline routing: `Branch.route` and `DistLine.route`.

The types are small:

```rust
pub struct Location {
    /// x is longitude when the space is geographic (GeoJSON axis order).
    pub x: f64,
    /// y is latitude when the space is geographic.
    pub y: f64,
    /// Per point provenance when it differs from the network default.
    pub kind: Option<CoordsKind>,
}

pub enum CoordsKind { Source, Synthetic, Manual, Derived }

pub struct GeoMeta {
    pub space: CoordinateSpace,
    /// Default provenance for points without their own `kind`.
    pub kind: Option<CoordsKind>,
}

pub enum CoordinateSpace {
    /// x = lon, y = lat, decimal degrees. `crs` defaults to EPSG:4326.
    Geographic { crs: Option<String> },
    /// Planar projected coordinates, `crs` when known.
    Projected { crs: Option<String> },
    /// Drawing coordinates with no earth referent (.pwd, hand diagrams).
    Diagram { canvas: Option<Canvas> },
    /// The source did not declare a space (bare OpenDSS buscoords).
    Unknown,
}
```

The coordinate space is a network property and is not a bus property. A network with
per bus coordinate systems cannot be rendered. Per point `kind` exists so a
partially hand placed network round trips: three buses pinned manually, the
rest from a synthetic layout, and a renderer knows which points it may move.

`powerio_dist` cannot depend on `powerio`, and `powerio-pkg` depends on both,
so no shared crate can sit below the two models. The types are therefore
defined in `powerio::geo` and mirrored in `powerio_dist::geo`, the same
arrangement `Extras` already uses. A parity test in `powerio-pkg` serializes
both and asserts identical JSON, so the copies cannot drift.

Before this is implemented, it is worth considering whether a `powerio-geo` or `powerio-format` crate make be necessary.

Because the fields are additive and skipped when absent, they ride the
`.pio.json` model JSON as a minor bump.

## Layer 2: the geographic document

Coordinates also arrive and leave as files of their own: a `Buscoords` CSV next
to a DSS master, a GeoJSON export from a GIS tool, a layout computed by a
renderer for a case that had no geometry. The container for these is
`GeoLayer`, surfaced as a `DisplayData::Geo` variant beside the existing
PowerWorld `.pwd` display path.

The canonical wire form is a GeoJSON FeatureCollection with one foreign member:

```json
{
  "type": "FeatureCollection",
  "powerio_geo": { "version": "0.1.0", "space": "geographic", "kind": "source" },
  "features": [
    { "type": "Feature",
      "geometry": { "type": "Point", "coordinates": [-80.05, 34.20] },
      "properties": { "target": "bus", "id": "312", "uid": "buses:11" } },
    { "type": "Feature",
      "geometry": { "type": "LineString", "coordinates": [[-80.05, 34.20], [-80.10, 34.30]] },
      "properties": { "target": "branch", "uid": "branches:4", "from": "312", "to": "410" } }
  ]
}
```

When the space is geographic this is valid RFC 7946 GeoJSON, so GIS tools open
it directly. The suggested extension is `.geo.json`.

Reading is tolerant; writing is canonical. The reader accepts headerless
buscoords CSV (`bus,x,y`), CSV and JSON records with aliased field names
(`bus_i`/`bus`/`id`, `lat`/`latitude`/`y`, `lon`/`lng`/`longitude`/`x`, branch
endpoint pairs), and GeoJSON Point and LineString features. Writers emit only
the canonical form above.

Features reference elements by an `ElementKey` of up to three fields, matched
in order: `uid`, then `id`, then case insensitive `name`. Ids are strings on
the wire, so one document format serves balanced integer bus ids,
multiconductor string ids, and BMOPF ids. Branch paths additionally fall back
to the unordered `(from, to)` bus pair. Positional branch indices are accepted
on read as a legacy alias and never written; the durable identity is the
model `uid`.

## Harvest and emit

Readers promote coordinates out of `extras` into `location` and stamp the
space. Promotion removes the raw keys: `location` is the single source of
truth, and byte level round trips of an unmodified case already go through the
retained source text, not through extras.

| Format | Read | Space |
| --- | --- | --- |
| PowerWorld aux | `Latitude:1`/`Longitude:1` bus fields (`SubNum` stays in extras; it is identity, not geometry) | geographic |
| OpenDSS | `Buscoords` | unknown, with a range check diagnostic that suggests geographic when every point fits lon/lat bounds |
| BMOPF JSON | out of schema `longitude`/`latitude` (the BMOPFTools sideload convention) | geographic |
| pandapower | bus `geo` GeoJSON Point strings | geographic |
| PyPSA | `buses.csv` x/y | geographic |
| MATPOWER, PSS/E, PowerModels, egret, GOC3, PSLF, Surge | nothing to read; `location` stays `None` | — |

Writers emit from `location`:

- **OpenDSS**: a companion `Buscoords` CSV.
- **BMOPF JSON**: dropped by default with a structured warning, because the
  schema sets `additionalProperties: false` and the writer promises schema
  valid output. `BmopfWriteOptions::sideload_coordinates` opts in to emitting
  `longitude`/`latitude` per bus, matching the BMOPFTools convention. This
  default flips if the task force adopts coordinate fields upstream.
- **pandapower**: the bus `geo` column, replacing today's null.
- **PowerWorld aux**: `Latitude:1`/`Longitude:1` bus columns.
- **GeoJSON**: the canonical `GeoLayer` document.
- Formats with no geometry concept warn that locations were dropped, the same
  contract `base_frequency` uses. `powerio geo extract` writes the sidecar as
  the escape hatch.

## PowerWorld `.pwd` promotion

The `.pwd` reader already decodes substation symbols in diagram coordinates.
Two additions connect it to the geo model: `geo_layer_from_pwd` produces a
`GeoLayer` in diagram space with substation targets, and
`apply_substation_points` joins those points onto buses through the `SubNum`
extras key. The auto generated PowerWorld layouts equal a Mercator projection
of geography, so `pwd_mercator_to_lonlat` ships as a documented, approximate
inverse for consumers that want to place a diagram on a map.

## API surface

Rust (and therefore wasm consumers):

- `powerio::geo::{Location, CoordsKind, CoordinateSpace, GeoMeta, GeoLayer}`;
- `parse_display_bytes(bytes, "geojson")` returning `DisplayData::Geo`;
- `Network::geo_layer()` and `Network::apply_geo_layer(&GeoLayer) -> GeoApplyReport`
  (matched and unmatched counts, notes); the multiconductor equivalents attach
  through `powerio-pkg`, which sees both model crates;
- `powerio geo extract | apply | convert` in the CLI.

C ABI: `pio_geo_parse`, `pio_geo_extract`, `pio_geo_apply`, and `pio_dist_geo_*`
counterparts, all string in and string out. No new object lifetimes and no ABI
bump; format names stay strings.

## What PowerIO does not do

PowerIO stores and transports coordinates; it does not compute them. Synthetic
layout (force placement of a coordinate free case) is renderer math and stays
in the consumer. A consumer that computes a layout can write it back with
`kind = synthetic` so the provenance survives.

## Phasing

1. Typed fields plus the OpenDSS and BMOPF harvest and emit paths (model JSON
   1.1.0).
2. Balanced harvest and emit: PowerWorld aux, pandapower, PyPSA, fidelity
   warnings.
3. `GeoLayer`, `DisplayData::Geo`, branch routing, the CLI subcommand
   (model JSON 1.2.0).
4. `.pwd` promotion and the Mercator helper.
5. C ABI and Python bindings.

Tracking issues: [#180](https://github.com/eigenergy/powerio/issues/180)
(typed fields), [#183](https://github.com/eigenergy/powerio/issues/183)
(balanced formats), [#184](https://github.com/eigenergy/powerio/issues/184)
(GeoLayer and `.pwd`), [#185](https://github.com/eigenergy/powerio/issues/185)
(bindings).
