# Geographic and display data

PowerIO keeps coordinates when a supported source provides them. They are
optional. No parser invents them, and when you emit a located case to a network
format that has no place for coordinates, the writer reports the loss.

A standalone geographic document parses to `powerio.GeoLayer`, a value like any
other case. The canonical `.geo.json`, plain GeoJSON, CSV or JSON records with
aliased field names, headerless buscoords CSV, and a PowerWorld `.pwd` display
all parse to it; a `.pwd` becomes a drawing layer with supported bus, branch,
and substation objects. A drawing with no decoded equipment returns an error.

```rust,ignore
use powerio::{PioValue, emit, parse, serialize};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = parse("layer.geo.json")?;
    let PioValue::GeoLayer(layer) = module.value() else {
        panic!("a layer document parses to powerio.GeoLayer");
    };

    // A layer travels through PowerIO IR and out as the canonical document.
    serialize(&module, "layer.pio.json")?;
    emit(&module, "geo-json", "layer.geo.json")?;
    Ok(())
}
```

If you need the raw display record, `PwdDisplay` is still available; it has the
canvas, the save stamp, and the symbol table in diagram coordinates.

## Coordinate fields

Balanced and multiconductor networks use the same JSON shape for coordinates:

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

Balanced networks reach these types as
`powerio::geo::{Location, CoordsKind, CoordinateSpace, GeoMeta, Canvas}`
through `BalancedNetwork.geo` and `Bus.location`; multiconductor networks use
the matching `powerio_dist::geo` types through `MulticonductorNetwork.geo` and
`DistBus.location`, and a package serialization test keeps the two JSON shapes
identical. A branch can also have a polyline route (`Branch.route`,
`DistLine.route`) when the source provides intermediate geometry. Without one,
a renderer draws the branch endpoint to endpoint from the bus locations.

The coordinate space belongs to the network, and a point sets its own `kind`
only when its origin differs from the network default. For geographic
coordinates, `x` is longitude and `y` is latitude, which is GeoJSON axis order,
and a missing CRS means EPSG:4326. `kind` says whether the coordinates came
from the source, a generated layout, a manual edit, or a derived transform.

## Harvest and emit

A parser promotes source coordinates into `location`, sets the space, and
removes the raw keys from `extras`. Writers read `location`.

| Format | Fields | Space |
| --- | --- | --- |
| PowerWorld PWB | Validated bus latitude/longitude fields in supported 425/537 record layouts | geographic |
| PowerWorld aux | `Latitude:1`/`Longitude:1` bus columns, else the bare `Latitude`/`Longitude` pair (`SubNum` stays in extras: it is identity rather than geometry) | geographic |
| pandapower | bus `geo` GeoJSON Point strings | geographic |
| PyPSA | `buses.csv` `x`/`y` | geographic |
| DOE GO Challenge 3 | bus `longitude`/`latitude` | geographic |
| OpenDSS | `Buscoords` | unknown; a diagnostic identifies values within longitude and latitude bounds |
| BMOPF JSON | BMOPFTools `bus[id].geo` Point and `line[id].geo` LineString; legacy `longitude`/`latitude`; canonical `extras.geojson` | declared CRS, geographic when omitted |

MATPOWER, PSS/E, PowerModels, egret, PSLF, and Surge have no place for
geometry. Emitting a located case to one of them reports the dropped locations,
in the same way it reports a dropped `base_frequency`. If you need the
coordinates to survive, `powerio geo extract` writes them as a separate layer
document.

## The geographic document

Coordinates also arrive and leave as files of their own, such as a `Buscoords`
CSV next to a DSS master or a GeoJSON export from a GIS tool, and a renderer
can hand back the layout it computed the same way. The container for such a
file is `GeoLayer`, which the Rust facade's `parse` returns as
`PioValue::GeoLayer`.

The canonical document is a GeoJSON FeatureCollection with one foreign member,
`powerio_geo`, and the suggested extension is `.geo.json`:

