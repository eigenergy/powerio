# Geographic and display data

PowerIO stores coordinates when a supported source provides them. Coordinates
are optional; parsers do not invent them, and emission to a network format
without a coordinate representation reports the loss.

PowerWorld `.pwd` files are display data rather than network cases. Parse them
with `parse_display_file` rather than the network parser. The parser acquires
the binary display file from its path. A Rust application that already owns
the bytes calls `parse_display(bytes, "pwd")`; no `parse_bytes` spelling is
introduced.

## Coordinate fields

Both network model families expose the same JSON shape:

```rust
pub struct Location {
    /// Longitude for geographic coordinates.
    pub x: f64,
    /// Latitude for geographic coordinates.
    pub y: f64,
    /// Point origin when it differs from the network default.
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
GeoMeta, Canvas}` through `BalancedNetwork.geo` and `Bus.location`.
Multiconductor networks use the matching `powerio_dist::geo` types through
`MulticonductorNetwork.geo` and `DistBus.location`. A package serialization test keeps the two JSON shapes
identical. Branches carry optional polyline routing (`Branch.route`,
`DistLine.route`) when a source provides intermediate geometry; endpoint only
rendering derives from the bus locations.

The coordinate space belongs to the network. For geographic coordinates,
`x` is longitude and `y` is latitude in GeoJSON axis order. A missing CRS in a
geographic space means EPSG:4326. `kind` records whether coordinates came from
the source, a generated layout, a manual edit, or a derived transform.

## Harvest and emit

Parsers promote coordinates into `location` and stamp the space; promotion
removes the raw keys from `extras`. Emission uses `location`.

| Format | Fields | Space |
| --- | --- | --- |
| PowerWorld aux | `Latitude:1`/`Longitude:1` bus columns, else the bare `Latitude`/`Longitude` pair (`SubNum` stays in extras: it is identity rather than geometry) | geographic |
| pandapower | bus `geo` GeoJSON Point strings | geographic |
| PyPSA | `buses.csv` `x`/`y` | geographic |
| OpenDSS | `Buscoords` | unknown; a diagnostic identifies values within longitude and latitude bounds |
| BMOPF JSON | `longitude`/`latitude` (the BMOPFTools sideload convention; emission is opt in via `BmopfEmitOptions::sideload_coordinates`) | geographic |

MATPOWER, PSS/E, PowerModels, egret, GOC3, PSLF, and Surge carry no geometry.
Emitting a located case to one of them reports the dropped locations, the same
behavior `base_frequency` has; `powerio geo extract` writes the sidecar as the
escape hatch.

## The geographic document

Coordinates also arrive and leave as files of their own: a `Buscoords` CSV
next to a DSS master, a GeoJSON export from a GIS tool, a layout computed by a
renderer. The container is `GeoLayer`, surfaced as `DisplayData::Geo` beside
the PowerWorld `.pwd` display path.

The canonical form is a GeoJSON FeatureCollection with one foreign
member, suggested extension `.geo.json`:

```json
{
  "type": "FeatureCollection",
  "powerio_geo": { "powerio_version": "1.0.0", "space": "geographic", "kind": "source" },
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

Parsing is tolerant; emission is canonical. `GeoLayer::parse_text` takes UTF-8
text plus a file name hint and touches no filesystem. It accepts headerless
buscoords CSV (`bus, x, y`), CSV and JSON records with aliased field names
(`bus_i`/`bus`/`id`, `lat`/`latitude`/`y`, `lon`/`lng`/`longitude`/`x`, branch
endpoint pairs), and GeoJSON Point and LineString features. Features reference
elements by up to three key fields, matched in order: `uid`, then `id`, then
case insensitive `name`. Branch routes additionally fall back to the unordered
`(from, to)` bus pair. A bare integer branch id (`branch`, `branchid`,
`branchnumber`, `catsid`) is accepted during parsing as a 1-based positional row
alias and never written; the durable identity is the payload `uid`. A branch
key never reads from a bare `id` property, because GIS exports and RFC 7946
tooling put a feature row counter there.

`BalancedNetwork::to_geo_layer()` transforms coordinates to a layer, and
`BalancedNetwork::apply_geo_layer(&layer)`
applies and returns a `GeoApplyReport` with the matched and unmatched feature
counts plus `unlocated_buses` and `unlocated_branches`, the elements that
carry no geometry when the pass ends. The two together tell a layer that
matched nothing from a model that needed nothing; `report.require_located()`
is the strict caller's one line check. The multiconductor equivalents attach
through `powerio::dist_geo` (`dist_geo_layer`, `apply_dist_geo_layer`). The CLI
wraps the same surface:

```console
$ powerio geo extract case.aux -o case.geo.json
$ powerio geo apply case.m layout.csv -o placed.m
$ powerio geo convert buscoords.csv -o case.geo.json
```

## PowerWorld display files

The `.pwd` reader returns `DisplayData::PowerWorld` with a `PwdDisplay`: canvas
dimensions, a timestamp, and substation symbols with number, name, and diagram
coordinates.

Four facade helpers connect it to the geo model. `to_geo_layer_from_pwd` lifts
the substation symbols into a diagram space `GeoLayer` (also reachable as
`powerio geo extract case.pwd`); `to_geo_layer_from_aux_text` parses the
`Latitude` and `Longitude` columns of an AUX `Substation` table directly into
a geographic layer; `apply_substation_points` joins either onto buses through
the `SubNum` extras key; and `to_lonlat_from_pwd_mercator` is a documented,
approximate inverse of the projection PowerWorld's auto generated layouts
use, for consumers that want to place a diagram on a map. The component crate
keeps `to_geo_layer_from_aux_substations(&AuxFile)` for parser authors; the
facade does not expose its borrowed parser type.

A bus row of a complete case export carries its own coordinates as well. The
aux reader promotes the substation `Latitude:1`/`Longitude:1` pair, and the
bus's own bare `Latitude`/`Longitude` pair, into `Bus.location`; a promoted
pair leaves extras.

Rust and Python use `parse_display_file`; Rust also has `parse_display` for
already acquired bytes. Python returns
`DisplayData(kind="powerworld", data=PwdDisplay(...))`.
Display files do not pass through `BalancedNetwork`, module emission, or `.pio.json`.

## Distribution graph projection

`MulticonductorNetwork::to_graph()` returns a bus and terminal graph without requiring
coordinates. Python exposes `dist_net.to_graph()`, and the C `dist` feature
exposes `pio_multiconductor_network_to_graph_json`. Graph topology and
geographic placement remain separate data.

PowerIO stores and transports coordinates; it does not compute them. Synthetic
layout of a coordinate free case is renderer math and stays in the consumer,
which can store the result with `kind = synthetic` so the coordinate origin
survives.

The C ABI exposes the document as strings: `pio_geo_parse` normalizes a
tolerant sidecar to the canonical form and returns the parser's diagnostics through
its `PioDiagnostics **out_diagnostics` out parameter,
`pio_balanced_network_to_geo_layer_json` and
`pio_balanced_network_apply_geo_layer` work on a parsed network handle (apply
returns a new handle whose diagnostics carry the match report), and
`pio_multiconductor_network_to_geo_layer_json`/
`pio_multiconductor_network_apply_geo_layer` are the multiconductor
equivalents. The C `*_geo_extract` and `*_geo_apply` symbols remain frozen ABI
6 compatibility names.
Python mirrors the surface with `parse_geo` and
`to_geo_layer()`/`apply_geo_layer()` on both network types.
