"""Bounded Sienna document/HDF5 bundles with durable structured array retention.

The electrical value owns supported component fields. The module extension
keeps companion arrays and their format metadata through PowerIO IR; forecast
and planning semantics remain stated by the original Sienna associations.
"""
from __future__ import annotations

import copy
import io
import json
import math
import os
from pathlib import Path
from typing import Any

KEY = "org.eigenergy.powerio.sienna.arrays"
MAX_BYTES = 64 << 20
MAX_ITEMS = 4096
FORMATS = {"sienna-bundle": "sienna-json", **{f"sienna-0-{i}-bundle": f"sienna-0-{i}-json" for i in range(1, 5)}}


def _name(value: str) -> str:
    if not isinstance(value, str) or not value or value in {".", ".."} or any(c in value for c in ("/", "\\", "\x00", ":")):
        raise ValueError("Sienna companion must be a single relative filename")
    return value


def _array(value):
    import numpy as np
    array = np.asarray(value)
    if array.dtype.kind not in "biufSU":
        raise ValueError(f"Unsupported HDF5 value type {array.dtype}")
    if array.nbytes > MAX_BYTES:
        raise ValueError("HDF5 array exceeds the 64 MiB limit")
    values = array.reshape(-1).tolist()
    if array.dtype.kind == "f" and not np.isfinite(array).all():
        values = [v if math.isfinite(v) else str(v) for v in values]
    record = {"dtype": array.dtype.str, "shape": list(array.shape), "values": values}
    if array.dtype.kind == "S":
        # Fixed-width strings are bytes. UTF-8 text stays readable in IR;
        # other bytes are stated as hex so JSON carries them unchanged.
        try:
            record["values"] = [v.decode("utf-8") for v in values]
        except UnicodeDecodeError:
            record["encoding"] = "hex"
            record["values"] = [v.hex() for v in values]
    return record


def _decode(value):
    import numpy as np
    dtype = np.dtype(value["dtype"])
    shape = value["shape"]
    if dtype.kind not in "biufSU" or len(shape) > 8 or any(not isinstance(n, int) or n < 0 for n in shape):
        raise ValueError("Invalid HDF5 array metadata")
    count = 1
    for n in shape:
        count *= n
        if count * max(dtype.itemsize, 1) > MAX_BYTES:
            raise ValueError("HDF5 array exceeds the 64 MiB limit")
    values = value["values"]
    if count != len(values):
        raise ValueError("HDF5 array shape does not match values")
    if dtype.kind == "S":
        encoding = value.get("encoding", "utf-8")
        if encoding not in {"utf-8", "hex"} or not all(isinstance(v, str) for v in values):
            raise ValueError("Invalid HDF5 string values")
        values = [bytes.fromhex(v) if encoding == "hex" else v.encode("utf-8") for v in values]
    return np.asarray(values, dtype=dtype).reshape(shape)


def _read_hdf(data: bytes) -> str:
    """The companion's array records as JSON text, serialized once."""
    import h5py
    total = 0
    records = []
    with h5py.File(io.BytesIO(data), "r") as store:
        seen = set()
        def visit(group, path):
            nonlocal total
            address = h5py.h5o.get_info(group.id).addr
            if address in seen:
                raise ValueError("HDF5 aliases and cycles are unsupported")
            seen.add(address)
            attrs = {key: _array(value) for key, value in group.attrs.items()}
            total += sum(_decode(v).nbytes for v in attrs.values())
            record = {"path": path, "attributes": attrs}
            if isinstance(group, h5py.Dataset):
                if group.is_virtual or group.external:
                    raise ValueError("External HDF5 data is unsupported")
                total += group.size * max(group.dtype.itemsize, 1)
                if total > MAX_BYTES:
                    raise ValueError("Expanded HDF5 arrays exceed the 64 MiB limit")
                record["array"] = _array(group[()])
            records.append(record)
            if len(records) > MAX_ITEMS or total > MAX_BYTES:
                raise ValueError("Sienna companion exceeds acquisition limits")
            if isinstance(group, h5py.Group):
                for name in group:
                    _name(name)
                    if not isinstance(group.get(name, getlink=True), h5py.HardLink):
                        raise ValueError("HDF5 external and soft links are unsupported")
                    visit(group[name], f"{path}/{name}" if path else name)
        visit(store, "")
    text = json.dumps(records)
    if len(text.encode()) > MAX_BYTES:
        raise ValueError("Structured HDF5 arrays exceed the 64 MiB limit")
    return text


