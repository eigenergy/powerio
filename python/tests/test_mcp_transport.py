"""Exercise the MCP server through a real stdio session."""

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
TIMEOUT = 60.0


def _run(steps):
    async def go():
        parameters = StdioServerParameters(
            command=sys.executable,
            args=["-m", "powerio.mcp"],
            env=dict(os.environ),
        )
        async with stdio_client(parameters) as (read, write):
            async with ClientSession(
                read, write, read_timeout_seconds=TIMEOUT
            ) as session:
                await asyncio.wait_for(session.initialize(), TIMEOUT)
                return await steps(session)

    return asyncio.run(go())


def _payload(result):
    assert not result.is_error, result.content[0].text
    return json.loads(result.content[0].text)


def test_module_round_trip_over_stdio():
    async def steps(session):
        parsed = _payload(
            await session.call_tool("parse", {"path": str(DATA / "case9.m")})
        )
        summary = _payload(
            await session.call_tool(
                "summarize", {"powerio_ir": parsed["powerio_ir"]}
            )
        )
        diagnostics = _payload(
            await session.call_tool(
                "diagnostics", {"powerio_ir": parsed["powerio_ir"]}
            )
        )
        return parsed, summary, diagnostics

    parsed, summary, diagnostics = _run(steps)
    assert parsed["value_type"] == "powerio.BalancedNetwork"
    assert summary["elements"]["buses"] == 9
    assert diagnostics["summary"]["status"] == "ok"


def test_raw_content_reaches_parse_and_emit_over_stdio():
    text = (DATA / "case9.m").read_text()

    async def steps(session):
        parsed = _payload(
            await session.call_tool(
                "parse", {"content": text, "format": "matpower"}
            )
        )
        emitted = _payload(
            await session.call_tool(
                "emit",
                {
                    "format": "matpower",
                    "content": text,
                    "source_format": "matpower",
                },
            )
        )
        return parsed, emitted

    parsed, emitted = _run(steps)
    assert parsed["summary"]["elements"]["buses"] == 9
    assert emitted["text"] == text


def test_collection_indexing_over_stdio_does_not_create_a_network(
    time_series_powerio_ir,
):
    async def steps(session):
        collection = _payload(
            await session.call_tool(
                "summarize", {"powerio_ir": time_series_powerio_ir}
            )
        )
        selected = _payload(
            await session.call_tool(
                "summarize",
                {"powerio_ir": time_series_powerio_ir, "time_index": 0},
            )
        )
        return collection, selected

    collection, selected = _run(steps)
    assert collection["collection"] == "TimeSeries"
    assert selected["value_type"] == "OperatingPoint"
    assert "elements" not in selected


def test_removed_collection_tools_are_not_advertised_over_stdio():
    async def steps(session):
        result = await session.list_tools()
        return {tool.name for tool in result.tools}

    names = _run(steps)
    assert "summarize" in names
    assert "list_states" not in names
    assert "inspect_state" not in names
    assert "export_state" not in names
