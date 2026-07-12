# Geographic and display data

PowerIO stores coordinates when a supported source provides them. Coordinates
are optional; readers do not invent them, and network writers without a
coordinate representation report the loss.

PowerWorld `.pwd` files are display data rather than network cases. Parse them
with `parse_display_file` or `parse_display_bytes`, not the network parser.

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
identical.

The coordinate space belongs to the network. For geographic coordinates,
`x` is longitude and `y` is latitude in GeoJSON axis order. A missing CRS in a
geographic space means EPSG:4326. `kind` records whether coordinates came from
the source, a generated layout, a manual edit, or a derived transform.

## Distribution formats

The distribution readers currently provide coordinate data:

| Format | Input | Coordinate space |
| --- | --- | --- |
| OpenDSS | `Buscoords` | unknown; a diagnostic identifies values within longitude and latitude bounds |
| BMOPF JSON | `longitude` and `latitude` fields used by BMOPFTools | geographic |

The OpenDSS writer emits a `Buscoords` file. The BMOPF schema rejects
coordinate properties, so the BMOPF writer drops them with a warning by
default. `BmopfWriteOptions::sideload_coordinates` writes the BMOPFTools
`longitude` and `latitude` extension.

Balanced network readers do not yet populate these fields. That work is
tracked in [#183](https://github.com/eigenergy/powerio/issues/183).

## PowerWorld display files

The `.pwd` reader returns `DisplayData::PowerWorld` with a `PwdDisplay`. The
decoded data contains canvas dimensions, a timestamp, and substation symbols
with number, name, and diagram coordinates. It does not attach those symbols to
a parsed network.

Rust uses `parse_display_file` and `parse_display_bytes`. Python exposes the
same names and returns `DisplayData(kind="powerworld", data=PwdDisplay(...))`.
Display files do not pass through `Network`, `Conversion`, or `.pio.json`.

## Distribution graph projection

`DistNetwork::graph()` returns a bus and terminal graph without requiring
coordinates. Python exposes `dist_net.graph()`, and the C `dist` feature
exposes `pio_dist_graph_json`. Graph topology and geographic placement remain
separate data.

Standalone geographic documents, balanced coordinate readers, and direct C and
Python coordinate helpers are tracked in
[#180](https://github.com/eigenergy/powerio/issues/180),
[#182](https://github.com/eigenergy/powerio/issues/182), and
[#183](https://github.com/eigenergy/powerio/issues/183).
