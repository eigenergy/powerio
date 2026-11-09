"""Electrical time series must remain usable after source bytes are discarded."""

import io
import json

import pytest
import powerio


def document():
    def series(*values):
        return {"data_type": "time_series", "values": list(values)}

    return {
        "system": {"baseMVA": 100, "time_keys": ["morning", "evening"], "time_period_length_minutes": 15},
        "elements": {
            "bus": {"7": {"matpower_bustype": "ref"}, "43": {}},
            "load": {"load_1": {"bus": "43", "p_load": series(10, 20), "q_load": 2}},
            "generator": {"1": {"bus": "7", "pg": 11, "p_max": 100}},
            "branch": {"1": {"from_bus": "7", "to_bus": "43", "reactance": 0.1,
                              "pf": series(10, -3), "qf": series(-2, 1),
                              "pt": series(-9.8, 3.1), "qt": series(2.2, -0.8)}},
        },
    }


def canonical():
    original = powerio.parse(io.StringIO(json.dumps(document())), format="egret-json")
    restored = powerio.deserialize(io.StringIO(powerio.serialize(original).text))
    return restored, powerio.emit(restored, "egret-json")


def test_temporal_python_ir_and_canonical_emission():
    restored, output = canonical()
    assert isinstance(restored.value, powerio.TimeSeries)
    assert [point.label for point in restored.value.time_points] == ["morning", "evening"]
    assert [point.duration_seconds for point in restored.value.time_points] == [900, 900]
    actual = json.loads(output.text)
    assert actual["elements"]["branch"]["1"]["pf"]["values"] == [10, -3]
    assert actual["system"]["time_period_length_minutes"] == 15
    reread = powerio.parse(io.StringIO(output.text), format="egret-json")
    assert len(reread.value) == 2
    assert reread.value[1].loads[0]["p"] == 20


def test_canonical_output_with_native_egret_modeldata():
    # Required in Validation CI; optional in the zero-dependency binding suite.
    ModelData = pytest.importorskip("egret.data.model_data").ModelData
    _, output = canonical()
    native = ModelData(json.loads(output.text))
    for index, flows in enumerate(((10, -2, -9.8, 2.2), (-3, 1, 3.1, -0.8))):
        snapshot = native.clone_at_time_index(index).data
        assert "time_keys" not in snapshot["system"]
        assert snapshot["system"]["time_period_length_minutes"] == 15
        assert snapshot["elements"]["load"]["load_1"]["p_load"] == (10, 20)[index]
        assert tuple(snapshot["elements"]["branch"]["1"][key] for key in ("pf", "qf", "pt", "qt")) == flows
