"""Optional PyPSA NetCDF/HDF5 transport for the shared CSV electrical reader.

PyPSA owns its binary container layouts. The PowerIO reader still determines
which electrical and temporal fields have a typed meaning and reports losses.
Imports are lazy so the base PowerIO package has no Python dependencies.
"""
from __future__ import annotations

import io
import json
import os
from pathlib import Path
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
        seen = set()
        def check_links(group):
            address = h5py.h5o.get_info(group.id).addr
            if address in seen:
                raise ValueError("HDF5 aliases and cycles are unsupported")
            seen.add(address)
            if isinstance(group, h5py.Dataset):
                if group.is_virtual or group.external:
                    raise ValueError("External HDF5 data is unsupported")
                return
            for key in group:
                if not isinstance(group.get(key, getlink=True), h5py.HardLink):
                    raise ValueError("External and soft HDF5 links are unsupported")
                check_links(group[key])
        check_links(store)
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



class _GlobalRequested(Exception):
    """A pickle named a global, the only way unpickling can call code."""


class _NoGlobalsCache:
    unpickler: type | None = None


def _no_globals() -> type:
    if _NoGlobalsCache.unpickler is None:
        # pickle, like the other container helpers' imports, loads only when a
        # PyPSA binary source is read, so dispatching other formats stays cheap.
        import pickle

        class _NoGlobals(pickle.Unpickler):
            def find_class(self, module, name):
                raise _GlobalRequested(f"{module}.{name}")

        _NoGlobalsCache.unpickler = _NoGlobals
    return _NoGlobalsCache.unpickler


def _executable_pickle(value: bytes) -> bool:
    """Whether unpickling ``value`` would reach a global.

    PyTables unpickles a bytes attribute that ends in ``.``, running each
    instruction until one fails, and retries with latin-1 when ASCII decoding
    fails. Running the same unpickler with every global refused follows that
    path exactly, which a separate parse of the opcodes does not: it stopped at
    the first byte it could not read and so never saw a call placed before it.
    Latin-1 decodes every byte, so the check reaches every instruction either
    of PyTables' attempts would. Without globals, an instruction only builds
    plain data, so a failure leaves nothing executed.
    """
    try:
        _no_globals()(io.BytesIO(value), encoding="latin1").load()
    except _GlobalRequested:
        return True
    except Exception:
        return False
    return False


def _sanitized_hdf(path: Path, destination: Path) -> Path:
    """Read JSON metadata without executing the object pickle PyPSA stores.

    Tables remain in their native HDF5 layout. Pandas only sees a private copy
    with JSON metadata rewritten as tables and attributes checked for executable
    pickle instructions.
    """
    import pickletools
    import shutil

    import h5py
    import pandas as pd
    metadata = {}
    with h5py.File(path, "r") as store:
        def attributes(_name, obj):
            if _name.split("/", 1)[0] in {"meta", "crs"}:
                return
            for value in obj.attrs.values():
                # h5py returns a variable-length string attribute as `str`,
                # while PyTables reads the same attribute as bytes and may
                # unpickle it, so both are checked.
                if isinstance(value, str):
                    raw = value.encode("utf-8", "surrogateescape")
                elif isinstance(value, bytes):
                    raw = value
                else:
                    continue
                if len(raw) > MAX_BYTES:
                    raise ValueError("HDF5 metadata exceeds the acquisition limit")
                if _executable_pickle(raw):
                    raise ValueError("Executable HDF5 attribute serialization is unsupported")
        attributes("", store)
        store.visititems(attributes)
        for key in ("meta", "crs"):
            node = store.get(key)
            if node is None:
                continue
            # The attribute scan skips this subtree because it is deleted below.
            # Without a `values` dataset nothing would be deleted, and its
            # attributes would reach pandas unscanned.
            dataset = node.get("values") if isinstance(node, h5py.Group) else None
            if not isinstance(dataset, h5py.Dataset) or dataset.size != 1:
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
                        value = json.loads(str(argument))
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
    import tempfile

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
        files: dict[str, bytes] = {}
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
    import tempfile

    import powerio as pio
    _dependencies()
    with tempfile.TemporaryDirectory(prefix="powerio-pypsa-") as temporary:
        directory = Path(temporary)
        network, diagnostics = _network(module, directory)
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
    return pio.EmitResult(artifacts=(artifact,), layout="file", fidelity="canonical", diagnostics=diagnostics)



def _static_network(module, directory: Path):
    import shutil

    import powerio as pio
    csv = pio.emit(module, "pypsa-csv")
    if directory.exists():
        shutil.rmtree(directory)
    directory.mkdir(parents=True)
    common = Path(csv.artifacts[0].name).parent
    for artifact in csv.artifacts:
        if artifact.data is None:
            raise ValueError("Expected in-memory CSV artifacts")
        relative = Path(artifact.name).relative_to(common)
        path = directory / relative
        if path.parent != directory or relative.is_absolute() or ".." in relative.parts:
            raise ValueError("Unexpected nested PyPSA CSV artifact")
        path.write_bytes(artifact.data)
    network = _dependencies().Network()
    network.import_from_csv_folder(directory)
    return network, list(csv.diagnostics)


def _network(module, directory: Path):
    import pandas as pd

    import powerio as pio
    if not isinstance(module.value, pio.TimeSeries):
        return _static_network(module, directory / "csv")
    series = module.value
    if not len(series):
        raise ValueError("A PyPSA time series needs at least one snapshot")
    if len(series) > MAX_FILES:
        raise ValueError("PyPSA export exceeds the bounded snapshot limit")
    labels = [point.label for point in series.time_points]
    if len(set(labels)) != len(labels):
        raise ValueError("PyPSA snapshots require unique labels")
    try:
        snapshots = pd.DatetimeIndex(pd.to_datetime(labels, format="ISO8601"))
    except ValueError:
        snapshots = pd.Index(labels, name="snapshot")
    network = None
    diagnostics = []
    dynamic: dict[tuple[str, str], list[Any]] = {}
    expanded_bytes = 0
    for point in series:
        current, findings = _static_network(pio.PioModule.from_value(point), directory / "csv")
        diagnostics.extend(findings)
        if network is None:
            network = current
            network.set_snapshots(snapshots)
        for name, component in network.components.items():
            first = component.static
            following = current.components[name].static
            if not first.index.equals(following.index) or not first.columns.equals(following.columns):
                raise ValueError(f"PyPSA temporal export requires a fixed component inventory: {name}")
            for column in first.columns:
                varying = column in component.defaults.index and bool(component.defaults.loc[column, "varying"])
                if varying:
                    values = following[column].to_numpy(copy=True)
                    expanded_bytes += values.nbytes
                    if expanded_bytes > MAX_BYTES:
                        raise ValueError("PyPSA temporal arrays exceed the 64 MiB acquisition limit")
                    dynamic.setdefault((name, column), []).append(values)
                elif not first[column].equals(following[column]):
                    raise ValueError(f"PyPSA cannot vary static attribute {name}.{column} across snapshots")
    assert network is not None
    for (name, column), values in dynamic.items():
        component = network.components[name]
        # Store only an explicitly changing series. Static dispatch remains a
        # scalar; a consumer still sees the complete declared snapshot axis.
        frame = pd.DataFrame(values, index=snapshots, columns=component.static.index)
        equal = frame.eq(frame.iloc[0]) | (frame.isna() & frame.iloc[0].isna())
        if not equal.all().all():
            component.dynamic[column] = frame
    for index, point in enumerate(series.time_points):
        if point.duration_seconds is not None:
            network.snapshot_weightings.iloc[index, :] = point.duration_seconds / 3600.0
    return network, diagnostics
