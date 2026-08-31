"""The MCP server driven over a real stdio transport.

The other MCP tests call the tool functions in process, which skips the SDK's
argument handling entirely. That gap hid a real defect: unless the annotation
is exactly ``str``, the SDK re-parses a string argument whose text reads as
JSON into an object before validation, so every transport argument carrying
JSON was destroyed before the tool saw it. These tests spawn
``python -m powerio.mcp`` and speak the protocol.
"""

import asyncio
import json
import os
import sys
from pathlib import Path

import pytest

pytest.importorskip("mcp", reason="powerio[mcp] not installed (needs Python 3.10+)")

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"

# The SDK waits forever by default; a server that starts and then blocks
# would hang the suite with nothing to fail it.
TIMEOUT = 60.0

# A payload the JSON reader must receive byte intact: a non-ASCII bus name
# and a float a decode and re-encode cycle is prone to mangle.
BMOPF_MALMO = '{"bus":{"Malmö":{"terminal_names":["1"],"v_max":[1e21]}}}'
LOWERABLE_DSS = """Clear
Set DefaultBaseFrequency=60
New Circuit.tiny basekv=12.47 pu=1.0 phases=3 bus1=src MVAsc3=2000 MVAsc1=2100
New Transformer.t1 phases=3 windings=2 buses=(src, sec) conns=(delta, wye) kvs=(12.47, 0.416) kvas=(500, 300) %Rs=(0.5, 0.5) xhl=6
New Load.l1 bus1=sec phases=3 conn=wye kv=0.416 kw=90 pf=0.95 model=1
Set VoltageBases=[12.47, 0.416]
"""


def _run(steps):
    """Drive one stdio session; ``steps`` is an async callable on the session."""

    async def go():
        params = StdioServerParameters(
            command=sys.executable, args=["-m", "powerio.mcp"], env=dict(os.environ)
        )
        async with stdio_client(params) as (read, write):
            async with ClientSession(
                read, write, read_timeout_seconds=TIMEOUT
            ) as session:
                await asyncio.wait_for(session.initialize(), TIMEOUT)
                return await steps(session)

    return asyncio.run(go())


def _payload(result):
    assert not result.is_error, result.content[0].text
    return json.loads(result.content[0].text)


def test_the_json_transport_round_trips_over_stdio():
    async def steps(session):
        parsed = _payload(
            await session.call_tool("parse", {"path": str(DATA / "case9.m")})
        )
        assert parsed["json_format"] == "model-json"
        return _payload(await session.call_tool("summarize", {"json": parsed["json"]}))

    assert _run(steps)["elements"]["buses"] == 9


def test_the_module_transport_round_trips_over_stdio():
    async def steps(session):
        parsed = _payload(
            await session.call_tool(
                "parse", {"path": str(DATA / "case9.m"), "transport": "module"}
            )
        )
        module = parsed["module_json"]
        diag = _payload(await session.call_tool("diagnostics", {"module_json": module}))
        assert diag["schema"] == "powerio.diagnostics"
        return _payload(await session.call_tool("summarize", {"module_json": module}))

    assert _run(steps)["elements"]["buses"] == 9


def test_non_json_content_survives_the_transport():
    async def steps(session):
        return _payload(
            await session.call_tool(
                "emit",
                {
                    "format": "psse",
                    "content": (DATA / "case9.m").read_text(),
                    "from_format": "matpower",
                },
            )
        )

    assert _run(steps)["text"].lstrip().startswith("0,")


def test_json_content_reaches_the_reader_byte_intact():
    async def steps(session):
        return _payload(await session.call_tool("parse", {"content": BMOPF_MALMO}))

    parsed = _run(steps)
    assert parsed["domain"] == "distribution"
    doc = json.loads(parsed["json"])
    assert doc["bus"]["Malmö"]["v_max"] == [1e21]


def test_module_tools_receive_json_strings_over_stdio():
    async def steps(session):
        series = (DATA / "package" / "frozen-0.9-series.pio.json").read_text()
        inspected = _payload(
            await session.call_tool("inspect", {"module_json": series})
        )
        listed = _payload(
            await session.call_tool("list_states", {"module_json": series})
        )
        selected = _payload(
            await session.call_tool(
                "inspect_state", {"module_json": series, "time_position": 0}
            )
        )
        exported = _payload(
            await session.call_tool(
                "export_state", {"module_json": series, "time_position": 0}
            )
        )

        parsed = _payload(
            await session.call_tool(
                "parse",
                {
                    "content": LOWERABLE_DSS,
                    "from_format": "dss",
                    "transport": "module",
                },
            )
        )
        module_json = parsed["module_json"]
        report = _payload(
            await session.call_tool("to_balanced_report", {"module_json": module_json})
        )
        lowered = _payload(
            await session.call_tool("to_balanced", {"module_json": module_json})
        )
        return inspected, listed, selected, exported, report, lowered

    inspected, listed, selected, exported, report, lowered = _run(steps)
    assert inspected["kind"] == "balanced_operating_point_time_series"
    assert listed["time_points"]
    assert selected["selected"]["item"] == "balanced_operating_point"
    assert json.loads(exported["module_json"])["schema"] == "powerio.module"
    assert report["ready"] is True
    assert lowered["kind"] == "balanced_network"