def _write_hdf(records):
    import h5py
    if not isinstance(records, list) or len(records) > MAX_ITEMS:
        raise ValueError("Invalid Sienna array records")
    buffer = io.BytesIO()
    total = 0
    seen = set()
    with h5py.File(buffer, "w") as store:
        for record in records:
            path = record["path"]
            if path in seen:
                raise ValueError("Duplicate HDF5 path")
            seen.add(path)
            for part in path.split("/") if path else []:
                _name(part)
            if "array" in record:
                array = _decode(record["array"])
                total += array.nbytes
                if total > MAX_BYTES:
                    raise ValueError("Expanded HDF5 arrays exceed the 64 MiB limit")
                obj = store.create_dataset(path, data=array)
            else:
                obj = store.require_group(path) if path else store
            for key, value in record["attributes"].items():
                array = _decode(value)
                total += array.nbytes
                if total > MAX_BYTES:
                    raise ValueError("HDF5 attributes exceed the 64 MiB limit")
                # A 0-d array keeps its stored type. h5py has no numpy
                # unicode type, so text becomes a variable-length string.
                obj.attrs[key] = array.item() if array.dtype.kind == "U" else array
    data = buffer.getvalue()
    if len(data) > MAX_BYTES:
        raise ValueError("HDF5 output exceeds the 64 MiB limit")
    return data


def parse(source: Any, format: str, *, name=None, acquisition_root=None, named_buffers=None):
    import powerio as pio
    if isinstance(source, (str, os.PathLike)):
        if name is not None or named_buffers is not None:
            raise ValueError("Path input cannot also supply name or named_buffers")
        path = Path(source)
        root = Path(acquisition_root).resolve() if acquisition_root is not None else path.parent.resolve()
        if not path.resolve().is_relative_to(root):
            raise ValueError("Sienna document is outside acquisition_root")
        if path.stat().st_size > MAX_BYTES:
            raise ValueError("Sienna document exceeds the 64 MiB limit")
        data = path.read_bytes()
        source_name = path.name
    else:
        if acquisition_root is not None:
            raise ValueError("Memory input uses named_buffers for companions")
        data = bytes(source) if isinstance(source, (bytes, bytearray, memoryview)) else source.read(MAX_BYTES + 1)
        source_name = name or "system.json"
        root = None
        path = None
    if isinstance(data, str):
        data = data.encode()
    if len(data) > MAX_BYTES:
        raise ValueError("Sienna document exceeds the 64 MiB limit")
    document = json.loads(data)
    filename = document.get("time_series_storage_file")
    associations = document.get("time_series_associations", [])
    if associations and not filename:
        raise ValueError("Sienna time-series associations require a companion")
    extension = None
    if filename:
        filename = _name(filename)
        if path is not None:
            assert root is not None
            companion = path.parent / filename
            if not companion.resolve().is_relative_to(root):
                raise ValueError("Sienna companion is outside acquisition_root")
            if companion.stat().st_size + len(data) > MAX_BYTES:
                raise ValueError("Sienna bundle exceeds the 64 MiB limit")
            companion_data = companion.read_bytes()
        else:
            if named_buffers is None or filename not in named_buffers:
                raise ValueError(f"Missing Sienna companion {filename}")
            companion_data = bytes(named_buffers[filename])
        if len(companion_data) + len(data) > MAX_BYTES:
            raise ValueError("Sienna bundle exceeds the 64 MiB limit")
        # The text json.dumps({"filename": ..., "arrays": records}) would
        # produce, without serializing the records a second time.
        extension = '{"filename": ' + json.dumps(filename) + ', "arrays": ' + _read_hdf(companion_data) + "}"
    module = pio.parse(data, format=FORMATS[format], name=source_name)
    if extension is not None:
        module = pio.PioModule(module._inner._with_extension_json(KEY, extension))
    return module


