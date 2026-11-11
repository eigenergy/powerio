"""Optional transport tests against PyPSA's own readers and writers."""
import io
from pathlib import Path

import pytest

pytest.importorskip("pypsa")
pytest.importorskip("tables")
import powerio as pio

CASE = Path(__file__).resolve().parents[2] / "tests/data/case14.m"


def test_native_time_varying_branch_limits(tmp_path):
    import pandas as pd
    import pypsa
    network = pypsa.Network()
    network.set_snapshots(pd.date_range("2026-01-01", periods=3, freq="h"))
    network.add("Bus", "a", v_nom=110.0)
    network.add("Bus", "b", v_nom=110.0)
    network.add("Line", "varied", bus0="a", bus1="b", x=0.1, s_nom=100.0,
                s_max_pu=[0.0, float("inf"), 0.75])
    network.add("Line", "fixed", bus0="a", bus1="b", x=0.2, s_nom=20.0, s_max_pu=0.5)
    network.add("Transformer", "varied", bus0="a", bus1="b", x=0.1, s_nom=50.0,
                s_max_pu=[float("inf"), 0.0, 0.5])
    source = tmp_path / "limits"
    network.export_to_csv_folder(source)
    module = pio.parse(source)
    assert module.type_name == "powerio.TimeSeries<powerio.BalancedNetwork>"
    native_limits = [
        network.get_switchable_as_dense(kind, "s_max_pu").mul(network.components[kind].static.s_nom)
        for kind in ["Line", "Transformer"]
    ]
    restored = pio.deserialize(pio.serialize(module).artifacts[0].data)
    for index in range(3):
        expected = [*native_limits[0].iloc[index], *native_limits[1].iloc[index]]
        assert [b["rate_a"] for b in module.value[index].branches] == expected
        assert [b["rate_a"] for b in restored.value[index].branches] == expected
        assert all(b["in_service"] for b in restored.value[index].branches)


@pytest.mark.parametrize("format", ["pypsa-netcdf", "pypsa-hdf5"])
def test_time_varying_branch_limits_write_to_binary_files(format, tmp_path):
    # The CSV writer states each point's rating as its s_nom, which PyPSA
    # cannot vary across snapshots, so writing a varying rating failed.
    import pandas as pd
    import pypsa
    network = pypsa.Network()
    network.set_snapshots(pd.date_range("2026-01-01", periods=3, freq="h"))
    network.add("Bus", "a", v_nom=110.0)
    network.add("Bus", "b", v_nom=110.0)
    network.add("Line", "varied", bus0="a", bus1="b", x=0.1, s_nom=100.0,
                s_max_pu=[0.0, float("inf"), 0.75])
    network.add("Transformer", "varied", bus0="a", bus1="b", x=0.1, s_nom=50.0,
                s_max_pu=[float("inf"), 0.0, 0.5])
    source = tmp_path / "limits"
    network.export_to_csv_folder(source)
    module = pio.parse(source)
    emitted = pio.emit(module.sever_source(), format)
    recovered = pio.parse(emitted.artifacts[0].data, format=format)
    for index in range(3):
        expected = [b["rate_a"] for b in module.value[index].branches]
        actual = [b["rate_a"] for b in recovered.value[index].branches]
        assert actual == pytest.approx(expected)
        for before, after in zip(module.value[index].branches, recovered.value[index].branches, strict=True):
            assert after["x"] == pytest.approx(before["x"])


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
    import pickle

    import h5py
    import numpy as np
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


@pytest.mark.parametrize("key", ["meta", "crs"])
def test_hdf_metadata_group_without_values_is_refused(key, tmp_path):
    # The attribute scan skips the metadata subtrees because the sanitizer
    # deletes them; one without `values` was kept, and PyTables unpickled its
    # attributes once pandas opened the copy.
    import pickle

    import h5py
    import numpy as np
    from powerio._pypsa_files import _sanitized_hdf
    class Payload:
        def __reduce__(self):
            return (eval, ("__import__('pathlib').Path(%r).touch()" % str(tmp_path / "executed"),))
    path = tmp_path / "bad.h5"
    with h5py.File(path, "w") as store:
        store.create_group(key).attrs["pandas_type"] = np.bytes_(pickle.dumps(Payload(), protocol=0))
    with pytest.raises(ValueError, match=f"Unexpected PyPSA {key} metadata layout"):
        _sanitized_hdf(path, tmp_path / "safe.h5")
    assert not (tmp_path / "executed").exists()


@pytest.mark.parametrize("shape", ["cut_short", "non_ascii_prefix"])
def test_hdf_attribute_pickle_the_scanner_cannot_parse_is_refused(shape, tmp_path):
    # PyTables unpickles a bytes attribute ending in ".", running each
    # instruction until one fails, and retries with latin-1 when the default
    # ASCII decoding fails. A scan that skipped an attribute it could not parse
    # let through a call placed before a malformed byte, or after a string
    # that does not decode as ASCII.
    import pickle

    import h5py
    import numpy as np
    from powerio._pypsa_files import _sanitized_hdf
    marker = tmp_path / "executed"
    class Payload:
        def __reduce__(self):
            return (eval, ("__import__('pathlib').Path(%r).touch()" % str(marker),))
    call = pickle.dumps(Payload(), protocol=0)
    payload = call[:-1] + b"\xff." if shape == "cut_short" else b"S'\xe9'\n0" + call
    path = tmp_path / "bad.h5"
    with h5py.File(path, "w") as store:
        store.create_group("generators").attrs["pandas_type"] = np.bytes_(payload)
    with pytest.raises(ValueError, match="Executable HDF5 attribute"):
        _sanitized_hdf(path, tmp_path / "safe.h5")
    assert not marker.exists()


