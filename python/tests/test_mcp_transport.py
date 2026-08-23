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
        parsed = _payload(await session.call_tool("parse", {"path": str(DATA / "case9.m")}))
        assert parsed["json_format"] == "model-json"
        return _payload(await session.call_tool("summary", {"json": parsed["json"]}))

    assert _run(steps)["elements"]["buses"] == 9


def test_the_package_transport_round_trips_over_stdio():
    async def steps(session):
        parsed = _payload(
            await session.call_tool(
                "parse", {"path": str(DATA / "case9.m"), "transport": "package"}
            )
        )
        package = parsed["package_json"]
        diag = _payload(await session.call_tool("diagnostics", {"package_json": package}))
        assert diag["schema"] == "powerio.diagnostics"
        return _payload(await session.call_tool("summary", {"package_json": package}))

    assert _run(steps)["elements"]["buses"] == 9


def test_non_json_content_survives_the_transport():
    async def steps(session):
        return _payload(
            await session.call_tool(
                "convert",
                {
                    "to_format": "psse",
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


# Three phase throughout, so the lowering pass has no blocker; the switch
# fixture carries a closed switch it will not project.
LOWERABLE = DATA / "dist" / "micro" / "fourwire_linecode.dss"
BLOCKED = DATA / "dist" / "micro" / "switch.dss"


def test_lower_preflight_and_apply_over_stdio():
    async def steps(session):
        pre = _payload(
            await session.call_tool(
                "lower", {"path": str(LOWERABLE), "mode": "preflight"}
            )
        )
        applied = _payload(
            await session.call_tool("lower", {"path": str(LOWERABLE), "mode": "apply"})
        )
        # The derived package survives the transport and the balanced verbs
        # take it as-is, which is the point of returning it rather than a
        # summary of it.
        summary = _payload(
            await session.call_tool(
                "summary", {"package_json": applied["package_json"]}
            )
        )
        return pre, applied, summary

    pre, applied, summary = _run(steps)
    assert pre["schema"] == "powerio.lower"
    assert pre["readiness"]["ready"] is True
    assert "package_json" not in pre
    assert applied["applied"] is True
    assert json.loads(applied["package_json"])["model_kind"] == "balanced"
    assert applied["lowering_history"][-1]["pass"] == "multiconductor-to-balanced"
    assert summary["domain"] == "transmission"


def test_lower_refuses_a_blocker_as_structured_data_over_stdio():
    async def steps(session):
        return _payload(
            await session.call_tool("lower", {"path": str(BLOCKED), "mode": "apply"})
        )

    out = _run(steps)
    assert out["applied"] is False
    assert "package_json" not in out
    codes = {b["code"] for b in out["readiness"]["blockers"]}
    assert "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CLOSED_SWITCH" in codes


def test_lower_takes_the_package_transport_over_stdio():
    async def steps(session):
        parsed = _payload(
            await session.call_tool(
                "parse", {"path": str(LOWERABLE), "transport": "package"}
            )
        )
        return _payload(
            await session.call_tool(
                "lower", {"package_json": parsed["package_json"], "mode": "apply"}
            )
        )

    assert json.loads(_run(steps)["package_json"])["model_kind"] == "balanced"
