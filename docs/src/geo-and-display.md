# Geographic and display data

PowerIO stores coordinates when a supported source provides them. Coordinates
are optional; readers do not invent them, and network writers without a
coordinate representation report the loss.

PowerWorld `.pwd` files are display data rather than network cases. Parse them
with `parse_display_file` or `parse_display_bytes` rather than the network
parser.

## Coordinate fields

Both network model families expose the same JSON shape:

```rust
pub struct Location {
    /// Longitude for geographic coordinates.
    pub x: f64,
    /// Latitude for geographic coordinates.
    pub y: f64,
    /// Point provenance when it differs from the network default.
    pub kind: Option<CoordsKind>,
}

pub enum CoordsKind { Source, Synthetic, Manual, Derived }

pub struct GeoMeta {
    pub space: CoordinateSpace,
    pub kind: Option<CoordsKind>,
}

pub struct Canvas {
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub units: Option<String>,
}

pub enum CoordinateSpace {
    Geographic { crs: Option<String> },
    Projected { crs: Option<String> },
    Diagram { canvas: Option<Canvas> },
    Unknown,
}
```

Balanced networks use `powerio::geo::{Location, CoordsKind, CoordinateSpace,
GeoMeta, Canvas}` through `Network.geo` and `Bus.location`. Multiconductor
networks use the matching `powerio_dist::geo` types through `DistNetwork.geo`
and `DistBus.location`. A package serialization test keeps the two JSON shapes
identical. Branches carry optional polyline routing (`Branch.route`,
`DistLine.route`) when a source provides intermediate geometry; endpoint only
rendering derives from the bus locations.

The coordinate space belongs to the network. For geographic coordinates,
`x` is longitude and `y` is latitude in GeoJSON axis order. A missing CRS in a
geographic space means EPSG:4326. `kind` records whether coordinates came from
the source, a generated layout, a manual edit, or a derived transform.

## Harvest and emit

Readers promote coordinates into `location` and stamp the space; promotion
removes the raw keys from `extras`. Writers emit from `location`.

| Format | Fields | Space |
| --- | --- | --- |
| PowerWorld aux | `Latitude:1`/`Longitude:1` bus columns (`SubNum` stays in extras: it is identity rather than geometry) | geographic |
| pandapower | bus `geo` GeoJSON Point strings | geographic |
| PyPSA | `buses.csv` `x`/`y` | geographic |
| OpenDSS | `Buscoords` | unknown; a diagnostic identifies values within longitude and latitude bounds |
| BMOPF JSON | `longitude`/`latitude` (the BMOPFTools sideload convention; writing is opt in via `BmopfWriteOptions::sideload_coordinates`) | geographic |

MATPOWER, PSS/E, PowerModels, egret, GOC3, PSLF, and Surge carry no geometry.
Writing a located case to one of them reports the dropped locations, the same
behavior `base_frequency` has; `powerio geo extract` writes the sidecar as the
escape hatch.

## The geographic document

Coordinates also arrive and leave as files of their own: a `Buscoords` CSV
next to a DSS master, a GeoJSON export from a GIS tool, a layout computed by a
renderer. The container is `GeoLayer`, surfaced as `DisplayData::Geo` beside
the PowerWorld `.pwd` display path.

The canonical wire form is a GeoJSON FeatureCollection with one foreign
member, suggested extension `.geo.json`:

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
it directly.

Reading is tolerant; writing is canonical. `GeoLayer::parse_bytes` takes bytes
plus a file name hint and touches no filesystem. It accepts headerless
buscoords CSV (`bus, x, y`), CSV and JSON records with aliased field names
(`bus_i`/`bus`/`id`, `lat`/`latitude`/`y`, `lon`/`lng`/`longitude`/`x`, branch
endpoint pairs), and GeoJSON Point and LineString features. Features reference
elements by up to three key fields, matched in order: `uid`, then `id`, then
case insensitive `name`. Branch routes additionally fall back to the unordered
`(from, to)` bus pair. A bare integer branch id is accepted on read as a
1-based positional row alias and never written; the durable identity is the
payload `uid`.

`Network::geo_layer()` extracts, and `Network::apply_geo_layer(&layer)`
applies and returns a `GeoApplyReport` with matched and unmatched counts. The
multiconductor equivalents attach through `powerio-pkg` (`dist_geo_layer`,
`apply_dist_geo_layer`). The CLI wraps the same surface:

```console
$ powerio geo extract case.aux -o case.geo.json
$ powerio geo apply case.m layout.csv -o placed.m
$ powerio geo convert buscoords.csv -o case.geo.json
```

## PowerWorld display files

The `.pwd` reader returns `DisplayData::PowerWorld` with a `PwdDisplay`: canvas
dimensions, a timestamp, and substation symbols with number, name, and diagram
coordinates.

Three helpers connect it to the geo model. `geo_layer_from_pwd` lifts the
substation symbols into a diagram space `GeoLayer` (also reachable as
`powerio geo extract case.pwd`); `apply_substation_points` joins those points
onto buses through the `SubNum` extras key; and `pwd_mercator_to_lonlat` is a
documented, approximate inverse of the projection PowerWorld's auto generated
layouts use, for consumers that want to place a diagram on a map.

Rust uses `parse_display_file` and `parse_display_bytes`. Python exposes the
same names and returns `DisplayData(kind="powerworld", data=PwdDisplay(...))`.
Display files do not pass through `Network`, `Conversion`, or `.pio.json`.

## Distribution graph projection

`DistNetwork::graph()` returns a bus and terminal graph without requiring
coordinates. Python exposes `dist_net.graph()`, and the C `dist` feature
exposes `pio_dist_graph_json`. Graph topology and geographic placement remain
separate data.

PowerIO stores and transports coordinates; it does not compute them. Synthetic
layout of a coordinate free case is renderer math and stays in the consumer,
which can write the result back with `kind = synthetic` so the provenance
survives.

The C ABI exposes the document as strings: `pio_geo_parse` normalizes a
tolerant sidecar to the canonical form, `pio_geo_extract` and `pio_geo_apply`
work on a parsed network handle (apply returns a new handle whose warnings
carry the match report), and `pio_dist_geo_extract`/`pio_dist_geo_apply` are
the multiconductor equivalents. Python mirrors the surface with `parse_geo`
and `geo_layer()`/`apply_geo_layer()` on both network types.
