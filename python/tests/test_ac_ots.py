"""AC optimal transmission switching in Python: the typed values a module
holds, construction with and without options, the branch rows, the topology
checks, and attaching an AC OPF solution."""

import io
import json
from pathlib import Path

import pytest

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
TOPOLOGY_INVALID = "BUILD.INSTANCE.TOPOLOGY_INVALID"
OPTIONS_INVALID = "BUILD.INSTANCE.SWITCHING_OPTIONS_INVALID"


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


# Options, branch rows, topology checks, and results on case14, whose 20
# branches are all in service initially.


def _case14():
    return powerio.parse(DATA / "case14.m")


def _row_of(instance, from_bus, to_bus):
    """The branch table position of the branch between two buses."""
    for row, branch in enumerate(instance.network.branches):
        if {branch["from_id"], branch["to_id"]} == {from_bus, to_bus}:
            return row
    raise AssertionError(f"no branch {from_bus}-{to_bus}")


def _opened(instance, *rows):
    closed = [True] * len(instance.branches)
    for row in rows:
        closed[row] = False
    return closed


def _ac_opf_solution(fixed_module, objective):
    """A synthetic AC OPF solution of a fixed-topology module."""
    fixed_text = powerio.serialize(fixed_module).text
    document = json.loads(fixed_text)
    document["value"] = {
        "type": "powerio.AcOpfSolution",
        "data": _ac_solution_data(fixed_text, objective),
    }
    return powerio.deserialize(io.StringIO(json.dumps(document))).value


def test_default_options_equal_the_no_options_instance():
    module = _case14()
    plain = module.to_ac_ots_instance().value
    options = plain.options
    ids = [row["id"] for row in plain.branches]
    assert len(ids) == 20
    assert options == {
        "switchable": ids,
        "switching_costs": {},
        "islanding": "forbidden",
        "max_open": None,
        "max_switched": None,
        "current_limits": "skip",
        "stated_current_limits": {},
    }
    # The options read back rebuild the same instance.
    explicit = module.to_ac_ots_instance(**options).value
    assert explicit.options == options
    assert explicit.branches == plain.branches
    assert (
        json.loads(powerio.serialize(explicit.module).text)["value"]
        == json.loads(powerio.serialize(plain.module).text)["value"]
    )
    # Any option makes the options explicit: then every branch may switch,
    # as in C and Julia.
    budgeted = module.to_ac_ots_instance(max_switched=2).value
    assert budgeted.options["switchable"] == "all"
    assert budgeted.options["max_switched"] == 2


def test_options_and_branch_rows():
    module = _case14()
    ids = [row["id"] for row in module.to_ac_ots_instance().value.branches]
    instance = module.to_ac_ots_instance(
        switchable=ids[:3],
        switching_costs={ids[0]: (4.0, 7.0), ids[1]: (2.0, 5.0)},
        islanding="allowed",
        max_open=2,
        max_switched=2,
        current_limits="stated",
        stated_current_limits={ids[1]: (1.25, None)},
    ).value
    assert isinstance(instance, powerio.AcOtsInstance)
    options = instance.options
    assert options["switchable"] == ids[:3]
    assert options["switching_costs"] == {ids[0]: (4.0, 7.0), ids[1]: (2.0, 5.0)}
    assert options["islanding"] == "allowed"
    assert (options["max_open"], options["max_switched"]) == (2, 2)
    assert options["current_limits"] == "stated"
    assert options["stated_current_limits"] == {ids[1]: (1.25, None)}
    rows = instance.branches
    assert [row["switchable"] for row in rows] == [True] * 3 + [False] * 17
    assert all(row["initial_closed"] for row in rows)
    assert rows[0]["switching_cost"] == (4.0, 7.0)
    assert rows[2]["switching_cost"] is None
    assert rows[1]["stated_current"] == (1.25, None)
    assert rows[1]["current"] == (1.25, None)
    assert rows[0]["current"] == (None, None)
    derived = module.to_ac_ots_instance(current_limits="derive_from_rate_a").value
    assert all(row["current"][0] is not None for row in derived.branches)
    assert all(row["stated_current"] == (None, None) for row in derived.branches)


def test_refused_options():
    module = _case14()
    with pytest.raises(powerio.PowerIOError) as unknown:
        module.to_ac_ots_instance(switchable=["no-such-branch"])
    assert unknown.value.code == OPTIONS_INVALID
    with pytest.raises(powerio.PowerIOError) as budget:
        module.to_ac_ots_instance(switchable="none", max_switched=1)
    assert budget.value.code == OPTIONS_INVALID
    with pytest.raises(ValueError):
        module.to_ac_ots_instance(islanding="sometimes")
    with pytest.raises(ValueError):
        module.to_ac_ots_instance(current_limits="rated")
    with pytest.raises(ValueError):
        module.to_ac_ots_instance(max_open=-1)


