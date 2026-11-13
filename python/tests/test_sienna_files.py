"""Sienna companion arrays survive IR and load in the pinned InfraStore consumer."""
import json
import os
from datetime import datetime, timedelta
from pathlib import Path

import pytest

pytest.importorskip("h5py")
infrastore = pytest.importorskip("infrastore")
import numpy as np  # noqa: E402

import powerio as pio  # noqa: E402


def validate_document(document):
    root = os.environ.get("POWERIO_SIENNA_BUNDLES")
    if root:
        from jsonschema import Draft7Validator
        schema = json.loads((Path(root) / document.get("schema_version", "0.1.0") / "SystemDocument.json").read_text())
        Draft7Validator(schema).validate(document)


def import_associations(store, rows):
    # InfraStore 0.14 uses the pre-0.4 timestamp spelling. The output document
    # itself is validated unchanged against its named schema above.
    legacy = json.loads(json.dumps(rows))
    for row in legacy:
        if row.get("time_reference") == "zoneless" and not row["initial_timestamp"].endswith("Z"):
            row["initial_timestamp"] += "Z"
    store.import_time_series_associations_openapi(json.dumps(legacy))


def declared_associations(store, version):
    rows = json.loads(store.export_time_series_associations_openapi())
    if version == 4:
        for row in rows:
            if row.get("time_reference") == "zoneless":
                row["initial_timestamp"] = row["initial_timestamp"].removesuffix("Z")
    return rows


@pytest.mark.parametrize("version", [1, 2, 3, 4])
def test_bundle_arrays_and_associations_survive_ir(tmp_path, version):
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    document = json.loads(pio.emit(pio.parse(case), f"sienna-0-{version}-json").text)
    owner = document["components"]["PowerLoad"][0]["id"]
    path = tmp_path / "series.h5"
    with infrastore.Store.create(str(path), catalog="memory") as store:
        store.add_time_series(owner, "PowerLoad", infrastore.OwnerCategory.Component,
            infrastore.SingleTimeSeries(datetime(2030, 1, 1), timedelta(hours=1), np.array([15., 19.]), "active_power", units="MW"))
        document["time_series_associations"] = declared_associations(store, version)
    document["time_series_storage_file"] = "series.h5"
    source = json.dumps(document).encode()
    module = pio.parse(source, format=f"sienna-0-{version}-bundle", named_buffers={"series.h5": path.read_bytes()})
    module = pio.deserialize(pio.serialize(module).artifacts[0].data)
    output = tmp_path / "out"
    pio.emit(module, f"sienna-0-{version}-bundle", output)
    actual = json.loads((output / "system.json").read_text())
    assert actual["time_series_associations"] == document["time_series_associations"]
    validate_document(actual)
    with infrastore.Store.open_without_catalog(str(output / "series.h5"), catalog="memory") as store:
        import_associations(store, actual["time_series_associations"])
        row = actual["time_series_associations"][0]
        np.testing.assert_array_equal(store.get_array_by_hash(row["data_hash"]), [15., 19.])
    with pytest.raises(Exception, match="exist"):
        pio.emit(module, f"sienna-0-{version}-bundle", output)


def test_missing_and_external_companions_are_rejected(tmp_path):
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    doc = json.loads(pio.emit(pio.parse(case), "sienna-json").text)
    doc["time_series_storage_file"] = "series.h5"
    with pytest.raises(Exception, match="Missing Sienna companion"):
        pio.parse(json.dumps(doc).encode(), format="sienna-bundle")
    doc["time_series_storage_file"] = "../series.h5"
    with pytest.raises(Exception, match="relative filename"):
        pio.parse(json.dumps(doc).encode(), format="sienna-bundle")
    import h5py
    with h5py.File(tmp_path / "external.h5", "w") as store:
        store["unsafe"] = h5py.ExternalLink("/etc/passwd", "data")
    doc["time_series_storage_file"] = "external.h5"
    with pytest.raises(Exception, match="external and soft links"):
        pio.parse(json.dumps(doc).encode(), format="sienna-bundle", named_buffers={"external.h5": (tmp_path / "external.h5").read_bytes()})


