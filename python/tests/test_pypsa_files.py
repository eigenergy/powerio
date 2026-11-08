"""Optional transport tests against PyPSA's own readers and writers."""
import io
from pathlib import Path

import pytest

pytest.importorskip("pypsa")
pytest.importorskip("tables")
import powerio as pio

CASE = Path(__file__).resolve().parents[2] / "tests/data/case14.m"


@pytest.mark.parametrize("format,suffix", [("pypsa-netcdf", ".nc"), ("pypsa-hdf5", ".h5")])
def test_binary_roundtrip_and_destination(format, suffix, tmp_path):
    source = pio.parse(CASE)
    output = pio.emit(source, format)
    recovered = pio.parse(io.BytesIO(output.artifacts[0].data), format=format)
    assert len(recovered.value.buses) == 14
    assert len(recovered.value.branches) == 20
    assert len(recovered.value.generators) == 5
    path = tmp_path / ("case" + suffix)
    pio.emit(source, format, path)
    assert len(pio.parse(path).value.buses) == 14
    original = path.read_bytes()
    with pytest.raises(FileExistsError):
        pio.emit(source, format, path)
    assert path.read_bytes() == original


def test_hdf_metadata_is_not_executed(tmp_path):
    import h5py
    import numpy as np
    import pickle
    from powerio._pypsa_files import _sanitized_hdf
    class Payload:
        def __reduce__(self):
            return (eval, ("__import__('pathlib').Path(%r).touch()" % str(tmp_path / "executed"),))
    path = tmp_path / "bad.h5"
    with h5py.File(path, "w") as store:
        values = store.create_dataset("meta/values", (1,), dtype=h5py.vlen_dtype(np.dtype("uint8")))
        values[0] = np.frombuffer(pickle.dumps(Payload()), dtype="uint8")
    with pytest.raises(ValueError, match="exactly one JSON object"):
        _sanitized_hdf(path, tmp_path / "safe.h5")
    assert not (tmp_path / "executed").exists()