def test_hdf_variable_length_string_attribute_pickle_is_refused(tmp_path):
    # h5py returns a variable-length ASCII string attribute as `str`, which the
    # scan skipped, while PyTables reads the same attribute as bytes and
    # unpickles it once pandas opens the copy.
    import pickle

    import h5py
    from powerio._pypsa_files import _sanitized_hdf
    marker = tmp_path / "executed"
    class Payload:
        def __reduce__(self):
            return (eval, ("__import__('pathlib').Path(%r).touch()" % str(marker),))
    payload = pickle.dumps(Payload(), protocol=0).decode("ascii")
    path = tmp_path / "bad.h5"
    with h5py.File(path, "w") as store:
        store.create_group("generators").attrs.create(
            "pandas_type", payload, dtype=h5py.string_dtype("ascii"))
    with pytest.raises(ValueError, match="Executable HDF5 attribute"):
        _sanitized_hdf(path, tmp_path / "safe.h5")
    assert not marker.exists()


def test_hdf_attribute_pickle_after_a_nul_is_refused(tmp_path):
    # PyTables writes null terminated string attributes and reads every stored
    # byte. h5py's attribute reader stopped such a string at its first NUL, so
    # the scan saw a truncated pickle while PyTables unpickled the call after
    # the NUL once pandas opened the copy.
    import pickle

    import h5py
    import numpy as np
    from powerio._pypsa_files import _sanitized_hdf
    marker = tmp_path / "executed"
    class Payload:
        def __reduce__(self):
            return (eval, ("__import__('pathlib').Path(%r).touch()" % str(marker),))
    # BININT1 0 places a NUL third and POP discards it.
    payload = b"\x80\x02K\x000" + pickle.dumps(Payload(), protocol=0)
    path = tmp_path / "bad.h5"
    with h5py.File(path, "w") as store:
        group = store.create_group("network")
        stored = h5py.h5t.C_S1.copy()
        stored.set_size(len(payload))
        stored.set_strpad(h5py.h5t.STR_NULLTERM)
        attribute = h5py.h5a.create(group.id, b"pandas_type", stored, h5py.h5s.create(h5py.h5s.SCALAR))
        attribute.write(np.array(payload, dtype=f"S{len(payload)}"), mtype=stored)
    with pytest.raises(ValueError, match="Executable HDF5 attribute"):
        _sanitized_hdf(path, tmp_path / "safe.h5")
    assert not marker.exists()


def test_hdf_legacy_filters_attribute_is_refused(tmp_path):
    # For a file declaring a PyTables format below 2.0, PyTables rewrites a
    # FILTERS pickle before unpickling it. Lengthening `tables.Leaf` turned a
    # stop instruction the scan honored into an argument, so the call after
    # it ran.
    import pickle

    import h5py
    import numpy as np
    from powerio._pypsa_files import _sanitized_hdf
    marker = tmp_path / "executed"
    class Payload:
        def __reduce__(self):
            return (eval, ("__import__('pathlib').Path(%r).touch()" % str(marker),))
    leaf = b"(ctables.Leaf\n"
    payload = b"U" + bytes([len(leaf)]) + leaf + b".." + pickle.dumps(Payload(), protocol=0)
    path = tmp_path / "bad.h5"
    with h5py.File(path, "w") as store:
        store.attrs["PYTABLES_FORMAT_VERSION"] = np.bytes_(b"1.6")
        store.create_group("network").attrs["FILTERS"] = np.bytes_(payload)
    with pytest.raises(ValueError, match="PyTables 1.x FILTERS"):
        _sanitized_hdf(path, tmp_path / "safe.h5")
    assert not marker.exists()


def test_hdf_text_attribute_ending_in_a_period_is_kept(tmp_path):
    import h5py
    import numpy as np
    from powerio._pypsa_files import _sanitized_hdf
    path = tmp_path / "text.h5"
    with h5py.File(path, "w") as store:
        store.create_group("generators").attrs["TITLE"] = np.bytes_(b"Exported by PyPSA.")
    assert _sanitized_hdf(path, tmp_path / "safe.h5") == tmp_path / "safe.h5"


@pytest.mark.parametrize("format,suffix", [("pypsa-netcdf", ".nc"), ("pypsa-hdf5", ".h5")])
def test_temporal_binary_roundtrip(format, suffix, tmp_path):
    import pandas as pd
    import pypsa
    network = pypsa.Network()
    network.set_snapshots(pd.date_range("2026-01-01", periods=3, freq="h"))
    network.add("Bus", "a", v_nom=110.0)
    network.add("Generator", "g", bus="a", p_nom=100.0, p_set=[10.0, 20.0, 30.0])
    network.add("Load", "d", bus="a", p_set=[10.0, 20.0, 30.0], q_set=[1.0, 2.0, 3.0])
    source = tmp_path / ("sequence" + suffix)
    if format == "pypsa-netcdf":
        network.export_to_netcdf(source)
    else:
        network.export_to_hdf5(source)
    module = pio.parse(source)
    assert "TimeSeries" in module.type_name
    assert len(module.value) == 3
    emitted = pio.emit(module.sever_source(), format)
    recovered = pio.parse(emitted.artifacts[0].data, format=format)
    assert len(recovered.value) == 3
    for index, expected in enumerate([10.0, 20.0, 30.0]):
        point = recovered.value[index]
        network_value = point.network if hasattr(point, "network") else point
        assert network_value.loads[0]["p"] == pytest.approx(expected)
