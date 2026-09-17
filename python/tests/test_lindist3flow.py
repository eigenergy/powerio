"""Typed LinDist3Flow values use physical identities and owned result columns."""

import gc
import io
import json
from pathlib import Path

import pytest

import powerio

FIXTURE = Path(__file__).resolve().parents[2] / "tests/data/dist/micro/lindist3flow-solution.pio.json"


def test_lindist3flow_solution_access_and_lifetimes():
    module = powerio.deserialize(FIXTURE)
    solution = module.value
    assert isinstance(solution, powerio.LinDist3FlowOpfSolution)
    instance = solution.instance
    assert isinstance(instance, powerio.LinDist3FlowOpfInstance)
    network = instance.network
    assert solution.termination == "converged"
    assert solution.objective == 1.0
    assert solution["line_active_power"] == [1000.0]
    assert solution["source_reactive_power"] == [200.0]
    assert solution["generator_active_power"] == []
    with pytest.raises(KeyError):
        solution["missing"]
    values = solution["line_active_power"]
    values[0] = 0.0
    assert solution["line_active_power"] == [1000.0]
    del module, solution
    gc.collect()
    metadata = instance.metadata
    assert set(metadata["nodes"]) == {("source", "a"), ("load", "a")}
    assert metadata["roots"] == [("source", "a")]
    conductor = metadata["conductors"][0]
    assert conductor["parent"] == ("source", "a")
    assert conductor["child"] == ("load", "a")
    assert conductor["reversed"]
    assert conductor["source_line_row"] == conductor["conductor_position"] == 0
    assert metadata["reference_voltages"] == [(230.0, 0.0), (230.0, 0.0)]
    assert metadata["reference_provenance"] == "source_propagated"
    assert metadata["meshed"] is False
    assert metadata["preparation_policy"] == "reject"
    assert metadata["preparation_actions"] == []
    assert network.n_buses == 2
    document = json.loads(powerio.serialize(instance.module).text)
    assert document["version"] == 2
    assert document["value"]["type"] == "powerio.LinDist3FlowOpfInstance"


def test_lindist3flow_construction_and_ir_generation():
    document = json.loads(FIXTURE.read_text())
    base = document["value"]["data"]["instance"]["base"]
    document["value"] = {"type": "powerio.MulticonductorNetwork", "data": base["network"]}
    document["version"] = 2
    network_module = powerio.deserialize(io.StringIO(json.dumps(document)))
    instance = network_module.to_lindist3flow_opf_instance()
    assert isinstance(instance.value, powerio.LinDist3FlowOpfInstance)
    assert instance.value.metadata["roots"] == [("source", "a")]
    for document in [json.loads(FIXTURE.read_text()), json.loads(powerio.serialize(instance).text)]:
        assert document["version"] == 2
        assert isinstance(
            powerio.deserialize(io.StringIO(json.dumps(document))).value,
            (powerio.LinDist3FlowOpfInstance, powerio.LinDist3FlowOpfSolution),
        )
        document["version"] = 3
        with pytest.raises(powerio.PowerIOError, match="unsupported"):
            powerio.deserialize(io.StringIO(json.dumps(document)))
