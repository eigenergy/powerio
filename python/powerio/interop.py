"""Optional source and sink plugins for TransitionZero's interop pipeline.

The entry points support PyPSA staging and the SiennaSchemas 0.2 table layout. External pipeline
packages are imported only when this module is explicitly loaded by interop.
"""
from __future__ import annotations

import io
import json
import tempfile
from pathlib import Path
from typing import Any, ClassVar, Literal

from interop.core.pipeline import StagedSource, State
from interop.core.reporting import ScopedRecorder
from interop.plugins.sinks.emit_sienna_system_json import EmitSiennaSystemJson
from interop.plugins.sources.stage_sienna_system_json import stage_topology
from interop.ports.outbound.filesystem import FilesystemPort, Location
from interop.ports.outbound.reporting import EventKind, TranslationEvent
from pydantic import BaseModel

import powerio as pio

MAX_BYTES = 64 << 20
EXTENSION_KEY = "org.eigenergy.powerio.transitionzero"


def report(recorder: ScopedRecorder, diagnostics) -> None:
    for finding in diagnostics:
        code = str(finding.code)
        if code.endswith("VALUE_DEFAULTED"):
            kind = EventKind.TRANSLATOR_DEFAULT_APPLIED
        elif code.endswith(("RECORD_DROPPED", "COMPONENT_SKIPPED")):
            kind = EventKind.COMPONENT_SKIPPED
        else:
            kind = EventKind.NOT_MAPPED
        recorder.append(TranslationEvent(kind=kind, note=f"{code}: {finding.message}"))


class PowerIOSourceParams(BaseModel):
    input_path: Location
    format: str | None = None
    tables: Literal["pypsa", "sienna"] = "pypsa"


class PowerIOSource(StagedSource):
    name: ClassVar[str] = "powerio_source"
    prefix: ClassVar[str] = "powerio"
    params_schema: ClassVar[type[BaseModel]] = PowerIOSourceParams

    def __init__(self, fs: FilesystemPort, recorder: ScopedRecorder) -> None:
        self._fs = fs
        self._recorder = recorder

    def load_into_state(self, params: BaseModel | None, staging_dir: Path) -> State:
        if not isinstance(params, PowerIOSourceParams):
            raise TypeError("powerio_source requires PowerIOSourceParams")
        with self._fs.open_read(params.input_path) as stream:
            data = stream.read(MAX_BYTES + 1)
        if len(data) > MAX_BYTES:
            raise ValueError("PowerIO source exceeds the 64 MiB acquisition limit")
        from . import _sienna_files
        companions = None
        if params.format in _sienna_files.FORMATS:
            filename = json.loads(data).get("time_series_storage_file")
            if filename:
                filename = _sienna_files._name(filename)
                with self._fs.open_read(self._fs.resolve(params.input_path, filename)) as stream:
                    companion = stream.read(MAX_BYTES + 1)
                companions = {filename: companion}
        module = pio.deserialize(data) if params.format == "pio-ir" else pio.parse(data, format=params.format, name=str(params.input_path), named_buffers=companions)
        has_arrays = _sienna_files.KEY in module.extension_keys
        if has_arrays and params.tables == "pypsa":
            raise ValueError("Choose tables='sienna' to stage Sienna companion time series before applying the pipeline's temporal translation steps")
        if params.tables == "pypsa":
            from interop.core.extensions import ExtensionKind, append_extensions
            from interop.plugins.sources.stage_pypsa_network_file import stage_network
            output = pio.emit(module, "pypsa-netcdf")
            report(self._recorder, module.diagnostics)
            report(self._recorder, output.diagnostics)
            data = output.artifacts[0].data
            if data is None:
                raise ValueError("Expected an in-memory PyPSA output")
            staged = stage_network(io.BytesIO(data), staging_dir)
            state = State(staging_dir=staging_dir, source_topology=staged.topology, source_time_series=staged.time_series)
            if staged.network is not None:
                append_extensions(state.source_extensions, ExtensionKind.NETWORK, [staged.network])
            restore_extensions(module, state)
            return state
        output = pio.emit(module, "sienna-0-2-bundle" if has_arrays else "sienna-0-2-json")
        report(self._recorder, module.diagnostics)
        report(self._recorder, output.diagnostics)
        text = next((a.data for a in output.artifacts if a.name == "system.json"), None) if has_arrays else output.text
        if text is None:
            raise ValueError("Expected a Sienna JSON document")
        document = json.loads(text)
        if (document["time_series_associations"] or document["time_series_storage_file"]) and not has_arrays:
            raise ValueError("Use a Sienna bundle source for documents with time-series companions")
        supported = {"Area", "LoadZone", "ACBus", "Arc", "Line", "MonitoredLine", "ThermalStandard", "RenewableDispatch", "RenewableNonDispatch", "HydroDispatch", "EnergyReservoirStorage", "PowerLoad", "InterruptiblePowerLoad", "TwoTerminalGenericHVDCLine"}
        skipped = [kind for kind, rows in document["components"].items() if rows and kind not in supported]
        if skipped:
            raise ValueError(f"TransitionZero's Sienna staging does not handle {skipped}; use tables='pypsa' to stage these electrical components")
        normalize_sienna_staging_units(document)
        from interop.core.extensions import (
            ExtensionKind,
            GeneratorExtension,
            append_extensions,
        )
        from interop.plugins.shared.sienna_constants import (
            PrimeMover,
            SiennaComponent,
            ThermalFuel,
        )
        from interop.plugins.shared.sienna_pypsa_translations.constants import (
            pypsa_carrier,
        )
        if has_arrays:
            from interop.plugins.sources.stage_sienna_system_json import stage_system
            companion = next(a.data for a in output.artifacts if a.name == document["time_series_storage_file"])
            if companion is None:
                raise ValueError("Expected in-memory Sienna companion")
            factors = sienna_series_factors(document)
            staged_system = stage_system(document, io.BytesIO(companion), staging_dir)
            if factors:
                import polars as pl
                for key, by_name in factors.items():
                    frame = staged_system.time_series.get(key)
                    if frame is not None:
                        staged_system.time_series[key] = frame.with_columns((pl.col("value") * pl.col("component").replace_strict(by_name, default=1.0, return_dtype=pl.Float64)).alias("value"))
            state = State(staging_dir=staging_dir, source_topology=staged_system.topology, source_time_series=staged_system.time_series)
        else:
            state = State(staging_dir=staging_dir, source_topology=stage_topology(document, staging_dir))
        for kind in ("ThermalStandard", "RenewableDispatch", "RenewableNonDispatch"):
            for row in document["components"].get(kind, []):
                try:
                    pypsa_carrier(SiennaComponent(kind), PrimeMover(row["prime_mover_type"]), ThermalFuel(row["fuel"]) if kind == "ThermalStandard" else None)
                except KeyError:
                    # An unspecified technology remains unspecified in PyPSA.
                    append_extensions(state.source_extensions, ExtensionKind.GENERATOR, [GeneratorExtension(name=row["name"], carrier="other")])
                    self._recorder.append(TranslationEvent(kind=EventKind.NOT_MAPPED, note=f"{row['name']}: upstream carrier mapping has no entry for this technology; using other"))
        restore_extensions(module, state)
        return state


