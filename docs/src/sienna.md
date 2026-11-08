# SiennaSchemas exchange

A balanced `PioModule` can be emitted as a SiennaSchemas SystemDocument. Use
`sienna-json` to retain an input document's version, or to select 0.4.0 for a
new module. Explicit targets are `sienna-0-1-json`, `sienna-0-2-json`,
`sienna-0-3-json`, and `sienna-0-4-json`.

```python
import powerio as pio

module = pio.parse("case.raw")
result = pio.emit(module, "sienna-0-4-json", "case.json")
recovered = pio.parse("case.json")
```

The reader separates component identifiers from bus numbers and converts
component bases and natural units into the neutral electrical model. Voltage
angles and phase shifts change between Sienna radians and PowerIO degrees.
Storage dispatch changes between injection and consumption sign conventions.

The adapter maps AC buses, area membership, lines, transformer circuits,
two- and three-winding transformers, fixed admittances, constant-power and ZIP
loads, static generators, reservoir storage, and generic HVDC lines. Optional
source data remains in the module extension `org.eigenergy.powerio.sienna`,
which survives PowerIO IR serialization. Diagnostics identify data that has no
active electrical mapping. A same-version write overlays edited electrical
values on the retained document. Cross-version writes report source-only fields
that require a separate migration.

Sienna's `ext` map carries PowerIO identifiers and fields without a native schema
counterpart under `org.eigenergy.powerio`, keyed by component ID. Native schema
fields are authoritative. Consumers must handle these keys or list them in their
explicit ignore list. A field retained only in `ext` does not acquire Sienna
solver semantics; emission reports electrical constraints that need this handling.

Time-series associations and dynamics records remain available in the retained
document. The standalone electrical reader does not acquire a referenced HDF5
companion. Keep the companion with the document when moving a bundle.

## PyPSA binary files

The optional `powerio[pypsa-files]` extra enables `pypsa-netcdf` and `pypsa-hdf5`
through PyPSA's own container readers and writers. These transports use PowerIO's
existing CSV electrical and temporal mapping, so the same diagnostics apply.
The Python package imports the optional dependencies only when a binary
transport is used. Input and expanded arrays are bounded to 64 MiB.

```python
pio.emit(module, "pypsa-netcdf", "network.nc")
module = pio.parse("network.nc")
```

## TransitionZero pipelines

The Python distribution registers `powerio_source` and `powerio_sink` in the
`interop.sources` and `interop.sinks` entry-point groups. Install tz-oss-interop
and the PyPSA file extra in the same environment. The source accepts `input_path`
and an optional PowerIO `format`; the sink accepts `output_path` and `format`
(default `pio-ir`).

Both default to `tables: pypsa`, matching the PyPSA table contract. The source
stages transformers and shunt impedances alongside the other component tables.
`tables: sienna` selects the SiennaSchemas 0.2 contract. A Sienna source refuses
component families that the pipeline's staging helper cannot handle.

The sink currently requires a static destination state and a single-file target.
Unconsumed temporal or extension sidecars must be emitted separately. PowerIO
diagnostics are forwarded to the pipeline's recorder.
