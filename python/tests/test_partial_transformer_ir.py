"""IR fail-closed envelopes must not leak into physical Python table rows."""
import copy
import json
from pathlib import Path

import pytest

import powerio

RAW = (Path(__file__).resolve().parents[2] / "tests/data/psse/partial_3w_status.raw").read_text()


@pytest.mark.parametrize("stat,flags", [(0, [True, True, True]), (1, [True, True, True]),
    (2, [True, False, True]), (3, [True, True, False]), (4, [False, True, True])])
def test_partial_transformer_keeps_python_rows_and_fails_closed_ir(stat, flags):
    raw = RAW.replace("'T3W 2       ', 2,", f"'T3W 2       ', {stat},")
    module = powerio.parse(raw.encode(), format="psse", name="partial.raw")
    row = module.value.transformers_3w[0]
    assert "type" not in row and "value" not in row
    assert len(row["windings"]) == 3
    assert row["in_service"] == (stat != 0)
    assert row["extras"].get("winding_in_service", [True] * 3) == flags
    text = powerio.serialize(module).text
    stored = json.loads(text)["value"]["data"]["transformers_3w"][0]
    if stat >= 2:
        assert stored["type"] == "powerio.PartialTransformer3W"
        assert "windings" not in stored
        body = stored["value"]
    else:
        body = stored
    # JSON spells infinity as a string; typed Python ratings are floats.
    expected = copy.deepcopy(body)
    for winding in expected["windings"]:
        for key in ("rate_a", "rate_b", "rate_c"):
            if winding[key] == "Infinity":
                winding[key] = float("inf")
    assert body["windings"][2]["rate_b"] == "Infinity"
    assert row["windings"][2]["rate_b"] == float("inf")
    assert expected == row
    assert powerio.deserialize(text.encode()).value.transformers_3w == [row]


def test_zero_and_infinity_remain_numeric_in_python_winding_tables():
    module = powerio.parse(RAW.encode(), format="psse", name="partial.raw")
    ir = json.loads(powerio.serialize(module).text)
    body = ir["value"]["data"]["transformers_3w"][0]["value"]
    winding = body["windings"][0]
    winding.update(rate_a=0.0, rate_b="Infinity", rate_c=120.0)
    body["extras"]["winding_rating_sets"] = [[{
        "name": "zero-emergency", "rate_mva": 0.0,
    }], [], []]
    module = powerio.deserialize(json.dumps(ir).encode())
    row = module.value.transformers_3w[0]
    assert "type" not in row and "value" not in row
    assert "type" not in row["windings"][0]
    assert row["windings"][0]["rate_a"] == 0.0
    assert row["windings"][0]["rate_b"] == float("inf")
    assert row["windings"][0]["rate_c"] == 120.0
    assert row["extras"]["winding_rating_sets"][0] == [{"name": "zero-emergency", "rate_mva": 0.0}]
    assert row["extras"]["winding_in_service"] == [True, False, True]
    restored = powerio.deserialize(powerio.serialize(module).text.encode())
    assert restored.value.transformers_3w == [row]