```json
{
  "type": "FeatureCollection",
  "powerio_geo": { "powerio_version": "0.11.0", "space": "geographic", "kind": "source" },
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

Parsing is tolerant and emission is canonical. `GeoLayer::parse` takes UTF-8
text plus a file name hint and touches no filesystem. It accepts headerless
buscoords CSV (`bus, x, y`), CSV and JSON records with aliased field names
(`bus_i`/`bus`/`id`, `lat`/`latitude`/`y`, `lon`/`lng`/`longitude`/`x`, branch
endpoint pairs), and GeoJSON Point and LineString features. A feature refers to
its element by up to three key fields, tried in order: `uid`, then `id`, then
case insensitive `name`. A branch route can also fall back to the unordered
`(from, to)` bus pair. A bare integer branch id (`branch`, `branchid`,
`branchnumber`, `catsid`) is accepted on the way in as a one based positional
row alias and is not written back; the `uid` in the payload is the durable key.
A branch key accepts `id` when `kind` explicitly identifies a line or branch.
An untyped feature counter does not identify equipment. BMOPFTools
`bus_from`/`bus_to` endpoint names are accepted. A CRS string or a named CRS
object retains its identifier; PowerIO does not reproject the coordinates.

`BalancedNetwork::to_geo_layer()` turns a network's coordinates into a layer,
and `BalancedNetwork::apply_geo_layer(&layer)` applies one and returns a
`GeoApplyReport` with the matched and unmatched feature counts plus
`unlocated_buses` and `unlocated_branches`, the elements still without geometry
when the pass ends. Together those counts let you tell a layer that matched
nothing from a model that needed nothing, and `report.require_located()` is the
one line check for a caller that wants everything placed. The multiconductor
equivalents, `to_dist_geo_layer` and `apply_dist_geo_layer`, live in
`powerio::dist_geo`. The CLI wraps the same functions:

```console
$ powerio geo extract case.aux -o case.geo.json
$ powerio geo apply case.m layout.csv -o placed.m
$ powerio geo convert buscoords.csv -o case.geo.json
```

## PowerWorld display files

The `.pwd` reader returns a drawing `GeoLayer` with decoded bus positions,
branch paths, and substation positions. Coordinates remain in drawing units.
Unsupported or unmatched bus and branch records produce diagnostics. Read a
display with `parse`. Python's `parse_display`, which returns
`DisplayData(kind="powerworld", data=PwdDisplay(...))` with the canvas
dimensions, the save stamp, and the substation symbols, remains only for 0.11
compatibility, reads the whole file without the input byte limits, and will be
removed in 0.12.

The facade helpers connect it to the geo model. `to_geo_layer_from_pwd` lifts
the substation symbols into a diagram space `GeoLayer`, which is also what
`powerio geo extract case.pwd` does. `to_geo_layer_from_aux_text` parses the
`Latitude` and `Longitude` columns of an AUX `Substation` table straight into a
geographic layer. `apply_substation_points` joins either layer onto buses
through the `SubNum` extras key. `to_lonlat_from_pwd_mercator` is a documented,
approximate inverse of the projection PowerWorld's auto generated layouts use,
for when you want to place a diagram on a map. The component crate keeps
`to_geo_layer_from_aux_substations(&AuxFile)` for parser authors, since the
facade does not expose its borrowed parser type.

A bus row in a complete case export has its own coordinates as well. The aux
reader promotes the substation `Latitude:1`/`Longitude:1` pair, or, when those
are absent, the bus's own bare `Latitude`/`Longitude` pair, into
`Bus.location`, and a promoted pair leaves extras.

In Rust a display file parses to `PioValue::GeoLayer`. There is no path from a
display file to a `BalancedNetwork`; it is always a layer, and both module
emission and PowerIO IR handle it as one.

## Distribution graph projection

`MulticonductorNetwork::to_graph()` returns a bus and terminal graph and does
not need coordinates to do it; in Python that is `dist_net.to_graph()`. Graph
topology and geographic placement stay separate data.

PowerIO stores and transports coordinates and does not compute them. Laying out
a case that has none is renderer math and belongs in the consumer, which can
store the result with `kind = synthetic` so the origin of the coordinates
survives.

Python has `parse_geo`, and both network types have `to_geo_layer()` and
`apply_geo_layer()`.

## BMOPF geometry and schema validity

PowerIO reads the geometry proposed in
[BMOPFTools #258](https://github.com/frederikgeth/BMOPFTools.jl/pull/258):
`bus[id].geo` contains a Point, `line[id].geo` contains a LineString, and
`meta.crs` names the coordinate reference system. An absent CRS means WGS84.
Coordinates appear in longitude/latitude order for WGS84. A malformed vertex
invalidates its entire path; PowerIO does not connect the remaining vertices.

The schema currently identified as draft BMOPF 0.2 does not declare these
per-equipment `geo` fields. Explicit BMOPF output therefore stores a
FeatureCollection in the permitted `extras.geojson` location. Its features use
`kind`, `id`, `bus_from`, and `bus_to`. The `powerio_geo` member preserves
coordinate-space metadata for drawings and coordinates with no declared CRS.
This keeps output valid against its stated schema while retaining geometry.
Task Force review determines the eventual standard location.

Same-type source emission preserves the input bytes. Editing or explicitly
selecting a schema version writes the typed coordinates, so a retained source
cannot override updated positions. Both values of the compatibility setting
`BmopfEmitOptions::sideload_coordinates` preserve geometry in `extras.geojson`.
PowerIO IR also retains typed bus locations, line paths, and coordinate metadata.

A RAW revision 33 file does not contain the New England PWB bus-location
records. Attach its matching GeoJSON, CSV, AUX, or PWD companion explicitly.
PWD positions describe a drawing and must not be interpreted as latitude and
longitude.
