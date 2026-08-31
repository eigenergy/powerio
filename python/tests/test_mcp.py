"""Tests for the optional MCP server (``powerio.mcp``)."""

import asyncio
import json
import os
from pathlib import Path

import pytest

pytest.importorskip("mcp", reason="powerio[mcp] not installed (needs Python 3.10+)")

from powerio.mcp import server

import powerio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
DSS = DATA / "dist" / "micro" / "xfmr_single_phase.dss"
BMOPF = DATA / "dist" / "bmopf" / "example_ieee13.json"
PSSE_CASE5 = DATA / "psse" / "case5.raw"
PMD = DATA / "dist" / "pmd" / "ieee13.json"
PWD = DATA / "powerworld" / "ACTIVSg200.pwd"
MINIMAL_BMOPF = '{"bus":{"a":{"terminal_names":["1"]}}}'

HAS_GRIDFM = bool(getattr(powerio._powerio, "_has_gridfm", False))
gridfm_only = pytest.mark.skipif(
    not HAS_GRIDFM, reason="extension built without the gridfm feature"
)


def _emit_text(*, format=None, **inputs):
    return server.emit(format=format, **inputs)


def _emit_to(*, destination, format=None, **inputs):
    target = format or server._infer_format_from_destination(destination)
    return server.emit(format=target, destination=destination, **inputs)


def _parse(source, format=None, *, value_type=None):
    if isinstance(source, (bytes, bytearray, memoryview)):
        return powerio.parse_text(
            bytes(source).decode(),
            name="fixture",
            format=format,
            value_type=value_type,
        )
    return powerio.parse_file(source, format, value_type=value_type)


def _emit_module(module):
    return module.emit("pio-json").text


def _parse_module(text):
    return powerio.parse_text(text, name="module.pio.json")


def test_tool_surface_is_semantic():
    tools = {t.name: t for t in asyncio.run(server.mcp.list_tools())}
    names = set(tools)
    assert names == {
        "emit",
        "summarize",
        "parse",
        "to_normalized",
        "calc_matrix",
        "diagnostics",
        "display",
        "inspect",
        "list_states",
        "inspect_state",
        "export_state",
        "to_balanced_report",
        "to_balanced",
        "about",
    }
    for name in (
        "parse",
        "summarize",
        "to_normalized",
        "calc_matrix",
        "display",
    ):
        props = tools[name].input_schema["properties"]
        assert "from_format" in props
        assert "format" not in props
    emit_schema = tools["emit"].input_schema
    assert emit_schema["required"] == ["format"]
    emit_props = emit_schema["properties"]
    assert "format" in emit_props and "destination" in emit_props
    assert "to_format" not in emit_props and "out_path" not in emit_props
    assert "from_format" in emit_props
    assert "transport" in tools["parse"].input_schema["properties"]
    # The SDK 2.0 model field is snake_case `input_schema`; the wire format is
    # still the protocol's camelCase `inputSchema`.
    assert "inputSchema" in tools["emit"].model_dump(by_alias=True)
    # Transport text arguments are bare `str` with "" meaning unset: under any
    # other annotation the SDK re-parses JSON text before validation and the
    # tool receives an object instead of the document.
    for name in (
        "emit",
        "summarize",
        "to_normalized",
        "calc_matrix",
    ):
        props = tools[name].input_schema["properties"]
        for arg in ("content", "json", "module_json"):
            assert props[arg]["type"] == "string"
            assert props[arg]["default"] == ""
    for name in (
        "inspect",
        "list_states",
        "inspect_state",
        "export_state",
        "to_balanced_report",
        "to_balanced",
    ):
        props = tools[name].input_schema["properties"]
        for arg in ("content", "module_json"):
            assert props[arg]["type"] == "string"
            assert props[arg]["default"] == ""
    parse_content = tools["parse"].input_schema["properties"]["content"]
    assert parse_content["type"] == "string" and parse_content["default"] == ""
    # Matrix kind aliases are advertised; JSON transport has one spelling per meaning.
    kind = tools["calc_matrix"].input_schema["properties"]["kind"]
    assert kind["enum"] == sorted(server._MATRIX_KIND_ALIASES)
    json_format = tools["summarize"].input_schema["properties"]["json_format"]
    assert json_format["anyOf"][0]["enum"] == ["module", "model-json", "bmopf-json"]
    # The open ended format name sets are prose in the descriptions.
    assert "matpower" in tools["emit"].description
    assert "bmopf-json" in tools["emit"].description
    assert "matpower" in tools["parse"].description
    for removed in (
        "convert",
        "save",
        "normalize",
        "matrix",
        "summary",
        "state_inventory",
        "select_state",
        "to_balanced_inspect",
        "dc_data",
    ):
        assert removed not in tools


def test_summary_transmission_schema():
    s = server.summarize(path=str(DATA / "case9.m"))
    assert s["schema"] == "powerio.summarize"
    assert s["powerio_version"] == powerio.__version__
    assert s["domain"] == "transmission"
    assert s["model"] == "balanced"
    assert s["json_format"] == "model-json"
    assert s["source_format"] == "matpower"
    assert s["base_mva"] == 100.0
    assert s["elements"]["buses"] == 9
    assert s["elements"]["branches"] == 9
    assert s["topology"]["connected_components"] == 1
    assert s["topology"]["reference_buses"] == [0]
    assert s["diagnostics"] == []


