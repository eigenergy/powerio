# Sienna exchange

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
two- and three-winding transformers, fixed and switched admittances, constant-power and ZIP
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
companion.

## Document and HDF5 bundles

Install `powerio[sienna-files]` for the Python `sienna-bundle` transport, or
select `sienna-0-1-bundle` through `sienna-0-4-bundle` explicitly.

```python
module = pio.parse("system.json", format="sienna-bundle")
pio.serialize(module, "system.pio.json")
pio.emit(module, "sienna-bundle", "exported-system")
```

A bundle contains `system.json` and the named HDF5 companion. Input companions
must stay within the acquisition root; memory input supplies them through
`named_buffers`. Numeric HDF5 arrays, shapes, and attributes survive IR in
`org.eigenergy.powerio.sienna.arrays`. Bounds cover acquired bytes, expanded
arrays, and object counts. External HDF5 links and object arrays are rejected.
An existing output directory is never replaced. The electrical value interprets
components; retained forecast arrays keep the meaning declared by the Sienna
association records. Tests reopen emitted arrays with InfraStore 0.14.1 and validate association
records against every released schema. Bundle writes migrate the common time-series
associations across releases when their component owners remain in the target.
The explicit `zoneless` declaration keeps its wall-clock meaning: 0.4 writes an
offset-free timestamp, while older releases use InfraStore's legacy `Z` spelling.
The TransitionZero adapter selects 0.2 before handing associations to its pinned
InfraStore consumer. Per-unit load series retain their declared device base; other device-base edits
that need array rebasing are rejected. Other source-only records still require
separate migration.

## PowerSystems.jl serialized systems

`powersystems-json` reads native PowerSystems data formats 4.0.0 and 5.0.0,
and writes 5.0.0. This is the serialized Julia `System` layout, with UUID
references and stored device bases. It is distinct from a SiennaSchemas
SystemDocument. The format is detected from `data_format_version`.

```python
pio.emit(module, "powersystems-json", "system.json")
```

The generated file loads with `PowerSystems.System("system.json")`.
Compatibility checks cover PowerSystems 4.6.2 inputs and 5.9.0/5.12.5 consumers.
PowerSystems 5.9 predates native switched-shunt control mode and remote-bus
fields. A PowerIO extension preserves these through that runtime; native
fields take precedence in newer consumers. The 5.12.5 check verifies those
controls directly. PowerNetworkMatrices versions that omit switched admittances
from Ybus require the initial injection to be handled separately, as emission
diagnostics state.

The native adapter shares the electrical mapping above, including explicit star
circuits for three-winding equipment. Parallel devices share their native arc.
PowerSystems companion time-series files and masked components remain outside
this standalone electrical projection. Unknown native records stay in
`org.eigenergy.powerio.powersystems` in a serialized PowerIO module.

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

Both default to `tables: pypsa`, matching the PyPSA table layout. The source
stages transformers and shunt impedances alongside the other component tables.
`tables: sienna` selects the SiennaSchemas 0.2 tables. A Sienna source refuses
component families that the pipeline's staging helper cannot handle.

The sink accepts PyPSA electrical time series and a single-file target. Sienna
temporal staging accepts scalar SingleTimeSeries from any released `sienna-bundle`
version with `tables: sienna`, converting component bases to the pipeline's
capacity-based series. Forecast arrays remain available in bundles and IR.
The Sienna sink preserves its time-series association rows and HDF5 arrays in
`pio-ir` output. A `pio-ir` sink
retains pipeline extension records and their companion columns in the module's
`org.eigenergy.powerio.transitionzero` extension, including timestamp units and
timezones. They return to the pipeline when `powerio_source` reads that IR.
These records do not add planning constraints to the electrical value. Other
file targets require the pipeline's extension sidecar to be emitted separately.
PowerIO diagnostics are forwarded to the pipeline's recorder.