class PowerIOSinkParams(BaseModel):
    output_path: Location
    format: str = "pio-ir"
    tables: Literal["pypsa", "sienna"] = "pypsa"


class PowerIOSink:
    name: ClassVar[str] = "powerio_sink"
    params_schema: ClassVar[type[BaseModel]] = PowerIOSinkParams
    writes_user_mappings = None

    def __init__(self, fs: FilesystemPort, recorder: ScopedRecorder) -> None:
        self._fs = fs
        self._recorder = recorder

    def write(self, state: State, params: BaseModel | None) -> None:
        if not isinstance(params, PowerIOSinkParams):
            raise TypeError("powerio_sink requires PowerIOSinkParams")
        has_extensions = any(record.has_any_value() for records in state.destination_extensions.values() for record in records) or bool(state.destination_extension_series)
        if has_extensions and params.format != "pio-ir":
            raise ValueError("Use pio-ir to retain pipeline extensions, or emit the pipeline's extension sidecar before projecting to an electrical file")
        if params.tables == "pypsa":
            network = build_pypsa_network(state)
            with tempfile.TemporaryDirectory(prefix="powerio-interop-") as directory:
                path = Path(directory) / "network.nc"
                network.export_to_netcdf(path)
                module = pio.parse(path, format="pypsa-netcdf")
        else:
            supported = {"Area", "ACBus", "Arc", "PowerLoad", "InterruptiblePowerLoad", "ThermalStandard", "RenewableDispatch", "RenewableNonDispatch", "HydroDispatch", "EnergyReservoirStorage", "Line", "TwoTerminalGenericHVDCLine"}
            unknown = set(state.destination_tables) - supported - {"TimeSeriesAssociation"}
            if unknown:
                raise ValueError(f"Unrecognized Sienna destination tables: {sorted(unknown)}")
            payload = EmitSiennaSystemJson.build_payload(state)
            document = {"schema_version": "0.2.0", "components": payload["components"], "supplemental_attributes": [], "supplemental_attribute_associations": [], "plant_associations": [], "combined_cycle_associations": [], "service_associations": [], "time_series_associations": [], "time_series_storage_file": None}
            if state.destination_tables.get("TimeSeriesAssociation") is not None:
                from interop.plugins.sinks.emit_sienna_h5_sidecar import (
                    EmitSiennaH5Sidecar,
                )
                buffer = io.BytesIO()
                document["time_series_associations"] = EmitSiennaH5Sidecar.write_h5(state, buffer)
                document["time_series_storage_file"] = "time_series.h5"
                module = pio.parse(json.dumps(document).encode(), format="sienna-0-2-bundle", named_buffers={"time_series.h5": buffer.getvalue()})
            else:
                if state.destination_time_series:
                    raise ValueError("Sienna time series require time_series_association rows")
                module = pio.parse(json.dumps(document).encode(), format="sienna-0-2-json")
        if has_extensions:
            module = preserve_extensions(module, state)
            self._recorder.append(TranslationEvent(kind=EventKind.NOT_MAPPED, note="Pipeline extensions remain in the PowerIO module; the electrical value does not interpret their planning or scheduling semantics"))
        result = pio.serialize(module) if params.format == "pio-ir" else pio.emit(module, params.format)
        report(self._recorder, module.diagnostics)
        report(self._recorder, result.diagnostics)
        if len(result.artifacts) != 1 or result.layout != "file":
            raise ValueError("powerio_sink requires a single-file destination format")
        self._fs.write_bytes(params.output_path, result.artifacts[0].data)