def test_summary_distribution_schema_and_json_sniffing():
    for path in (DSS, BMOPF, PMD):
        s = server.summarize(path=str(path))
        assert s["schema"] == "powerio.summarize"
        assert s["domain"] == "distribution"
        assert s["model"] == "multiconductor"
        assert s["json_format"] == "bmopf-json"
        assert s["elements"]["buses"] > 0
        assert s["elements"]["sources"] >= 0
        assert s["topology"]["connected_components"] is None


def test_distribution_aliases_route_to_core_parser():
    text = DSS.read_text()
    for fmt in ("dss", "opendss"):
        assert (
            server.summarize(content=text, from_format=fmt)["domain"] == "distribution"
        )
    assert (
        json.loads(_emit_text(format="pmd", path=str(DSS))["text"])["data_model"]
        == "ENGINEERING"
    )
    assert "bus" in json.loads(_emit_text(format="bmopf", path=str(DSS))["text"])


def test_parse_transmission_transport_round_trip(tmp_path):
    parsed = server.parse(path=str(DATA / "case9.m"))
    assert parsed["schema"] == "powerio.parse"
    assert parsed["powerio_version"] == powerio.__version__
    assert parsed["domain"] == "transmission"
    assert parsed["model"] == "balanced"
    assert parsed["source_format"] == "matpower"
    assert parsed["json_format"] == "model-json"
    assert powerio.from_json(parsed["json"]).n_buses == 9
    assert (
        server.summarize(json=parsed["json"], json_format="model-json")["elements"][
            "buses"
        ]
        == 9
    )
    # The transport is inferred from the document when no format is passed.
    assert server.summarize(json=parsed["json"])["elements"]["buses"] == 9

    out = tmp_path / "case9.m"
    _emit_to(
        destination=str(out), json=parsed["json"], json_format=parsed["json_format"]
    )
    assert _parse(out, value_type=powerio.BalancedNetwork).value.n_buses == 9


def test_parse_distribution_uses_bmopf_transport(tmp_path):
    parsed = server.parse(path=str(DSS))
    assert parsed["schema"] == "powerio.parse"
    assert parsed["domain"] == "distribution"
    assert parsed["model"] == "multiconductor"
    assert parsed["json_format"] == "bmopf-json"
    doc = json.loads(parsed["json"])
    assert "bus" in doc and "voltage_source" in doc
    assert (
        server.summarize(json=parsed["json"], json_format="bmopf-json")["elements"][
            "sources"
        ]
        >= 1
    )

    out = tmp_path / "feeder.dss"
    _emit_to(
        destination=str(out), json=parsed["json"], json_format=parsed["json_format"]
    )
    assert "new circuit" in out.read_text().lower()


def test_module_transport_flows_through_summary_matrix_and_emit(tmp_path):
    parsed = server.parse(path=str(DATA / "case9.m"), transport="module")
    assert parsed["schema"] == "powerio.parse"
    assert parsed["transport"] == "module"
    assert parsed["json_format"] == "module"
    assert parsed["domain"] == "transmission"
    assert parsed["model"] == "balanced"
    assert "module_json" in parsed
    module = json.loads(parsed["module_json"])
    assert module["schema"] == "powerio.module"
    assert module["producer"]["version"] == powerio.__version__
    assert module["value"]["kind"] == "balanced_network"

    module_json = parsed["module_json"]
    assert server.summarize(json=module_json)["elements"]["buses"] == 9
    assert server.summarize(module_json=module_json)["elements"]["branches"] == 9

    matrix = server.calc_matrix("bprime", json=module_json)
    assert matrix["kind"] == "bprime"
    assert matrix["shape"] == [9, 9]

    normalized = server.to_normalized(module_json=module_json)
    assert normalized["domain"] == "transmission"
    assert normalized["summary"]["elements"]["buses"] == 9

    out = tmp_path / "case9.m"
    _emit_to(destination=str(out), module_json=module_json)
    assert _parse(out, value_type=powerio.BalancedNetwork).value.n_buses == 9

    module_path = tmp_path / "case9.pio.json"
    module_path.write_text(module_json)
    assert (
        server.summarize(path=str(module_path), from_format="module")["elements"][
            "buses"
        ]
        == 9
    )


def test_module_transport_routes_distribution_by_model_kind():
    parsed = server.parse(path=str(DSS), transport="module")
    module = json.loads(parsed["module_json"])
    assert module["schema"] == "powerio.module"
    assert module["value"]["kind"] == "multiconductor_network"

    summary = server.summarize(json=parsed["module_json"])
    assert summary["domain"] == "distribution"
    assert summary["model"] == "multiconductor"
    assert summary["elements"]["buses"] > 0


def test_module_diagnostics():
    parsed = server.parse(path=str(DATA / "case9.m"), transport="module")
    diag = server.diagnostics(parsed["module_json"])
    assert diag["schema"] == "powerio.diagnostics"
    assert diag["model_kind"] == "balanced"
    assert diag["summary"]["status"] in {"ok", "info", "warning", "error", "fatal"}
    assert isinstance(diag["summary"]["text"], str)
    assert isinstance(diag["diagnostics"], list)

    verbose = server.diagnostics(parsed["module_json"], verbose=True)
    assert verbose["diagnostics"] == json.loads(parsed["module_json"]).get(
        "diagnostics", []
    )


