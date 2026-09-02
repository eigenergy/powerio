import json
from pathlib import Path

import pytest

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"


@pytest.fixture
def time_series_powerio_ir() -> str:
    """A version 1 module containing two balanced operating points."""
    network_module = json.loads(powerio.serialize(powerio.parse(DATA / "case9.m")).text)
    return json.dumps(
        {
            "schema": network_module["schema"],
            "version": network_module["version"],
            "producer": network_module["producer"],
            "value": {
                "type": (
                    "powerio.TimeSeries<"
                    "powerio.OperatingPoint<powerio.BalancedNetwork>>"
                ),
                "data": {
                    "network": network_module["value"]["data"],
                    "time_points": [{"label": "h0"}, {"label": "h1"}],
                    "values": [{"quantities": {}}, {"quantities": {}}],
                },
            },
        }
    )