def build_pypsa_network(state: State):
    """Apply native PyPSA tables without rounding electrical parameters.

    The pipeline's existing temporal assembler resolves its UUID-based series
    metadata. Static tables may also come directly from a PyPSA staging step.
    """
    import pypsa
    metadata = state.destination_tables.get("time_series_metadata")
    if metadata is not None and metadata.height:
        from interop.plugins.sinks.emit_pypsa_network import build_network
        network = build_network(state, None)
    else:
        if state.destination_time_series:
            raise ValueError("Destination time series require time_series_metadata")
        network = pypsa.Network()
    components = {
        "carriers": "Carrier", "buses": "Bus", "generators": "Generator",
        "loads": "Load", "lines": "Line", "transformers": "Transformer",
        "shunt_impedances": "ShuntImpedance", "storage_units": "StorageUnit",
        "stores": "Store", "links": "Link", "global_constraints": "GlobalConstraint",
    }
    unknown = set(state.destination_tables) - components.keys() - {"time_series_metadata"}
    if unknown:
        raise ValueError(f"Unrecognized PyPSA destination tables: {sorted(unknown)}")
    for table, component in components.items():
        frame = state.destination_tables.get(table)
        if frame is None or not frame.height:
            continue
        data = frame.to_pandas().set_index("name")
        if data.index.has_duplicates:
            raise ValueError(f"Duplicate names in {table}")
        current = network.components[component].static
        missing = data.index.difference(current.index)
        if len(missing):
            attributes = {column: data.loc[missing, column] for column in data.columns}
            network.add(component, missing, **attributes)
        current = network.components[component].static
        for column in data.columns:
            # A null in a mapped optional field means the consumer's default.
            supplied = data[column].dropna()
            if not supplied.empty:
                current.loc[supplied.index, column] = supplied
    return network


def sienna_series_factors(document: dict) -> dict[tuple[str, str], dict[str, float]]:
    """Translate declared device bases to the pipeline's capacity-based series."""
    from interop.plugins.shared.sienna_time_series import (
        build_capacity_by_owner,
        has_series_capacity,
    )
    owners = {(kind, row["id"]): row for kind, rows in document["components"].items() for row in rows}
    factors: dict[tuple[str, str], dict[str, float]] = {}
    for association in document.get("time_series_associations", []):
        key = (association["owner_type"], association["owner_id"])
        owner = owners.get(key)
        if association["time_series_type"] != "SingleTimeSeries" or association.get("element_shape"):
            raise ValueError("TransitionZero staging requires scalar SingleTimeSeries; retain forecast arrays in a Sienna bundle or PowerIO IR")
        if owner is None or not owner.get("name") or not has_series_capacity(key[0]):
            raise ValueError("TransitionZero cannot stage time series for this Sienna owner")
        capacity = build_capacity_by_owner([key], document["components"])[key]
        if capacity <= 0:
            raise ValueError("TransitionZero temporal staging requires a positive owner capacity")
        if association.get("unit_system") == "COMPONENT_BASE":
            factor = owner["base_power"] / capacity
            factors.setdefault((key[0], association["name"]), {})[owner["name"]] = factor
    return factors