def _temporal_document(document, original):
    """Migrate the common association records without changing their time meaning."""
    if original and original.get("schema_version", "0.1.0") != document.get("schema_version", "0.1.0"):
        associations = original.get("time_series_associations", [])
        owners = {(kind, row["id"]) for kind, rows in document["components"].items() for row in rows}
        for row in associations:
            if row.get("owner_category") != "Component" or (row["owner_type"], row["owner_id"]) not in owners:
                raise ValueError("Cross-version Sienna time series require an owner retained in the target electrical document")
        document["time_series_associations"] = copy.deepcopy(associations)
        document["time_series_storage_file"] = original.get("time_series_storage_file")
    old_owners = {(kind, row["id"]): row for kind, rows in (original or {}).get("components", {}).items() for row in rows}
    new_owners = {(kind, row["id"]): row for kind, rows in document["components"].items() for row in rows}
    modern = document.get("schema_version", "0.1.0") == "0.4.0"
    for row in document.get("time_series_associations", []):
        if row.get("unit_system") == "COMPONENT_BASE":
            key = (row["owner_type"], row["owner_id"])
            old, new = old_owners.get(key), new_owners.get(key)
            if old is None or new is None:
                raise ValueError("Per-unit Sienna time series require a retained component owner")
            if old.get("base_power") != new.get("base_power"):
                if key[0] in {"PowerLoad", "InterruptiblePowerLoad", "StandardLoad", "ExponentialLoad"} and new.get("power_units") == "NATURAL_UNITS":
                    # Balanced loads have no device base. Their native power fields
                    # are in MW, so the declared series base remains independent.
                    new["base_power"] = old["base_power"]
                else:
                    raise ValueError("Changed Sienna component base requires rebasing its companion arrays before emission")
        timestamp = row.get("initial_timestamp")
        if row.get("time_reference") != "zoneless" or not isinstance(timestamp, str):
            continue
        # InfraStore 0.14 spells a zoneless clock with Z in the older schemas.
        # 0.4 requires the local spelling. The explicit declaration remains.
        if timestamp.endswith("Z"):
            local = timestamp[:-1]
        elif timestamp.endswith("+00:00"):
            local = timestamp[:-6]
        elif "+" in timestamp[10:] or "-" in timestamp[10:]:
            raise ValueError("A zoneless Sienna timestamp cannot declare a nonzero UTC offset")
        else:
            local = timestamp
        row["initial_timestamp"] = local if modern else local + "Z"


def emit(module, format: str, destination=None):
    import powerio as pio
    result = pio.emit(module, FORMATS[format])
    if result.text is None:
        raise ValueError("Expected Sienna document text")
    document = json.loads(result.text)
    _temporal_document(document, module.extension("org.eigenergy.powerio.sienna"))
    files = {}
    if document.get("time_series_storage_file"):
        # Decode the companion arrays only when a companion is written.
        extension = module.extension(KEY)
        if extension is None:
            raise ValueError("Sienna companion arrays are missing from this module")
        filename = _name(document["time_series_storage_file"])
        if filename == "system.json":
            raise ValueError("Companion name collides with system.json")
        files[filename] = _write_hdf(extension["arrays"])
    elif document.get("time_series_associations"):
        raise ValueError("Sienna associations have no companion filename")
    files["system.json"] = json.dumps(document, indent=2).encode() + b"\n"
    if sum(map(len, files.values())) > MAX_BYTES:
        raise ValueError("Sienna output bundle exceeds the 64 MiB limit")
    artifacts = tuple(pio.Artifact(name=k, data=v, path=None) for k, v in files.items())
    if destination is not None:
        if not isinstance(destination, (str, os.PathLike)):
            raise TypeError("Sienna bundles require a directory destination")
        target = Path(destination)
        # Reserve the destination before creating files; never overwrite it.
        target.mkdir()
        written = []
        try:
            for filename, data in files.items():
                output = target / filename
                with output.open("xb") as stream:
                    stream.write(data)
                written.append(output)
        except BaseException:
            for output in written:
                output.unlink(missing_ok=True)
            target.rmdir()
            raise
        artifacts = tuple(pio.Artifact(name=k, data=None, path=os.fspath(target / k)) for k in files)
    return pio.EmitResult(artifacts=artifacts, layout="directory", fidelity="canonical", diagnostics=result.diagnostics)