def test_pre_0_9_package_is_recognized_and_rejected_by_version():
    """A package written before 0.9.0 states `schema_version`, so it has to be
    recognized as a stored document before the version gate can name the
    problem: the pre 0.9 lineage is refused, never silently misread."""
    old = json.dumps(
        {
            "schema_version": "0.2.1",
            "model_kind": "balanced",
            "model": {"kind": "balanced"},
        }
    )
    assert server._looks_like_stored_json(old)
    with pytest.raises(ValueError) as excinfo:
        server.diagnostics(old)
    message = str(excinfo.value)
    assert "powerio_version" in message
    assert "0.9" in message


def test_removed_transport_spellings_are_refused():
    for spelling in (
        "package",
        "pio-package",
        "pio_package",
        "pio",
        "pio-json",
        "pio_json",
        "powerio",
        "powerio-json",
        "powerio_json",
        "json",
        "bmopf",
        "bmopf_json",
        "model_json",
        "MODEL-JSON",
        " model-json ",
    ):
        with pytest.raises(ValueError, match=r"json_format.*must be `module`"):
            server.summarize(json="{}", json_format=spelling)
    stored = _emit_module(powerio.parse_file(DATA / "case9.m"))
    with pytest.raises(ValueError, match=r"json_format.*must be `module`"):
        server.summarize(json=stored, json_format="pio-json")
    with pytest.raises(ValueError, match="unknown or unsupported case format"):
        server.summarize(content="{}", from_format="package")


def test_minimal_bmopf_json_routes_without_format(tmp_path):
    parsed = server.parse(content=MINIMAL_BMOPF)
    assert parsed["domain"] == "distribution"
    assert parsed["model"] == "multiconductor"
    assert parsed["json_format"] == "bmopf-json"
    assert parsed["summary"]["elements"]["buses"] == 1

    s = server.summarize(content=MINIMAL_BMOPF)
    assert s["domain"] == "distribution"

    out = tmp_path / "minimal.json"
    _emit_to(format="bmopf-json", destination=str(out), json=MINIMAL_BMOPF)
    assert json.loads(out.read_text())["bus"]["a"]["terminal_names"] == ["1"]


def test_powermodels_json_still_routes_as_transmission():
    module = powerio.parse_file(DATA / "case9.m")
    pm = module.emit("powermodels-json").text
    parsed = server.parse(content=pm)
    assert parsed["domain"] == "transmission"
    assert parsed["json_format"] == "model-json"
    assert parsed["summary"]["elements"]["buses"] == 9

    packaged = server.parse(content=pm, transport="module")
    assert packaged["domain"] == "transmission"
    assert json.loads(packaged["module_json"])["value"]["kind"] == "balanced_network"
    assert packaged["summary"]["elements"]["buses"] == 9


def test_normalize_rejects_distribution():
    with pytest.raises(ValueError, match="not defined for distribution"):
        server._to_normalized_tool(path=str(DSS))


def test_normalize_payload_has_schema_marker():
    norm = server._to_normalized_tool(path=str(DATA / "case9.m"))
    assert norm["schema"] == "powerio.to_normalized"
    assert norm["powerio_version"] == powerio.__version__
    assert norm["domain"] == "transmission"
    assert norm["model"] == "balanced"


def test_parse_reads_pypsa_folder(tmp_path):
    folder = tmp_path / "case9-pypsa"
    powerio.parse_file(DATA / "case9.m").emit("pypsa-csv", folder)

    parsed = server.parse(path=str(folder))
    assert parsed["summary"]["domain"] == "transmission"
    assert parsed["summary"]["elements"]["buses"] == 9


@gridfm_only
def test_gridfm_routes_through_generic_verbs(tmp_path):
    out_dir = tmp_path / "gfm"
    write = server._emit_tool(
        format="gridfm",
        destination=str(out_dir),
        path=str(DATA / "case9.m"),
    )
    assert write["files"]

    parsed = server.parse(
        path=str(out_dir), from_format="gridfm", options={"scenario": 0}
    )
    assert parsed["summary"]["domain"] == "transmission"
    assert parsed["summary"]["elements"]["buses"] == 9

    converted = server._emit_tool(
        format="matpower", path=str(out_dir), from_format="gridfm"
    )
    assert "mpc.bus" in converted["text"]
    assert converted["schema"] == "powerio.emit"
    assert write["schema"] == "powerio.emit"
    assert converted["powerio_version"] == powerio.__version__


def test_matrix_kinds_aliases_and_errors():
    m = server._calc_matrix_tool("b", path=str(DATA / "case9.m"))
    assert m["schema"] == "powerio.calc_matrix"
    assert m["powerio_version"] == powerio.__version__
    assert m["domain"] == "transmission"
    assert m["model"] == "balanced"
    assert m["source_format"] == "matpower"
    assert m["diagnostics"] == []
    assert m["kind"] == "bprime"
    assert m["shape"] == [9, 9]
    assert type(m["data"][0]) is float and type(m["row"][0]) is int

    for alias, canonical in (
        ("b2", "bdoubleprime"),
        ("g", "ybus_real"),
        ("negB", "ybus_imag"),
        ("adj", "adjacency"),
        ("ptdf", "ptdf"),
        ("lodf", "lodf"),
        ("laplacian", "laplacian"),
        ("lacpf", "lacpf"),
    ):
        assert (
            server._calc_matrix_tool(alias, path=str(DATA / "case9.m"))["kind"]
            == canonical
        )

    with pytest.raises(ValueError, match="bprime"):
        server._calc_matrix_tool("nope", path=str(DATA / "case9.m"))
    with pytest.raises(ValueError, match="transmission"):
        server._calc_matrix_tool("b", path=str(DSS))


