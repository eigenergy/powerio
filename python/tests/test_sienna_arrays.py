"""Sienna companion strings keep their stored bytes through PowerIO IR."""
import json
from pathlib import Path

import pytest

h5py = pytest.importorskip("h5py")
import numpy as np  # noqa: E402

import powerio as pio  # noqa: E402


@pytest.mark.parametrize("raw", ["café".encode(), b"\xff\xfe"], ids=["utf8", "binary"])
def test_fixed_width_companion_strings_keep_their_bytes(tmp_path, raw):
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    document = json.loads(pio.emit(pio.parse(case), "sienna-0-4-json").text)
    document["time_series_storage_file"] = "series.h5"
    names = np.array([raw, b"plain"], dtype="S8")
    path = tmp_path / "series.h5"
    with h5py.File(path, "w") as store:
        store["names"] = names
        store["names"].attrs["label"] = np.bytes_(raw)
        store["names"].attrs["count"] = np.int32(2)
    module = pio.parse(
        json.dumps(document).encode(),
        format="sienna-0-4-bundle",
        named_buffers={"series.h5": path.read_bytes()},
    )
    module = pio.deserialize(pio.serialize(module).artifacts[0].data)
    output = tmp_path / "out"
    pio.emit(module, "sienna-0-4-bundle", output)
    with h5py.File(output / "series.h5", "r") as store:
        assert store["names"].dtype == names.dtype
        assert store["names"][()].tolist() == names.tolist()
        assert store["names"].attrs["label"] == np.bytes_(raw)
        assert store["names"].attrs["count"].dtype == np.int32
