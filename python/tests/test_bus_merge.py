"""The bus merge from Python: closed switches and zero impedance branches."""

from pathlib import Path

import pytest

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"


def _merge_case() -> powerio.BalancedNetwork:
    return powerio.parse(DATA / "psse" / "merge_v35.raw").value


def test_psse_rule_merges_the_jumper_chain_and_the_closed_switch():
    network = _merge_case()
    merge = network.merge_buses(zero_impedance="psse")

    # Buses 2, 3, 4 share the jumper chain; the closed switch joins 5 to
    # the reference bus 1.
    assert merge.merged_buses == {3: 2, 4: 2, 5: 1}
    assert merge.groups == [
        {"survivor": 1, "members": [1, 5]},
        {"survivor": 2, "members": [2, 3, 4]},
    ]
    assert merge.survivor(4) == 2
    assert [removed["reason"] for removed in merge.removed_branches] == [
        "zero_impedance",
        "zero_impedance",
    ]
    assert [removed["row"] for removed in merge.removed_branches] == [1, 2]
    assert merge.removed_switches[0]["reason"] == "closed_switch"
    assert merge.branch_rows == [0, None, None, 1]
    assert merge.switch_rows == [None]
    assert merge.network.n_buses == 2
    assert network.n_buses == 5, "the input is unchanged"
    codes = {diagnostic.code for diagnostic in merge.diagnostics}
    assert codes == {
        "CANONICALIZE.MERGE.ZERO_IMPEDANCE",
        "CANONICALIZE.MERGE.CLOSED_SWITCH",
    }

    again = merge.network.merge_buses(zero_impedance="psse", threshold=1e-4)
    assert again.merged_buses == {}


def test_removed_flows_follow_from_the_merged_solution():
    merge = _merge_case().merge_buses(zero_impedance="psse")
    # The merged network is buses 1 and 2 joined by the 1-2 line (x 0.1) and
    # the former 4-5 line, now 2-1 (x 0.2); 40 MW flows from 1 to 2.
    p_from = [80.0 / 3.0, -40.0 / 3.0]
    flows = merge.calc_removed_flows(p_from, [-p for p in p_from])
    assert [flow["p_from"] for flow in flows["branches"]] == pytest.approx(
        [80.0 / 3.0, -40.0 / 3.0]
    )
    assert flows["switches"][0]["p_from"] == pytest.approx(-220.0 / 3.0)
    assert {flow["method"] for flow in flows["branches"]} == {"tree"}
    assert flows["diagnostics"] == []


def test_rule_arguments_are_checked():
    network = _merge_case()
    with pytest.raises(ValueError, match="requires a threshold"):
        network.merge_buses(zero_impedance="impedance")
    with pytest.raises(ValueError, match="unknown zero_impedance rule"):
        network.merge_buses(zero_impedance="nearby")
    with pytest.raises(powerio.PowerIODataError) as raised:
        network.merge_buses(zero_impedance="psse", threshold=-1.0)
    assert raised.value.code == "CANONICALIZE.MERGE.INVALID_RULE"

    # MATPOWER states no THRSHZ, so "psse" needs an explicit threshold there.
    case9 = powerio.parse(DATA / "case9.m").value
    with pytest.raises(powerio.PowerIODataError):
        case9.merge_buses(zero_impedance="psse")
    assert case9.merge_buses(zero_impedance="exact").merged_buses == {}
