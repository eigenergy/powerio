"""Optional source and sink plugins for TransitionZero's interop pipeline.

The entry points support PyPSA staging and the SiennaSchemas 0.2 table contract. External pipeline
packages are imported only when this module is explicitly loaded by interop.
"""
from __future__ import annotations

import json
import io
import tempfile
from pathlib import Path
from typing import ClassVar, Literal

from pydantic import BaseModel
from interop.core.pipeline import StagedSource, State
from interop.core.reporting import ScopedRecorder
from interop.plugins.sources.stage_sienna_system_json import stage_topology
from interop.plugins.sinks.emit_sienna_system_json import EmitSiennaSystemJson
from interop.ports.outbound.filesystem import FilesystemPort, Location
from interop.ports.outbound.reporting import EventKind, TranslationEvent

import powerio as pio

MAX_BYTES = 64 << 20


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
        module = pio.deserialize(data) if params.format == "pio-ir" else pio.parse(data, format=params.format, name=str(params.input_path))
        if params.tables == "pypsa":
            from interop.plugins.sources.stage_pypsa_network_file import stage_network
            from interop.core.extensions import ExtensionKind, append_extensions
            output = pio.emit(module, "pypsa-netcdf")
            report(self._recorder, module.diagnostics)
            report(self._recorder, output.diagnostics)
            staged = stage_network(io.BytesIO(output.artifacts[0].data), staging_dir)
            state = State(staging_dir=staging_dir, source_topology=staged.topology, source_time_series=staged.time_series)
            if staged.network is not None:
                append_extensions(state.source_extensions, ExtensionKind.NETWORK, [staged.network])
            return state
        output = pio.emit(module, "sienna-0-2-json")
        report(self._recorder, module.diagnostics)
        report(self._recorder, output.diagnostics)
        document = json.loads(output.text)
        if document["time_series_associations"] or document["time_series_storage_file"]:
            raise ValueError("Use a Sienna bundle source for documents with time-series companions")
        supported = {"Area", "LoadZone", "ACBus", "Arc", "Line", "MonitoredLine", "ThermalStandard", "RenewableDispatch", "RenewableNonDispatch", "HydroDispatch", "EnergyReservoirStorage", "PowerLoad", "InterruptiblePowerLoad", "TwoTerminalGenericHVDCLine"}
        skipped = [kind for kind, rows in document["components"].items() if rows and kind not in supported]
        if skipped:
            raise ValueError(f"TransitionZero's Sienna staging does not handle {skipped}; use tables='pypsa' to stage these electrical components")
        return State(staging_dir=staging_dir, source_topology=stage_topology(document, staging_dir))


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
        if state.source_time_series or state.destination_time_series or state.destination_tables.get("time_series_metadata") is not None:
            raise ValueError("PowerIO electrical sink requires a static state; temporal data must be emitted as a bundle")
        if state.destination_extensions or state.destination_extension_series:
            raise ValueError("Emit the pipeline's extension sidecar before projecting to an electrical file")
        if params.tables == "pypsa":
            from interop.plugins.sinks.emit_pypsa_network import build_network
            network = build_network(state, None)
            for table, component in (("transformers", "Transformer"), ("shunt_impedances", "ShuntImpedance"), ("stores", "Store")):
                frame = state.destination_tables.get(table)
                if frame is not None and frame.height:
                    data = frame.to_pandas().set_index("name")
                    network.add(component, data.index, **{column: data[column] for column in data.columns})
            with tempfile.TemporaryDirectory(prefix="powerio-interop-") as directory:
                path = Path(directory) / "network.nc"
                network.export_to_netcdf(path)
                module = pio.parse(path, format="pypsa-netcdf")
        else:
            payload = EmitSiennaSystemJson.build_payload(state)
            document = {"schema_version": "0.2.0", "components": payload["components"], "supplemental_attributes": [], "supplemental_attribute_associations": [], "plant_associations": [], "combined_cycle_associations": [], "service_associations": [], "time_series_associations": [], "time_series_storage_file": None}
            module = pio.parse(json.dumps(document).encode(), format="sienna-0-2-json")
        result = pio.serialize(module) if params.format == "pio-ir" else pio.emit(module, params.format)
        report(self._recorder, module.diagnostics)
        report(self._recorder, result.diagnostics)
        if len(result.artifacts) != 1 or result.layout != "file":
            raise ValueError("powerio_sink requires a single-file destination format")
        self._fs.write_bytes(params.output_path, result.artifacts[0].data)
