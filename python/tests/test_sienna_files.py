"""Sienna companion arrays survive IR and load in the pinned InfraStore consumer."""
import json
from datetime import datetime, timedelta
from pathlib import Path

import pytest

pytest.importorskip("h5py")
infrastore = pytest.importorskip("infrastore")
import numpy as np  # noqa: E402

import powerio as pio  # noqa: E402


@pytest.mark.parametrize("version", [1, 2, 3, 4])
def test_bundle_arrays_and_associations_survive_ir(tmp_path, version):
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    document = json.loads(pio.emit(pio.parse(case), f"sienna-0-{version}-json").text)
    owner = document["components"]["PowerLoad"][0]["id"]
    path = tmp_path / "series.h5"
    with infrastore.Store.create(str(path), catalog="memory") as store:
        store.add_time_series(owner, "PowerLoad", infrastore.OwnerCategory.Component,
            infrastore.SingleTimeSeries(datetime(2030, 1, 1), timedelta(hours=1), np.array([15., 19.]), "active_power", units="MW"))
        document["time_series_associations"] = json.loads(store.export_time_series_associations_openapi())
    document["time_series_storage_file"] = "series.h5"
    source = json.dumps(document).encode()
    module = pio.parse(source, format=f"sienna-0-{version}-bundle", named_buffers={"series.h5": path.read_bytes()})
    module = pio.deserialize(pio.serialize(module).artifacts[0].data)
    output = tmp_path / "out"
    pio.emit(module, f"sienna-0-{version}-bundle", output)
    actual = json.loads((output / "system.json").read_text())
    assert actual["time_series_associations"] == document["time_series_associations"]
    with infrastore.Store.open_without_catalog(str(output / "series.h5"), catalog="memory") as store:
        store.import_time_series_associations_openapi(json.dumps(actual["time_series_associations"]))
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


def test_bundle_stages_time_series_into_transitionzero(tmp_path):
    pytest.importorskip("interop")
    from interop.adapters.outbound.local_filesystem import LocalFilesystem
    from interop.core.reporting import EventLog, ScopedRecorder
    from powerio.interop import PowerIOSource, PowerIOSourceParams
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    doc = json.loads(pio.emit(pio.parse(case), "sienna-0-2-json").text)
    owner = doc["components"]["PowerLoad"][0]["id"]
    path = tmp_path / "series.h5"
    with infrastore.Store.create(str(path), catalog="memory") as store:
        store.add_time_series(owner,"PowerLoad",infrastore.OwnerCategory.Component,
            infrastore.SingleTimeSeries(datetime(2030,1,1),timedelta(hours=1),np.array([15.,19.]),"active_power",units="MW",unit_system="natural_units"))
        doc["time_series_associations"] = json.loads(store.export_time_series_associations_openapi())
    doc["time_series_storage_file"] = path.name
    document_path = tmp_path / "system.json"
    document_path.write_text(json.dumps(doc))
    source = PowerIOSource(LocalFilesystem(), ScopedRecorder(EventLog()))
    state = source.load_into_state(PowerIOSourceParams(input_path=document_path,format="sienna-0-2-bundle",tables="sienna"),tmp_path / "staged")
    assert len(state.source_time_series) == 1
    frame = next(iter(state.source_time_series.values())).collect()
    assert frame.height == 2
    assert len(state.source_topology) >= 3


@pytest.mark.parametrize("kind", ["Deterministic", "Probabilistic", "Scenarios"])
def test_forecast_axes_and_values_survive_ir(tmp_path, kind):
    case = Path(__file__).resolve().parents[2] / "tests/data/case9.m"
    doc = json.loads(pio.emit(pio.parse(case), "sienna-0-4-json").text)
    owner = doc["components"]["PowerLoad"][0]["id"]
    values = np.arange(6. if kind == "Deterministic" else 12.).reshape((3,2) if kind == "Deterministic" else (2,3,2))
    kwargs = {"percentiles":[0.1,0.9]} if kind == "Probabilistic" else {}
    series = getattr(infrastore,kind)(datetime(2030,1,1),timedelta(hours=1),timedelta(hours=3),timedelta(hours=2),2,data=values,name="active_power",units="MW",**kwargs)
    path = tmp_path / "forecast.h5"
    with infrastore.Store.create(str(path),catalog="memory") as store:
        store.add_time_series(owner,"PowerLoad",infrastore.OwnerCategory.Component,series)
        doc["time_series_associations"] = json.loads(store.export_time_series_associations_openapi())
    doc["time_series_storage_file"] = path.name
    module = pio.parse(json.dumps(doc).encode(),format="sienna-bundle",named_buffers={path.name:path.read_bytes()})
    module = pio.deserialize(pio.serialize(module).artifacts[0].data)
    output = tmp_path / "out"
    pio.emit(module,"sienna-bundle",output)
    actual = json.loads((output / "system.json").read_bytes())
    assert actual["time_series_associations"] == doc["time_series_associations"]
    with infrastore.Store.open_without_catalog(str(output / path.name),catalog="memory") as store:
        store.import_time_series_associations_openapi(json.dumps(actual["time_series_associations"]))
        np.testing.assert_array_equal(store.get_array_by_hash(actual["time_series_associations"][0]["data_hash"]),values)
