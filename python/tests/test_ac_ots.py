"""AC optimal transmission switching values in Python: the read-only typed
values a module holds and the default construction."""

import io
import json
from pathlib import Path

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"


def _ac_solution_data(fixed_text: str, objective: float) -> dict:
    """The AC OPF solution a binding writes for a fixed-topology problem, with
    synthetic values that are not an AC feasibility certificate."""
    document = json.loads(fixed_text)
    network = document["value"]["data"]["network"]
    buses = len(network["buses"])
    branches = len(network["branches"])
    generators = len(network["generators"])
    return {
        "instance": document["value"]["data"],
        "termination": {"kind": "not_reported"},
        "residuals": {
            "max_active_power_mismatch": None,
            "max_reactive_power_mismatch": None,
        },
        "bus_voltage_magnitude": [1.01] * buses,
        "bus_voltage_angle": [0.0] * buses,
        "bus_active_injection": [0.0] * buses,
        "bus_reactive_injection": [0.0] * buses,
        "branch_from_active_flow": [0.0] * branches,
        "branch_to_active_flow": [0.0] * branches,
        "branch_from_reactive_flow": [0.0] * branches,
        "branch_to_reactive_flow": [0.0] * branches,
        "generator_active_power": [12.0] * generators,
        "generator_reactive_power": [3.0] * generators,
        "three_winding_transformer_terminal_powers": [],
        "objective": objective,
    }


def test_default_instance_is_a_typed_value():
    module = powerio.parse(DATA / "case9.m").to_ac_ots_instance()
    assert isinstance(module.value, powerio.AcOtsInstance)
    assert module.value.network.n_branches == 9
    document = json.loads(powerio.serialize(module).text)
    assert document["value"]["type"] == "powerio.AcOtsInstance"
    assert document["value"]["data"]["options"]["switchable"]["select"] == "only"
    assert document["history"][-1]["kind"] == "transform"
    restored = powerio.deserialize(io.StringIO(powerio.serialize(module).text))
    assert isinstance(restored.value, powerio.AcOtsInstance)


def test_a_solution_document_reads_as_a_typed_value():
    instance_text = powerio.serialize(
        powerio.parse(DATA / "case9.m").to_ac_ots_instance()
    ).text
    instance_document = json.loads(instance_text)
    # Keeping the initial topology changes nothing, so the fixed problem is
    # the base AC OPF without the switching cost term.
    base = instance_document["value"]["data"]["base"]
    fixed = dict(base)
    fixed["objective"] = {
        "terms": [
            term
            for term in base["objective"]["terms"]
            if term["term"] != "branch_switching_cost"
        ]
    }
    fixed_text = json.dumps(
        dict(instance_document, value={"type": "powerio.AcOpfInstance", "data": fixed})
    )
    solution = dict(
        instance_document,
        value={
            "type": "powerio.AcOtsSolution",
            "data": {
                "instance": instance_document["value"]["data"],
                "branch_closed": [True] * 9,
                "ac_solution": _ac_solution_data(fixed_text, 40.0),
            },
        },
    )
    module = powerio.deserialize(io.StringIO(json.dumps(solution)))
    assert isinstance(module.value, powerio.AcOtsSolution)
    assert isinstance(module.value.instance, powerio.AcOtsInstance)
    assert all(branch["in_service"] for branch in module.value.network.branches)