def test_bad_json_transport_leads_with_the_diagnostic_code():
    for bad in ("{}", "[]", "null", '{"buses": "nope"}'):
        with pytest.raises(ValueError, match=r"^PARSE\.SOURCE\.MALFORMED: "):
            server._calc_matrix_tool("bprime", json=bad, json_format="model-json")


def test_parse_failures_carry_the_code_and_the_tools_lead_with_it():
    with pytest.raises(powerio.PowerIOError) as native:
        _parse(("mpc.bus = [").encode(), "matpower", value_type=powerio.BalancedNetwork)
    assert native.value.code == "PARSE.MATPOWER.MALFORMED"
    with pytest.raises(ValueError) as mapped:
        server.summarize(content="mpc.bus = [", from_format="matpower")
    text = str(mapped.value)
    assert text.startswith("PARSE.MATPOWER.MALFORMED: ")
    # The Rust message already leads with the code; the tool wrapper must not
    # prefix it a second time.
    assert text.count("PARSE.MATPOWER.MALFORMED") == 1


def test_unknown_format_code_is_not_doubled():
    with pytest.raises(powerio.PowerIOError) as native:
        _parse(str(DATA / "case9.m"), "not-a-real-format")
    assert native.value.code == "REQUEST.FORMAT.UNKNOWN"
    with pytest.raises(ValueError) as mapped:
        server.summarize(path=str(DATA / "case9.m"), from_format="not-a-real-format")
    text = str(mapped.value)
    assert text.startswith("REQUEST.FORMAT.UNKNOWN: ")
    assert text.count("REQUEST.FORMAT.UNKNOWN") == 1


def test_empty_transport_text_means_unset():
    with pytest.raises(ValueError, match="provide exactly one"):
        server._summarize_tool()
    assert server._summarize_tool(path=str(DATA / "case9.m"))["elements"]["buses"] == 9


def test_emit_returns_text_or_writes_a_destination(tmp_path):
    text = server._emit_tool(format="psse", path=str(DATA / "case9.m"))
    assert text["schema"] == "powerio.emit"
    assert text["text"].lstrip().startswith("0,")

    destination = tmp_path / "case9.raw"
    written = server._emit_tool(
        format="psse",
        destination=str(destination),
        path=str(DATA / "case9.m"),
    )
    assert written["schema"] == "powerio.emit"
    assert written["path"] == str(destination)
    assert destination.read_text().lstrip().startswith("0,")


def test_emit_text_folder_and_overwrite(tmp_path):
    out = tmp_path / "case9.json"
    r = _emit_to(
        format="powermodels-json", destination=str(out), path=str(DATA / "case9.m")
    )
    assert r["schema"] == "powerio.emit"
    assert r["powerio_version"] == powerio.__version__
    assert r["path"] == str(out)
    assert r["bytes_written"] == out.stat().st_size
    with pytest.raises(ValueError):
        _emit_to(
            format="powermodels-json",
            destination=str(out),
            path=str(DATA / "case9.m"),
        )
    _emit_to(
        format="matpower",
        destination=str(out),
        path=str(DATA / "case9.m"),
        overwrite=True,
    )

    folder = tmp_path / "pypsa"
    w = _emit_to(
        format="pypsa-csv", destination=str(folder), path=str(DATA / "case9.m")
    )
    assert w["files"] and (folder / "buses.csv").exists()


def test_emit_requires_format():
    with pytest.raises(ValueError, match="format"):
        _emit_text(path=str(DATA / "case9.m"))


def test_emit_infers_unambiguous_output_format(tmp_path):
    raw = tmp_path / "case9.raw"
    _emit_to(destination=str(raw), path=str(DATA / "case9.m"))
    assert raw.read_text().lstrip().startswith("0,")

    with pytest.raises(ValueError, match="format"):
        _emit_to(destination=str(tmp_path / "case9.json"), path=str(DATA / "case9.m"))


def test_file_uri_paths_are_accepted(tmp_path):
    source_uri = (DATA / "case9.m").as_uri()
    assert server.summarize(path=source_uri)["elements"]["buses"] == 9

    out = tmp_path / "case9.json"
    _emit_to(format="powermodels-json", destination=out.as_uri(), path=source_uri)
    assert json.loads(out.read_text())["name"] == "case9"


def test_file_uri_decoding_preserves_windows_drive_letters():
    def text(path):
        return str(path).replace("\\", "/")

    assert (
        text(
            server._decode_local_path("file:///C:/Users/Sam/case%209.m", purpose="path")
        )
        == "C:/Users/Sam/case 9.m"
    )
    assert (
        text(
            server._decode_local_path(
                "file://localhost/D:/grid/case.raw", purpose="path"
            )
        )
        == "D:/grid/case.raw"
    )
    with pytest.raises(ValueError, match="must be local"):
        server._decode_local_path("file://server/share/case.raw", purpose="path")


def test_mcp_allowed_roots_restrict_filesystem_paths(monkeypatch, tmp_path):
    local_case = tmp_path / "case9.m"
    local_case.write_text((DATA / "case9.m").read_text())
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOTS", str(tmp_path))

    assert server.summarize(path=str(local_case))["elements"]["buses"] == 9
    with pytest.raises(ValueError, match="outside allowed MCP roots"):
        server.summarize(path=str(DATA / "case9.m"))


