"""Optional PyPSA NetCDF/HDF5 transport for the shared CSV electrical reader.

PyPSA owns its binary container layouts. The PowerIO reader still determines
which electrical and temporal fields have a typed meaning and reports losses.
Imports are lazy so the base PowerIO package has no Python dependencies.
"""
from __future__ import annotations

import io
import json
import pickletools
import shutil
import os
from pathlib import Path
import tempfile
from typing import Any

FORMATS = {"pypsa-netcdf": ".nc", "pypsa-hdf5": ".h5"}
MAX_BYTES = 64 << 20
MAX_FILES = 4096


def detect(source: Any, format: str | None) -> str | None:
    if format in FORMATS:
        return format
    if format is not None:
        return None
    if isinstance(source, (str, os.PathLike)):
        suffix = Path(source).suffix.lower()
        if suffix == ".nc":
            return "pypsa-netcdf"
        if suffix in {".h5", ".hdf5"}:
            return "pypsa-hdf5"
    return None


def _dependencies():
    try:
        import pypsa
    except ImportError as exc:
        raise ImportError("Install powerio[pypsa-files] for PyPSA NetCDF/HDF5 exchange") from exc
    return pypsa


def _check_size(path: Path) -> None:
    if path.stat().st_size > MAX_BYTES:
        raise ValueError("PyPSA file exceeds the 64 MiB acquisition limit")
    # Both modern NetCDF and PyPSA HDF5 use HDF5 storage. Bound expanded arrays
    # before asking the external reader to materialize them. Classic NetCDF is
    # already bounded by its uncompressed file size.
    import h5py
    if not h5py.is_hdf5(path):
        return
    total = 0
    with h5py.File(path, "r") as store:
        def visit(_name, obj):
            nonlocal total
            if isinstance(obj, h5py.Dataset):
                total += obj.size * max(obj.dtype.itemsize, 8)
                if total > MAX_BYTES:
                    raise ValueError("PyPSA expanded arrays exceed the 64 MiB acquisition limit")
                if (obj.dtype.kind == "O" and h5py.check_string_dtype(obj.dtype) is None
                        and _name not in {"meta/values", "crs/values"}):
                    raise ValueError("Object/pickled HDF5 arrays are unsupported; export tabular PyPSA HDF5")
        store.visititems(visit)



def _sanitized_hdf(path: Path, destination: Path) -> Path:
    """Read JSON metadata without executing the object pickle PyPSA stores.

    Tables remain in their native HDF5 layout. Pandas only sees a private copy
    with JSON metadata rewritten as tables and attributes checked for executable
    pickle instructions.
    """
    import h5py
    import pandas as pd
    unsafe = {"GLOBAL", "STACK_GLOBAL", "REDUCE", "BUILD", "INST", "OBJ",
              "NEWOBJ", "NEWOBJ_EX", "EXT1", "EXT2", "EXT4", "PERSID", "BINPERSID"}
    metadata = {}
    with h5py.File(path, "r") as store:
        def attributes(_name, obj):
            if _name.split("/", 1)[0] in {"meta", "crs"}:
                return
            for value in obj.attrs.values():
                if not isinstance(value, bytes):
                    continue
                if len(value) > MAX_BYTES:
                    raise ValueError("HDF5 metadata exceeds the acquisition limit")
                try:
                    operations = list(pickletools.genops(value))
                except (ValueError, UnicodeError):
                    continue
                if any(op.name in unsafe for op, _, _ in operations):
                    raise ValueError("Executable HDF5 attribute serialization is unsupported")
        attributes("", store)
        store.visititems(attributes)
        for key in ("meta", "crs"):
            dataset = store.get(f"{key}/values")
            if dataset is None:
                continue
            if dataset.size != 1:
                raise ValueError(f"Unexpected PyPSA {key} metadata layout")
            raw = bytes(dataset[0])
            if len(raw) > MAX_BYTES:
                raise ValueError("HDF5 metadata exceeds the acquisition limit")
            # Extract a JSON string literal only. No pickle operation is executed,
            # including its numpy reconstruction instructions.
            candidates = []
            for op, argument, _ in pickletools.genops(raw):
                if op.name in {"UNICODE", "BINUNICODE", "SHORT_BINUNICODE", "BINUNICODE8"}:
                    try:
                        value = json.loads(argument)
                    except (ValueError, TypeError):
                        continue
                    if isinstance(value, dict):
                        candidates.append(value)
            if len(candidates) != 1:
                raise ValueError(f"PyPSA {key} metadata must contain exactly one JSON object")
            metadata[key] = candidates[0]
    shutil.copyfile(path, destination)
    with h5py.File(destination, "r+") as store:
        for key in metadata:
            del store[key]
    with pd.HDFStore(destination, "a") as store:
        for key, value in metadata.items():
            store.put(key, pd.Series([json.dumps(value)]), format="table")
    return destination