def test_validate_topology_returns_the_switching_cost():
    module = _case14()
    default = module.to_ac_ots_instance().value
    ids = [row["id"] for row in default.branches]
    row = _row_of(default, 1, 2)
    instance = module.to_ac_ots_instance(
        switchable=ids, switching_costs={ids[row]: (4.0, 7.0)}
    ).value
    assert instance.validate_topology([True] * 20) == 0.0
    assert instance.validate_topology(_opened(instance, row)) == 4.0
    with pytest.raises(powerio.PowerIOError) as short:
        instance.validate_topology([True])
    assert short.value.code == TOPOLOGY_INVALID
    with pytest.raises(TypeError):
        instance.validate_topology([1] * 20)


def test_forbidden_islanding_refuses_a_split():
    module = _case14()
    default = module.to_ac_ots_instance().value
    # Bus 8 hangs off bus 7 by one branch; opening it splits the network.
    cut = _opened(default, _row_of(default, 7, 8))
    with pytest.raises(powerio.PowerIOError) as split:
        default.validate_topology(cut)
    assert split.value.code == TOPOLOGY_INVALID
    assert "islanding is forbidden" in str(split.value)
    with pytest.raises(powerio.PowerIOError):
        default.fix_topology(cut)
    # Bus 8 keeps its generator, so an allowed split is accepted, and the
    # new island gets its own reference bus.
    allowed = module.to_ac_ots_instance(islanding="allowed").value
    assert allowed.validate_topology(cut) == 0.0
    _, diagnostics = allowed.fix_topology(cut)
    assert "TRANSFORM.INSTANCE.ASSUMPTION" in {d.code for d in diagnostics}


def test_a_budget_refuses_too_many_changes():
    module = _case14()
    instance = module.to_ac_ots_instance(max_switched=1).value
    one = _opened(instance, _row_of(instance, 1, 2))
    two = _opened(instance, _row_of(instance, 1, 2), _row_of(instance, 2, 3))
    assert instance.validate_topology(one) == 0.0
    with pytest.raises(powerio.PowerIOError) as over:
        instance.validate_topology(two)
    assert over.value.code == TOPOLOGY_INVALID
    assert "at most 1 may change" in str(over.value)
    open_budget = module.to_ac_ots_instance(max_open=1).value
    with pytest.raises(powerio.PowerIOError) as too_open:
        open_budget.validate_topology(two)
    assert too_open.value.code == TOPOLOGY_INVALID


def test_fix_topology_gives_an_ac_opf_instance_with_the_branch_open():
    instance = _case14().to_ac_ots_instance().value
    row = _row_of(instance, 1, 2)
    fixed, diagnostics = instance.fix_topology(_opened(instance, row))
    assert isinstance(fixed, powerio.AcOpfInstance)
    in_service = [branch["in_service"] for branch in fixed.network.branches]
    assert in_service == _opened(instance, row)
    assert diagnostics
    assert all(isinstance(d, powerio.Diagnostic) for d in diagnostics)
    assert {d.code for d in diagnostics} == {"TRANSFORM.INSTANCE.DATA_DISCARDED"}
    # The module carries the same diagnostics after the inherited ones.
    assert fixed.module.diagnostics[-len(diagnostics) :] == diagnostics
    assert fixed.module.type_name == "powerio.AcOpfInstance"


def test_attaching_an_ac_opf_solution():
    module = _case14()
    ids = [row["id"] for row in module.to_ac_ots_instance().value.branches]
    instance = module.to_ac_ots_instance(
        switchable=ids, switching_costs={ids[0]: (4.0, 7.0)}
    ).value
    row = _row_of(instance, 1, 2)
    assert row == 0
    closed = _opened(instance, row)
    fixed, _ = instance.fix_topology(closed)
    ac = _ac_opf_solution(fixed.module, 100.0)
    assert isinstance(ac, powerio.AcOpfSolution)

    solution = powerio.AcOtsSolution.from_ac_opf_solution(instance, closed, ac)
    assert isinstance(solution, powerio.AcOtsSolution)
    assert solution.branch_closed == closed
    assert solution.switching_cost == 4.0
    assert solution.branches_open == 1
    assert solution.branches_switched == 1
    assert solution.objective == 104.0
    assert isinstance(solution.instance, powerio.AcOtsInstance)
    # The solution's own network keeps the initial statuses; the solved
    # network applies the final ones.
    assert all(branch["in_service"] for branch in solution.network.branches)
    solved = solution.solved_network()
    assert solved.type_name == "powerio.BalancedNetwork"
    assert [b["in_service"] for b in solved.value.branches] == closed

    # The solution must answer the fixed problem of the topology it states.
    with pytest.raises(powerio.PowerIOError) as mismatch:
        powerio.AcOtsSolution.from_ac_opf_solution(instance, [True] * 20, ac)
    assert mismatch.value.code == "BUILD.SOLUTION.TOPOLOGY_MISMATCH"
    with pytest.raises(TypeError):
        powerio.AcOtsSolution.from_ac_opf_solution(instance, closed, fixed)