@pytest.mark.skipif(os.name == "nt", reason="POSIX symlink semantics")
def test_mcp_refuses_pypsa_child_symlink_escape(monkeypatch, tmp_path):
    root = tmp_path / "allowed"
    folder = root / "pypsa"
    outside = tmp_path / "outside"
    root.mkdir()
    outside.mkdir()
    powerio.parse_file(DATA / "case9.m").emit("pypsa-csv", folder)
    escaped = outside / "buses.csv"
    escaped.write_text((folder / "buses.csv").read_text())
    (folder / "buses.csv").unlink()
    os.symlink(escaped, folder / "buses.csv")
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOTS", str(root))

    with pytest.raises(ValueError, match="outside its allowed MCP root"):
        server.summarize(path=str(folder), from_format="pypsa-csv")


@pytest.mark.skipif(os.name == "nt", reason="POSIX symlink semantics")
def test_mcp_preflights_a_directory_before_format_detection(monkeypatch, tmp_path):
    root = tmp_path / "allowed"
    folder = root / "dataset" / "raw"
    outside = tmp_path / "outside"
    folder.mkdir(parents=True)
    outside.mkdir()
    parquet = outside / "bus_data.parquet"
    parquet.write_bytes(b"not parquet")
    os.symlink(parquet, folder / "bus_data.parquet")
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOTS", str(root))

    with pytest.raises(ValueError, match="outside its allowed MCP root"):
        server.summarize(path=str(root / "dataset"))


@pytest.mark.parametrize("name", ["POWERIO_MCP_ROOT", "POWERIO_MCP_ALLOWED_ROOT"])
def test_mcp_tools_honour_the_legacy_single_root_variables(monkeypatch, tmp_path, name):
    local_case = tmp_path / "case9.m"
    local_case.write_text((DATA / "case9.m").read_text())
    monkeypatch.setenv(name, str(tmp_path))

    assert server.summarize(path=str(local_case))["elements"]["buses"] == 9
    with pytest.raises(ValueError, match="outside allowed MCP roots"):
        server.summarize(path=str(DATA / "case9.m"))


@pytest.mark.skipif(os.name == "nt", reason="POSIX symlink semantics")
def test_mcp_write_refuses_symlink_escape_from_allowed_root(monkeypatch, tmp_path):
    # A dangling symlink named as the output file, sitting inside an allowed
    # root but pointing outside it, must not let an emission escape the sandbox:
    # `path.exists()` is False for the dangling link, and joining its name onto
    # the resolved parent would leave the link unresolved, so the containment
    # check has to follow the final symlink itself.
    root = tmp_path / "allowed"
    root.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    escape = root / "escape.json"
    os.symlink(outside / "leaked.json", escape)
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOTS", str(root))

    with pytest.raises(ValueError, match="outside allowed MCP roots"):
        _emit_to(format="bmopf-json", destination=str(escape), json=MINIMAL_BMOPF)
    assert not (outside / "leaked.json").exists()

    # A genuine new file inside the root still writes.
    _emit_to(format="bmopf-json", destination=str(root / "ok.json"), json=MINIMAL_BMOPF)
    assert (root / "ok.json").exists()


@pytest.mark.skipif(os.name == "nt", reason="POSIX symlink semantics")
def test_directory_emit_refuses_symlink_escape_at_a_child_name(monkeypatch, tmp_path):
    # The folder writers create children the single file containment check
    # never saw; every installed file must pass the same `for_write` resolution.
    root = tmp_path / "allowed"
    out = root / "pypsa"
    out.mkdir(parents=True)
    outside = tmp_path / "outside"
    outside.mkdir()
    os.symlink(outside / "leaked.csv", out / "buses.csv")
    case = root / "case9.m"
    case.write_text((DATA / "case9.m").read_text())
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOTS", str(root))

    with pytest.raises(ValueError, match="outside allowed MCP roots"):
        _emit_to(
            format="pypsa-csv", destination=str(out), path=str(case), overwrite=True
        )
    assert not (outside / "leaked.csv").exists()
    # Refusal happens before anything is installed.
    assert not (out / "network.csv").exists()


def test_directory_emit_honours_overwrite(tmp_path):
    folder = tmp_path / "pypsa"
    _emit_to(format="pypsa-csv", destination=str(folder), path=str(DATA / "case9.m"))
    assert (folder / "buses.csv").exists()
    with pytest.raises(ValueError, match="refusing to overwrite"):
        _emit_to(
            format="pypsa-csv", destination=str(folder), path=str(DATA / "case9.m")
        )
    r = _emit_to(
        format="pypsa-csv",
        destination=str(folder),
        path=str(DATA / "case9.m"),
        overwrite=True,
    )
    # Writer-reported paths are the installed location, never the staging dir.
    assert r["dir"] == str(folder)
    assert all(f.startswith(str(folder)) for f in r["files"])
    assert not [p for p in folder.parent.iterdir() if p.name.startswith("pypsa.")]


@gridfm_only
def test_gridfm_emit_honours_overwrite(tmp_path):
    out_dir = tmp_path / "gfm"
    first = _emit_to(
        format="gridfm", destination=str(out_dir), path=str(DATA / "case9.m")
    )
    assert first["files"] and all(f.startswith(str(out_dir)) for f in first["files"])
    with pytest.raises(ValueError, match="refusing to overwrite"):
        _emit_to(format="gridfm", destination=str(out_dir), path=str(DATA / "case9.m"))
    _emit_to(
        format="gridfm",
        destination=str(out_dir),
        path=str(DATA / "case9.m"),
        overwrite=True,
    )


