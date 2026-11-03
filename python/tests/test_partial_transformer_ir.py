"""IR fail-closed envelopes must not leak into physical Python table rows."""
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
        assert stored["value"] == row
    else:
        assert stored == row
    assert powerio.deserialize(text.encode()).value.transformers_3w == [row]