@pytest.mark.parametrize("version", [1, 2, 3, 4])
@pytest.mark.parametrize("basis", ["natural_units", "component_base"])
def test_bundle_stages_time_series_into_transitionzero(tmp_path, version, basis):
    pytest.importorskip("interop")
    from interop.adapters.outbound.local_filesystem import LocalFilesystem
    from interop.core.reporting import EventLog, ScopedRecorder
    from powerio.interop import PowerIOSource, PowerIOSourceParams
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    doc = json.loads(pio.emit(pio.parse(case), f"sienna-0-{version}-json").text)
    owner_row = doc["components"]["PowerLoad"][0]
    owner = owner_row["id"]
    owner_row["base_power"] = 50.0
    capacity = owner_row["max_active_power"]
    values = np.array([15., 19.]) / (50.0 if basis == "component_base" else 1.0)
    path = tmp_path / "series.h5"
    with infrastore.Store.create(str(path), catalog="memory") as store:
        store.add_time_series(owner,"PowerLoad",infrastore.OwnerCategory.Component,
            infrastore.SingleTimeSeries(datetime(2030,1,1),timedelta(hours=1),values,"active_power",units="MW",unit_system=basis))
        doc["time_series_associations"] = declared_associations(store, version)
    doc["time_series_storage_file"] = path.name
    validate_document(doc)
    document_path = tmp_path / "system.json"
    document_path.write_text(json.dumps(doc))
    source = PowerIOSource(LocalFilesystem(), ScopedRecorder(EventLog()))
    state = source.load_into_state(PowerIOSourceParams(input_path=document_path,format=f"sienna-0-{version}-bundle",tables="sienna"),tmp_path / "staged")
    assert len(state.source_time_series) == 1
    frame = next(iter(state.source_time_series.values())).collect()
    assert frame.height == 2
    np.testing.assert_allclose(frame["value"].to_numpy(), np.array([15., 19.]) / capacity)
    assert len(state.source_topology) >= 3


@pytest.mark.parametrize("version", [1, 2, 3, 4])
@pytest.mark.parametrize("kind", ["Deterministic", "Probabilistic", "Scenarios"])
def test_forecast_axes_and_values_survive_ir(tmp_path, kind, version):
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    doc = json.loads(pio.emit(pio.parse(case), f"sienna-0-{version}-json").text)
    owner = doc["components"]["PowerLoad"][0]["id"]
    values = np.arange(6. if kind == "Deterministic" else 12.).reshape((3,2) if kind == "Deterministic" else (2,3,2))
    kwargs = {"percentiles":[0.1,0.9]} if kind == "Probabilistic" else {}
    series = getattr(infrastore,kind)(datetime(2030,1,1),timedelta(hours=1),timedelta(hours=3),timedelta(hours=2),2,data=values,name="active_power",units="MW",**kwargs)
    path = tmp_path / "forecast.h5"
    with infrastore.Store.create(str(path),catalog="memory") as store:
        store.add_time_series(owner,"PowerLoad",infrastore.OwnerCategory.Component,series)
        doc["time_series_associations"] = declared_associations(store, version)
    doc["time_series_storage_file"] = path.name
    module = pio.parse(json.dumps(doc).encode(),format="sienna-bundle",named_buffers={path.name:path.read_bytes()})
    module = pio.deserialize(pio.serialize(module).artifacts[0].data)
    for target in [1, 2, 3, 4]:
        output = tmp_path / f"out-{target}"
        pio.emit(module,f"sienna-0-{target}-bundle",output)
        actual = json.loads((output / "system.json").read_bytes())
        validate_document(actual)
        row = actual["time_series_associations"][0]
        assert row["initial_timestamp"] == ("2030-01-01T00:00:00" if target == 4 else "2030-01-01T00:00:00Z")
        assert row["time_reference"] == "zoneless"
        assert row["array_shape"] == list(values.shape)
        with infrastore.Store.open_without_catalog(str(output / path.name),catalog="memory") as store:
            import_associations(store, actual["time_series_associations"])
            np.testing.assert_array_equal(store.get_array_by_hash(row["data_hash"]),values)