def test_display_decodes_powerworld_pwd():
    d = server.display(str(PWD))
    assert d["schema"] == "powerio.display"
    assert d["powerio_version"] == powerio.__version__
    assert d["domain"] == "display"
    assert d["model"] == "display"
    assert d["source_format"] == "powerworld-pwd"
    assert d["canvas"]["width"] > 0
    assert d["substations"]
    assert {"number", "name", "x", "y"} <= set(d["substations"][0])


def test_display_errors_map_cleanly(tmp_path):
    with pytest.raises(ValueError):
        server.display(str(tmp_path / "nope.pwd"))
    bad = tmp_path / "bad.pwd"
    bad.write_bytes(b"not a powerworld display")
    with pytest.raises(ValueError):
        server.display(str(bad))


def test_compatibility_callables_are_removed():
    for name in (
        "summary",
        "convert",
        "save",
        "normalize",
        "matrix",
        "compute_matrix",
        "case_summary",
        "write_pypsa_csv_folder",
    ):
        assert not hasattr(server, name)


def _split_dss_case(tmp_path):
    """A dss case split across a feeder directory and a shared sibling."""
    root = tmp_path / "root"
    (root / "feeder").mkdir(parents=True)
    (root / "shared").mkdir()
    (root / "feeder" / "f.dss").write_text(
        "New Circuit.split basekv=12.47 pu=1 phases=3 bus1=a\n"
        "Redirect ../shared/linecodes.dss\n"
        "New Line.l1 bus1=a.1.2.3 bus2=b.1.2.3 phases=3 linecode=lc1"
        " length=1 units=km\n"
    )
    (root / "shared" / "linecodes.dss").write_text(
        "New Linecode.lc1 nphases=3 r1=0.1 x1=0.2\n"
    )
    return root


def _diagnostic_texts(diagnostics: list) -> "list[str]":
    """Each structured diagnostic record (`code`/`severity`/`message`/`target`)
    as one searchable string, for tests that check substrings a rendered
    `CODE: message` line used to carry directly."""
    return [
        ": ".join(part for part in (item.get("code"), item.get("message")) if part)
        for item in diagnostics
    ]


def test_mcp_parse_widens_dss_includes_to_the_allowed_root(monkeypatch, tmp_path):
    root = _split_dss_case(tmp_path)
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOTS", str(root))

    parsed = server.parse(path=str(root / "feeder" / "f.dss"))
    assert parsed["domain"] == "distribution"
    texts = _diagnostic_texts(parsed["diagnostics"])
    assert not any("INCLUDE_REFUSED" in text for text in texts), texts
    assert parsed["summary"]["elements"]["lines"] == 1


def test_mcp_parse_still_refuses_includes_outside_the_allowed_root(
    monkeypatch, tmp_path
):
    root = _split_dss_case(tmp_path)
    (tmp_path / "secret.dss").write_text("New Line.leaked bus1=x bus2=y\n")
    deck = root / "feeder" / "f.dss"
    deck.write_text(
        deck.read_text().replace("../shared/linecodes.dss", "../../secret.dss")
    )
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOTS", str(root))

    parsed = server.parse(path=str(deck))
    texts = _diagnostic_texts(parsed["diagnostics"])
    assert any("escapes the include root" in text for text in texts), texts
    doc = json.loads(parsed["json"])
    assert "leaked" not in json.dumps(doc)


# ---- typed state selection and the balanced lowering over MCP ----------------

LOWERABLE_DSS = """! Three phase feeder the balanced lowering supports.
Clear
Set DefaultBaseFrequency=60
New Circuit.feeder basekv=0.416 pu=1.0 phases=3 bus1=sourcebus MVAsc3=2000 MVAsc1=2100
New Linecode.lc3 nphases=3 basefreq=60 units=km
~ rmatrix = (0.211 | 0.049 0.211 | 0.049 0.049 0.211)
~ xmatrix = (0.747 | 0.673 0.747 | 0.651 0.673 0.747)
~ cmatrix = (10.0 | 0.0 10.0 | 0.0 0.0 10.0)
~ normamps=185
New Line.l1 bus1=sourcebus.1.2.3 bus2=loadbus.1.2.3 phases=3 linecode=lc3 length=0.4 units=km
New Load.la bus1=loadbus.1.2.3 phases=3 conn=wye kv=0.416 kw=24 pf=0.95 model=1
"""


def _legacy_package_doc(**extra) -> dict:
    """A released 0.9 package document, the lineage the stored reader upgrades."""
    network = json.loads(
        _parse(
            str(DATA / "case9.m"), value_type=powerio.BalancedNetwork
        ).value.to_json()
    )
    return {
        "powerio_version": "0.9.0",
        "producer": {"tool": "powerio", "version": "0.9.0"},
        "model_kind": "balanced",
        "model": {"kind": "balanced", "balanced_network": network},
        "origin": {"kind": "in_memory"},
        "validation": {"status": "ok", "counts": {}},
        **extra,
    }


def _series_module_json() -> str:
    legacy = _legacy_package_doc(
        operating_points={
            "time_axis": {
                "periods": 2,
                "duration_hours": [1.0, 1.0],
                "labels": ["h0", "h1"],
            },
            "points": [
                {"index": 0, "updates": []},
                {
                    "index": 1,
                    "updates": [
                        {
                            "element": {
                                "table": "generators",
                                "source_uid": "generators:0",
                            },
                            "fields": {"pg": 95.0},
                        }
                    ],
                },
            ],
        }
    )
    return _emit_module(_parse_module(json.dumps(legacy)))