def normalize_sienna_staging_units(document: dict) -> None:
    """Use the unit bases accepted by the upstream Sienna-to-PyPSA steps."""
    from power_openapi_models.core import models as core_models
    from power_openapi_models.operations import models
    for kind, rows in document["components"].items():
        model = getattr(models, kind, None) or getattr(core_models, kind)
        document["components"][kind] = [model.model_validate(row).model_dump(mode="json", by_alias=True) for row in rows]
    fields = {
        "Line": ("rating", "rating_b", "rating_c", "active_power_flow", "reactive_power_flow"),
        "MonitoredLine": ("rating", "rating_b", "rating_c", "active_power_flow", "reactive_power_flow"),
        "EnergyReservoirStorage": ("rating", "active_power", "reactive_power", "input_active_power_limits", "output_active_power_limits", "reactive_power_limits"),
    }
    def scale(value, factor):
        if isinstance(value, dict):
            return {key: scale(item, factor) for key, item in value.items()}
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            return value * factor
        return value
    for kind, keys in fields.items():
        for row in document["components"].get(kind, []):
            if row["power_units"] == "NATURAL_UNITS":
                for key in keys:
                    if key in row:
                        row[key] = scale(row[key], 1.0 / row["base_power"])
                row["power_units"] = "COMPONENT_BASE"


def preserve_extensions(module, state: State):
    """Keep pipeline records and named companion columns through IR serialization."""
    import polars as pl
    from interop.core.extensions import build_extensions
    payload = {"version": 1, "records": build_extensions(state.destination_extensions).model_dump(mode="json", exclude_none=True), "series": {}}
    for kind, lazy in state.destination_extension_series.items():
        frame = lazy.limit(1_000_001).collect(engine="streaming")
        if frame.height > 1_000_000 or frame.estimated_size() > MAX_BYTES:
            raise ValueError("Pipeline extension series exceed the bounded IR transport profile")
        schema = {}
        for column, dtype in frame.schema.items():
            if isinstance(dtype, pl.Datetime):
                schema[column] = {"type": "datetime", "unit": dtype.time_unit, "zone": dtype.time_zone}
                frame = frame.with_columns(pl.col(column).cast(pl.Int64))
            elif dtype == pl.String:
                schema[column] = {"type": "string"}
            elif dtype in (pl.Float32, pl.Float64):
                if not frame[column].drop_nulls().is_finite().all():
                    raise ValueError("Pipeline extension series require finite numbers")
                schema[column] = {"type": "float"}
            elif dtype in (pl.Int32, pl.Int64):
                schema[column] = {"type": "integer"}
            elif dtype == pl.Boolean:
                schema[column] = {"type": "boolean"}
            else:
                raise ValueError(f"Unsupported pipeline extension column type: {column}: {dtype}")
        payload["series"][str(kind)] = {"schema": schema, "order": frame.columns, "columns": frame.to_dict(as_series=False)}
    text = json.dumps(payload, allow_nan=False)
    if len(text.encode()) > MAX_BYTES:
        raise ValueError("Pipeline extensions exceed the 64 MiB IR transport limit")
    # The checked text is what with_extension would serialize; attach it directly.
    return pio.PioModule(module._inner._with_extension_json(EXTENSION_KEY, text))


def restore_extensions(module, state: State) -> None:
    import polars as pl
    from interop.core.extensions import (
        ExtensionKind,
        parse_extensions,
        stage_extensions,
    )
    payload = module.extension(EXTENSION_KEY)
    if payload is None:
        return
    if payload.get("version") != 1:
        raise ValueError("Unsupported PowerIO pipeline extension version")
    # The retained records are authoritative for these names; unrelated network
    # metadata produced during staging remains available.
    for kind, records in stage_extensions(parse_extensions(payload["records"])).items():
        names = {record.name for record in records}
        state.source_extensions[kind] = [r for r in state.source_extensions.get(kind, []) if r.name not in names] + records
    for kind, table in payload["series"].items():
        schema: dict[str, Any] = {}
        for column, dtype in table["schema"].items():
            key = dtype["type"]
            if key == "datetime":
                schema[column] = pl.Datetime(dtype["unit"], dtype["zone"])
            else:
                types = {"string": pl.String, "float": pl.Float64, "integer": pl.Int64, "boolean": pl.Boolean}
                if key not in types:
                    raise ValueError(f"Unsupported retained pipeline column type: {key}")
                schema[column] = types[key]
        state.source_extension_series[ExtensionKind(kind)] = pl.DataFrame(table["columns"], schema=schema).select(table["order"]).lazy()


