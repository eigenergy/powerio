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
                              "pt": series(-9.8, 3.1), "qt": series(2.2, -0.8),
                              "rating_long_term": series(None, 150),
                              "rating_short_term": series(160, None),
                              "rating_emergency": series(None, 170)}},
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
    assert actual["elements"]["branch"]["1"]["rating_long_term"]["values"] == [None, 150]
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


def test_canonical_ratings_build_the_intended_native_egret_thermal_constraints():
    ModelData = pytest.importorskip("egret.data.model_data").ModelData
    branch_library = pytest.importorskip("egret.model_library.transmission.branch")
    pe = pytest.importorskip("pyomo.environ")
    _, output = canonical()
    native = ModelData(json.loads(output.text))

    def thermal_model(branch, rating):
        model = pe.ConcreteModel()
        for flow in ("pf", "qf", "pt", "qt"):
            setattr(model, flow, pe.Var(["1"], initialize=branch[flow]))
        branch_library.declare_ineq_s_branch_thermal_limit(
            model, ["1"], {"1": branch}, {"1": rating}
        )
        branch_library.declare_ineq_p_branch_thermal_lbub(
            model, ["1"], {"1": branch}, {"1": rating}
        )
        return model

    for index in range(2):
        branch = native.clone_at_time_index(index).data["elements"]["branch"]["1"]
        for field, expected in (("rating_long_term", (None, 150)),
                                ("rating_short_term", (160, None)),
                                ("rating_emergency", (None, 170))):
            rating = branch[field]
            assert rating == expected[index]
            model = thermal_model(branch, rating)
            for constraints in (model.ineq_sf_branch_thermal_limit,
                                model.ineq_st_branch_thermal_limit):
                if rating is None:
                    assert len(constraints) == 0
                else:
                    constraint = constraints["1"]
                    assert pe.value(constraint.upper) == rating ** 2
                    assert pe.value(constraint.body) <= pe.value(constraint.upper)
            if rating is None:
                assert len(model.ineq_pf_branch_thermal_lb) == 0
                assert len(model.ineq_pf_branch_thermal_ub) == 0
            else:
                assert pe.value(model.ineq_pf_branch_thermal_lb["1"].lower) == -rating
                assert pe.value(model.ineq_pf_branch_thermal_ub["1"].upper) == rating

        # Negative control: the old numeric zero output creates constraints
        # at both terminals and rejects the fixture's nonzero branch flows.
        zero_model = thermal_model(branch, 0.0)
        for constraints in (zero_model.ineq_sf_branch_thermal_limit,
                            zero_model.ineq_st_branch_thermal_limit):
            assert pe.value(constraints["1"].upper) == 0.0
            assert pe.value(constraints["1"].body) > 0.0
        assert pe.value(zero_model.ineq_pf_branch_thermal_lb["1"].lower) == 0.0
        assert pe.value(zero_model.ineq_pf_branch_thermal_ub["1"].upper) == 0.0
        assert pe.value(zero_model.pf["1"]) != 0.0


@pytest.mark.parametrize("ratings", [(0, 150), (150, 0)])
def test_native_egret_zero_limit_cannot_become_an_unlimited_powerio_branch(ratings):
    data = document()
    data["elements"]["branch"]["1"]["rating_long_term"]["values"] = list(ratings)
    module = powerio.parse(io.StringIO(json.dumps(data)), format="egret-json")
    restored = powerio.deserialize(powerio.serialize(module).text.encode())
    for index, rating in enumerate(ratings):
        assert restored.value[index].branches[0]["rate_a"] == rating
        assert restored.value[index].branches[0]["in_service"] is True
    output = json.loads(powerio.emit(restored, "egret-json").text)
    assert output["elements"]["branch"]["1"]["rating_long_term"]["values"] == list(ratings)