def test_list_states_selection_and_export():
    module_json = _series_module_json()

    inventory = server._list_states_tool(module_json=module_json)
    assert inventory["kind"] == "balanced_operating_point_time_series"
    assert inventory["keyed_by"] == "time_position"
    assert [p["label"] for p in inventory["time_points"]] == ["h0", "h1"]
    selected = server._inspect_state_tool(module_json=module_json, time_position=1)
    assert selected["schema"] == "powerio.inspect_state"
    assert selected["selected"]["item"] == "balanced_operating_point"
    assert "generator_active_power" in selected["selected"]["stated_quantities"]

    exported = server._export_state_tool(module_json=module_json, time_position=1)
    assert exported["kind"] == "balanced_network"
    doc = json.loads(exported["module_json"])
    assert doc["schema"] == "powerio.module" and doc["version"] == 1
    # The exported static module is accepted by summary, matrix, conversion,
    # and diagnostics operations.
    summary = server._summarize_tool(module_json=exported["module_json"])
    assert summary["domain"] == "transmission"
    assert summary["elements"]["buses"] == 9
    matrix = server._calc_matrix_tool("ybus_real", module_json=exported["module_json"])
    assert matrix["shape"] == [9, 9]
    converted = _emit_text(format="matpower", module_json=exported["module_json"])
    assert "mpc.baseMVA" in converted["text"]
    inspected = server._inspect_tool(module_json=exported["module_json"])
    assert inspected["kind"] == "balanced_network"
    assert "select_state" not in inspected["operations"]


def test_selection_refusals_carry_codes():
    module_json = _series_module_json()
    with pytest.raises(ValueError, match="REQUEST.STATE.OUT_OF_RANGE"):
        server._inspect_state_tool(module_json=module_json, time_position=9)
    with pytest.raises(ValueError, match="REQUEST.STATE.WRONG_SELECTOR"):
        server._inspect_state_tool(module_json=module_json, scenario="base")
    static_module = _emit_module(powerio.parse_file(DATA / "case9.m"))
    with pytest.raises(ValueError, match="REQUEST.STATE.NOT_A_COLLECTION"):
        server._list_states_tool(module_json=static_module)
    with pytest.raises(ValueError, match="exactly one of time_position"):
        server._inspect_state_tool(module_json=module_json)


def test_inspect_discovers_operations():
    inspected = server._inspect_tool(content=LOWERABLE_DSS, from_format="dss")
    assert inspected["kind"] == "multiconductor_network"
    assert "to_balanced" in inspected["operations"]
    series = server._inspect_tool(module_json=_series_module_json())
    assert "inspect_state" in series["operations"]
    assert "select_state" not in series["operations"]
    assert "export_state" in series["operations"]


def test_to_balanced_report_and_transform():
    readiness = server._to_balanced_report_tool(
        content=LOWERABLE_DSS, from_format="dss"
    )
    assert readiness["schema"] == "powerio.to_balanced_report"
    assert readiness["ready"] is True

    lowered = server._to_balanced_tool(content=LOWERABLE_DSS, from_format="dss")
    assert lowered["kind"] == "balanced_network"
    doc = json.loads(lowered["module_json"])
    assert doc["schema"] == "powerio.module"
    history = doc.get("history", [])
    assert any(
        entry["name"] == "to_balanced" and entry["kind"] == "transform"
        for entry in history
    )
    # The returned module is accepted by summary and matrix operations.
    summary = server._summarize_tool(module_json=lowered["module_json"])
    assert summary["domain"] == "transmission"
    matrix = server._calc_matrix_tool("ybus_real", module_json=lowered["module_json"])
    assert matrix["shape"][0] == summary["elements"]["buses"]


def test_to_balanced_refuses_unsupported_input_structured():
    with pytest.raises(ValueError) as excinfo:
        server._to_balanced_tool(path=str(DSS))
    message = str(excinfo.value)
    assert "TRANSFORM.MULTI_TO_BALANCED" in message
    # The structured diagnostics ride along as JSON.
    assert "[" in message and "code" in message


TRANSFORMER_DSS = """! Two winding transformer feeder for name normalization.
Clear
Set DefaultBaseFrequency=60
New Circuit.tiny basekv=12.47 pu=1.0 phases=3 bus1=src MVAsc3=2000 MVAsc1=2100
New Transformer.t1 phases=3 windings=2 buses=(src, sec) conns=(delta, wye) kvs=(12.47, 0.416) kvas=(500, 300) %Rs=(0.5, 0.5) xhl=6
New Load.l1 bus1=sec phases=3 conn=wye kv=0.416 kw=90 pf=0.95 model=1
Set VoltageBases=[12.47, 0.416]
"""


def test_hostile_element_names_lower_with_normalized_history(tmp_path):
    """A module whose transformer name carries a NUL byte and overflows the
    identifier bound still lowers through the tool; the recorded history
    notes are normalized rather than dropped or crashing the pass."""
    feeder = tmp_path / "tiny.dss"
    feeder.write_text(TRANSFORMER_DSS)
    module = powerio.parse_file(feeder, "dss")
    doc = json.loads(_emit_module(module))
    hostile = "t\u0000evil" + "x" * (70_000)
    doc["value"]["data"]["transformers"][0]["name"] = hostile
    lowered = server._to_balanced_tool(module_json=json.dumps(doc))
    assert lowered["kind"] == "balanced_network"
    history = json.loads(lowered["module_json"]).get("history", [])
    entry = next(e for e in history if e["name"] == "to_balanced")
    for note in entry.get("assumptions", []) + entry.get("losses", []):
        assert note
        assert "\u0000" not in note and "\x00" not in note
        assert len(note.encode()) <= 65_536
    assert any("[truncated]" in note for note in entry.get("assumptions", []))