def parse(source: Any, format: str, *, name: str | None = None):
    import powerio as pio
    pypsa = _dependencies()
    with tempfile.TemporaryDirectory(prefix="powerio-pypsa-") as temporary:
        directory = Path(temporary)
        if isinstance(source, (str, os.PathLike)):
            path = Path(source)
            source_name = os.fspath(source)
        else:
            data = bytes(source) if isinstance(source, (bytes, bytearray, memoryview)) else source.read(MAX_BYTES + 1)
            if not isinstance(data, bytes):
                raise TypeError("PyPSA binary input must supply bytes")
            if len(data) > MAX_BYTES:
                raise ValueError("PyPSA file exceeds the 64 MiB acquisition limit")
            source_name = name or ("network" + FORMATS[format])
            path = directory / ("input" + FORMATS[format])
            path.write_bytes(data)
        _check_size(path)
        network = pypsa.Network()
        if format == "pypsa-netcdf":
            network.import_from_netcdf(path)
        else:
            network.import_from_hdf5(_sanitized_hdf(path, directory / "safe.h5"))
        folder = directory / "csv"
        network.export_to_csv_folder(folder)
        files = {}
        total = 0
        for entry in sorted(folder.iterdir()):
            total += entry.stat().st_size
            if total > MAX_BYTES or len(files) >= MAX_FILES:
                raise ValueError("Converted PyPSA CSV exceeds the bounded collection limits")
            files[entry.name] = entry.read_bytes()
        with pio.Collection.from_files(files, name=source_name, max_bytes=MAX_BYTES, max_files=MAX_FILES) as collection:
            candidates = [c for c in collection.inspect()["candidates"] if c["selection"].get("format") == "pypsa-csv"]
            if len(candidates) != 1:
                raise ValueError("PyPSA export did not produce exactly one CSV electrical case")
            result = collection.parse(candidates[0]["selection"])
            if result.incomplete:
                raise ValueError("PyPSA CSV acquisition is incomplete")
            return result.module


def emit(module, format: str, destination: Any = None):
    import powerio as pio
    pypsa = _dependencies()
    csv = pio.emit(module, "pypsa-csv")
    with tempfile.TemporaryDirectory(prefix="powerio-pypsa-") as temporary:
        directory = Path(temporary)
        folder = directory / "csv"
        folder.mkdir()
        common = Path(csv.artifacts[0].name).parent
        for artifact in csv.artifacts:
            if artifact.data is None:
                raise ValueError("Expected in-memory CSV artifacts")
            relative = Path(artifact.name).relative_to(common)
            path = folder / relative
            if path.parent != folder or relative.is_absolute() or ".." in relative.parts:
                raise ValueError("Unexpected nested PyPSA CSV artifact")
            path.write_bytes(artifact.data)
        network = pypsa.Network()
        network.import_from_csv_folder(folder)
        output = directory / ("network" + FORMATS[format])
        if format == "pypsa-netcdf":
            network.export_to_netcdf(output)
        else:
            network.export_to_hdf5(output)
        _check_size(output)
        data = output.read_bytes()
    artifact = pio.Artifact(name="network" + FORMATS[format], data=data, path=None)
    if isinstance(destination, (str, os.PathLike)):
        target = Path(destination)
        # Commit without replacing an existing destination, as native emission
        # does. The staging file is on the same filesystem as its destination.
        with tempfile.NamedTemporaryFile(dir=target.parent, prefix=".powerio-", delete=False) as staged:
            staging = Path(staged.name)
            try:
                staged.write(data)
                staged.flush()
                os.fsync(staged.fileno())
                os.link(staging, target)
            finally:
                staging.unlink(missing_ok=True)
        artifact = pio.Artifact(name=target.name, data=None, path=os.fspath(target))
    elif destination is not None:
        if isinstance(destination, io.TextIOBase):
            raise TypeError("PyPSA NetCDF/HDF5 requires a binary destination")
        destination.write(data)
    return pio.EmitResult(artifacts=(artifact,), layout="file", fidelity="canonical", diagnostics=csv.diagnostics)