def test_wrong_kind_lowering_is_refused_by_name():
    with pytest.raises(ValueError, match="WRONG_MODEL_KIND"):
        server._to_balanced_tool(path=str(DATA / "case9.m"))


def test_about_states_versions_and_tools():
    about = server._about_tool()
    assert about["powerio_version"] == powerio.versions()["powerio_version"]
    assert about["module_schema"] == {"name": "powerio.module", "version": 1}
    assert "dc_data" not in about["tools"]
    assert "convert" not in about["tools"]


# ---- module diagnostics reach every MCP response -----------------------------


def _balanced_module_with_diagnostics(diagnostics: list) -> str:
    """A static `balanced_network` module document (exported from the time
    series fixture) with the given diagnostics injected onto it."""
    module = _parse_module(_series_module_json())
    exported = module.export_state(time_position=1)
    doc = json.loads(_emit_module(exported))
    doc["diagnostics"] = diagnostics
    return json.dumps(doc)


def test_module_diagnostics_reach_summary_and_emission():
    error = {
        "id": "d-error",
        "severity": "error",
        "code": "M.E.1",
        "message": "an error finding",
    }
    note = {
        "id": "d-note",
        "severity": "note",
        "code": "M.N.1",
        "message": "a note finding",
    }

    module_doc = _balanced_module_with_diagnostics([error, note])
    summary = server._summarize_tool(module_json=module_doc)
    assert summary["diagnostics"] == [
        {
            "code": "M.E.1",
            "severity": "error",
            "message": "an error finding",
            "target": None,
            "id": "d-error",
        }
    ]

    # The note severity finding is filtered out, not merely unmatched.
    converted = _emit_text(format="matpower", module_json=module_doc)
    assert converted["diagnostics"] == [
        {
            "code": "M.E.1",
            "severity": "error",
            "message": "an error finding",
            "target": None,
            "id": "d-error",
        }
    ]

    note_only_doc = _balanced_module_with_diagnostics([note])
    assert server._summarize_tool(module_json=note_only_doc)["diagnostics"] == []

    # The same finding, carried by the older released package transport,
    # narrows to the same record shape: the two input forms agree on
    # code/severity/message. The legacy row itself carries no `id`.
    package = _legacy_package_doc(
        diagnostics=[
            {
                "code": error["code"],
                "severity": error["severity"],
                "message": error["message"],
            }
        ]
    )
    package_summary = server._summarize_tool(module_json=json.dumps(package))
    assert package_summary["diagnostics"] == [
        {
            "code": "M.E.1",
            "severity": "error",
            "message": "an error finding",
            "target": None,
        }
    ]


def test_multiconductor_module_diagnostics_survive_in_order():
    """Diagnostics carried by a stored multiconductor module reach the MCP
    diagnostics in source order, including one whose span references the
    module's own declared source and a trailing entry after it."""
    module = powerio.parse_text(LOWERABLE_DSS, name="lowerable.dss", format="dss")
    doc = json.loads(_emit_module(module))
    source_id = doc["sources"][0]["id"]
    doc["diagnostics"] = [
        {"id": "d0", "severity": "warning", "code": "A.B.C", "message": "no span here"},
        {
            "id": "d1",
            "severity": "error",
            "code": "D.E.F",
            "message": "in range span",
            "spans": [{"source": source_id, "byte_start": 0, "byte_end": 10}],
        },
        {"id": "d2", "severity": "error", "code": "G.H.I", "message": "trailing error"},
    ]
    module_doc = json.dumps(doc)

    summary = server._summarize_tool(module_json=module_doc)
    assert summary["diagnostics"] == [
        {
            "code": "A.B.C",
            "severity": "warning",
            "message": "no span here",
            "target": None,
            "id": "d0",
        },
        {
            "code": "D.E.F",
            "severity": "error",
            "message": "in range span",
            "target": None,
            "id": "d1",
            "spans": [{"source": source_id, "byte_start": 0, "byte_end": 10}],
        },
        {
            "code": "G.H.I",
            "severity": "error",
            "message": "trailing error",
            "target": None,
            "id": "d2",
        },
    ]


def test_parse_diagnostics_are_structured_records_on_every_transport():
    """A PSS/E read reports six findings. The path transport used to publish
    them as six plain rendered strings with no way back to code/severity/
    target, while the package transport (which reads back through a stored
    module) already published six dicts. Both now publish the identical
    structured list."""
    via_path = server.parse(path=str(PSSE_CASE5))
    via_package = server.parse(path=str(PSSE_CASE5), transport="module")

    for parsed in (via_path, via_package):
        diagnostics = parsed["diagnostics"]
        assert len(diagnostics) == 6, diagnostics
        for record in diagnostics:
            assert isinstance(record, dict), diagnostics
            assert record.keys() >= {"code", "severity", "message"}
            assert record["code"]
            assert record["severity"] in ("error", "warning", "remark", "note")
            assert record["message"]

    # The module transport round trips through the writer, which assigns
    # ids (d0, d1, ...) to records that lack them; the direct path's records
    # have none. Identical otherwise.
    def _without_ids(records):
        return [{k: v for k, v in r.items() if k != "id"} for r in records]

    assert _without_ids(via_path["diagnostics"]) == _without_ids(
        via_package["diagnostics"]
    )
