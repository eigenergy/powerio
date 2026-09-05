"""The multiconductor value surface and canonical parse/emit path."""

import inspect
import io
import json
from pathlib import Path

import pytest

import powerio
from powerio import dist

DATA = Path(__file__).resolve().parents[2] / "tests" / "data" / "dist"
FOURWIRE = DATA / "micro" / "fourwire_linecode.dss"
LOWERABLE_DSS = """Clear
Set DefaultBaseFrequency=60
New Circuit.tiny basekv=12.47 pu=1.0 phases=3 bus1=src MVAsc3=2000 MVAsc1=2100
New Transformer.t1 phases=3 windings=2 buses=(src, sec) conns=(delta, wye) kvs=(12.47, 0.416) kvas=(500, 300) %Rs=(0.5, 0.5) xhl=6
New Load.l1 bus1=sec phases=3 conn=wye kv=0.416 kw=90 pf=0.95 model=1
Set VoltageBases=[12.47, 0.416]
"""


def _parse_file_value(path, format=None, *, include_root=None):
    assert include_root is None
    value = powerio.parse(path, format=format).value
    assert isinstance(value, dist.MulticonductorNetwork)
    return value


def _parse_text_value(text, format):
    value = powerio.parse(
        io.StringIO(text),
        name="fixture",
        format=format,
    ).value
    assert isinstance(value, dist.MulticonductorNetwork)
    return value


def _emit_value(value, format, destination=None):
    return powerio.emit(powerio.PioModule.from_value(value), format, destination)


def _emit_module(module):
    return powerio.serialize(module).text


def _parse_module(text):
    return powerio.deserialize(io.StringIO(text))


def test_parse_file_counts_and_source_format():
    case = _parse_file_value(FOURWIRE)
    assert case.source_format == "dss"
    assert case.n_buses > 0
    assert case.n_lines > 0
    assert not hasattr(case, "warnings")
    assert not hasattr(case, "diagnostics")


def test_complete_read_only_tables_and_power_system_counts():
    case = _parse_file_value(FOURWIRE)
    assert case.base_frequency == 60.0
    assert case.n_voltage_sources == len(case.voltage_sources)
    for removed in ("n_sources", "sources", "linecodes", "untyped"):
        assert not hasattr(case, removed)
    for table, count in [
        ("buses", "n_buses"),
        ("line_codes", "n_line_codes"),
        ("lines", "n_lines"),
        ("switches", "n_switches"),
        ("transformers", "n_transformers"),
        ("loads", "n_loads"),
        ("generators", "n_generators"),
        ("ibrs", "n_ibrs"),
        ("control_profiles", "n_control_profiles"),
        ("shunts", "n_shunts"),
        ("capacitors", "n_capacitors"),
        ("voltage_sources", "n_voltage_sources"),
        ("untyped_objects", "n_untyped_objects"),
    ]:
        rows = getattr(case, table)
        assert isinstance(rows, list), table
        assert len(rows) == getattr(case, count), table
    assert case.buses[0]["id"]
    assert case.line_codes[0]["name"]


def test_multiconductor_is_the_only_model_name():
    case = _parse_file_value(FOURWIRE)
    assert isinstance(case, dist.MulticonductorNetwork)
    assert powerio.MulticonductorNetwork is dist.MulticonductorNetwork
    assert "MulticonductorNetwork" in dist.__all__
    assert "DistCase" not in dist.__all__
    # The final surface has one multiconductor model name.
    assert not hasattr(dist, "DistNetwork")
    assert "DistNetwork" not in dist.__all__
    assert not hasattr(dist, "DistCase")


def test_same_format_write_echoes_source():
    case = _parse_file_value(FOURWIRE)
    conv = _emit_value(case, "dss")
    assert conv.text == FOURWIRE.read_text()
    assert conv.diagnostics == ()


def test_cross_format_writes():
    case = _parse_file_value(FOURWIRE)
    pmd = _emit_value(case, "pmd-json")
    assert json.loads(pmd.text)["data_model"] == "ENGINEERING"
    bmopf = _emit_value(case, "bmopf-json")
    assert "bus" in json.loads(bmopf.text)


def test_graph_projection():
    graph = _parse_file_value(FOURWIRE).to_graph()
    assert {bus["id"] for bus in graph["buses"]} == {"sourcebus", "loadbus"}
    source = next(bus for bus in graph["buses"] if bus["id"] == "sourcebus")
    assert source["has_source"] is True
    edge = next(edge for edge in graph["edges"] if edge["id"] == "l1")
    assert edge["kind"] == "line"
    assert edge["from"] == "sourcebus"
    assert edge["to"] == "loadbus"
    assert edge["n_phases"] == 4
    assert len(edge["conductors"]) == 4


def test_json_sniffing_round_trip(tmp_path):
    case = _parse_file_value(FOURWIRE)
    for fmt in ("pmd-json", "bmopf-json"):
        text = _emit_value(case, fmt).text
        p = tmp_path / f"case_{fmt}.json"
        p.write_text(text)
        module = powerio.parse(p)
        if fmt == "pmd-json":
            assert module.value.source_format == fmt
            assert module.value.n_buses == case.n_buses
        else:
            assert isinstance(module.value, powerio.MulticonductorNetwork)


def test_parse_text_and_parse_file_emit_the_same_result():
    text = FOURWIRE.read_text()
    via_str = _emit_value(_parse_text_value(text, "dss"), "pmd-json")
    via_file = _emit_value(_parse_file_value(FOURWIRE), "pmd-json")
    assert via_str.text == via_file.text
    assert isinstance(via_str, powerio.EmitResult)


def test_network_diagnostics_belong_only_to_the_module():
    network = _parse_file_value(FOURWIRE)
    assert not hasattr(network, "diagnostics")
    assert not hasattr(network, "warnings")


def test_top_level_parse_routes_distribution_inputs():
    text = FOURWIRE.read_text()
    via_str = powerio.emit(
        powerio.parse(io.StringIO(text), name="feeder.dss", format="dss"),
        "pmd-json",
    )
    via_file = powerio.emit(powerio.parse(FOURWIRE), "pmd-json")
    assert json.loads(via_str.text)["data_model"] == "ENGINEERING"
    assert via_str.text == via_file.text
    assert via_str.diagnostics == via_file.diagnostics


def test_pio_module_distribution_writer_infers_source_format(tmp_path):
    module = powerio.parse(FOURWIRE)
    assert json.loads(powerio.emit(module, "pmd-json").text)["data_model"] == "ENGINEERING"
    out = tmp_path / "echo"
    result = powerio.emit(module, "dss", out)
    assert result.text is None
    assert result.diagnostics == ()
    assert (out / "case.dss").read_bytes() == FOURWIRE.read_bytes()


def test_dist_emit_destination_writes_sidecars_beside_the_case(tmp_path):
    # A dss write that re-serializes (rather than echoing a retained source)
    # emits `Buscoords <name>` for a case with coordinates, and returns the
    # CSV as a sidecar. The sidecar must reach disk in the same artifact
    # directory as the case, or
    # OpenDSS cannot compile the result. `apply_geo_layer` drops the retained
    # source, so the write takes the re-serializing path.
    source = _parse_file_value(DATA / "opendss" / "ieee13" / "IEEE13Nodeckt.dss")
    layer = source.to_geo_layer()
    assert not hasattr(source, "geo_layer")
    placed, _ = source.apply_geo_layer(json.dumps(layer))
    memory = _emit_value(placed, "dss")
    assert memory.layout == "directory"
    assert any(artifact.name.endswith("/case.dss") for artifact in memory.artifacts)
    out = tmp_path / "ieee13"
    _emit_value(placed, "dss", out)
    text = (out / "case.dss").read_text()
    names = [
        line.split()[1]
        for line in text.splitlines()
        if line.lower().startswith("buscoords")
    ]
    assert names, "the write emitted no Buscoords directive"
    for name in names:
        assert (out / name).is_file(), f"{name} was not written beside the case"
        assert (out / name).read_text().strip(), f"{name} is empty"


def test_dist_emit_destination_refuses_existing_case_and_sidecar_entries(tmp_path):
    source = _parse_file_value(DATA / "opendss" / "ieee13" / "IEEE13Nodeckt.dss")
    placed, _ = source.apply_geo_layer(json.dumps(source.to_geo_layer()))

    # An existing entry at the output root refuses and keeps its bytes.
    blocked = tmp_path / "blocked"
    blocked.write_text("precious")
    with pytest.raises(Exception) as refusal:
        _emit_value(placed, "dss", blocked)
    assert "already exists" in str(refusal.value)
    assert blocked.read_text() == "precious"

    # The complete artifact inventory commits under one new root.
    ok = tmp_path / "ok"
    _emit_value(placed, "dss", ok)
    sidecar_names = [
        line.split()[1]
        for line in (ok / "case.dss").read_text().splitlines()
        if line.lower().startswith("buscoords")
    ]
    assert sidecar_names
    assert (ok / sidecar_names[0]).is_file()

    nested = tmp_path / "nested"
    nested.mkdir()
    precious = nested / "precious.txt"
    precious.write_text("precious")
    with pytest.raises(Exception) as refusal:
        _emit_value(placed, "dss", nested)
    assert "already exists" in str(refusal.value)
    assert precious.read_text() == "precious"
    assert not (nested / "case.dss").exists()


def test_lower_to_balanced_refusal_carries_code_and_diagnostics():
    # IEEE13Nodeckt is an unbalanced feeder (single- and two-phase laterals,
    # single-phase regulators, a wye-wye substation transformer): the
    # positive sequence projection refuses it outright rather than guess.
    module = powerio.parse(DATA / "opendss" / "ieee13" / "IEEE13Nodeckt.dss")
    report = module.to_balanced_report(base_mva=100.0)
    assert report["ready"] is False
    assert not hasattr(module, "to_balanced_inspect")
    with pytest.raises(powerio.PowerIODataError) as excinfo:
        module.to_balanced(base_mva=100.0)
    error = excinfo.value
    assert error.code
    assert error.diagnostics
    assert error.diagnostics[0]["code"] == error.code
    for diagnostic in error.diagnostics:
        assert diagnostic.keys() == {"code", "severity", "message", "target"}
        assert diagnostic["code"].startswith("TRANSFORM.")
        assert diagnostic["severity"] in ("error", "warning", "remark", "note")
        assert diagnostic["message"]
    # The refusal leaves the handle usable, still carrying its module.
    assert isinstance(module.value, dist.MulticonductorNetwork)


def test_pio_module_from_multiconductor_value_keeps_source_echo():
    source = DATA / "micro" / "xfmr_single_phase.dss"
    parsed = powerio.parse(source)
    wrapped = powerio.PioModule.from_value(parsed.value)
    assert isinstance(wrapped.value, dist.MulticonductorNetwork)
    assert wrapped.diagnostics == parsed.diagnostics
    assert powerio.emit(wrapped, "dss").text.encode() == source.read_bytes()


def test_successful_lowering_leaves_the_source_module_usable_and_records_intact():
    module = powerio.parse(io.StringIO(LOWERABLE_DSS), name="lowerable.dss", format="dss")
    doc = json.loads(_emit_module(module))
    doc["producer"] = {"name": "python-binding-test", "version": "1"}
    doc["history"] = [{"id": "before-lowering", "kind": "parse", "name": "test_parse"}]
    doc["extensions"] = {"org.example.test": {"kept": True}}
    module = _parse_module(json.dumps(doc))
    before = json.loads(_emit_module(module))

    lowered = module.to_balanced()

    # The native transform owns a record-complete sibling, not this handle.
    assert isinstance(module.value, dist.MulticonductorNetwork)
    assert json.loads(_emit_module(module)) == before
    assert module.value.n_buses > 0

    lowered_doc = json.loads(_emit_module(lowered))
    assert isinstance(lowered.value, powerio.BalancedNetwork)
    assert lowered_doc["producer"] == before["producer"]
    assert lowered_doc["extensions"] == before["extensions"]
    assert lowered_doc["history"][0] == before["history"][0]
    assert any(
        entry["name"] == "to_balanced"
        for entry in lowered_doc["history"]
    )


def test_module_emit_echoes_distribution_bytes(tmp_path):
    case = _parse_file_value(FOURWIRE)
    out = tmp_path / "echo"
    result = _emit_value(case, "dss", out)
    assert result.text is None
    assert result.diagnostics == ()
    assert (out / "case.dss").read_bytes() == FOURWIRE.read_bytes()


def test_parse_diagnostics_surface_on_module():
    module = powerio.parse(
        io.StringIO(
            "clear\n"
            "new circuit.w basekv=12.47 bus1=src\n"
            "new line.l1 bus1=src bus2=b2 length=1 units=furlong\n"
        ),
        name="warning.dss",
        format="dss",
    )
    assert any("furlong" in diagnostic.message for diagnostic in module.diagnostics)


def test_unknown_format_raises_value_error():
    with pytest.raises(powerio.PowerIOParseError, match="baseMVA"):
        _parse_text_value("clear\n", "matpower")
    case = _parse_file_value(FOURWIRE)
    with pytest.raises(
        ValueError, match="UNKNOWN_FORMAT|not a recognized target format"
    ):
        _emit_value(case, "matpower")


def test_malformed_json_raises_parse_error():
    with pytest.raises(powerio.PowerIOParseError):
        _parse_text_value("{not json", "bmopf-json")


def test_missing_file_raises_precise_oserror():
    # Io errors map to the precise OSError subclass with the path attached.
    with pytest.raises(FileNotFoundError) as exc:
        _parse_file_value(DATA / "does_not_exist.dss")
    assert exc.value.filename and "does_not_exist.dss" in str(exc.value.filename)


def test_emit_carries_parse_diagnostics():
    module = powerio.parse(
        io.StringIO(
            "clear\n"
            "new circuit.w basekv=12.47 bus1=src\n"
            "new line.l1 bus1=src bus2=b2 length=1 units=furlong\n"
        ),
        name="warning.dss",
        format="dss",
    )
    assert any("furlong" in diagnostic.message for diagnostic in module.diagnostics)


def test_bmopf_containing_data_model_string_routes_to_bmopf(tmp_path):
    # The sniff keys on a TOP LEVEL data_model key; a nested occurrence is
    # not the marker.
    case = _parse_file_value(FOURWIRE)
    text = _emit_value(case, "bmopf-json").text
    doc = json.loads(text)
    doc["bus"]["data_model"] = doc["bus"][next(iter(doc["bus"]))]
    p = tmp_path / "nested_marker.json"
    p.write_text(json.dumps(doc))
    assert isinstance(powerio.parse(p).value, powerio.MulticonductorNetwork)


def _split_case(tmp_path):
    """A case split across a feeder directory and a shared sibling."""
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


def test_parse_confines_includes_to_the_case_directory(tmp_path):
    root = _split_case(tmp_path)
    deck = root / "feeder" / "f.dss"
    confined = powerio.parse(deck)
    assert any(
        "escapes the case directory" in diagnostic.message
        for diagnostic in confined.diagnostics
    )
    assert "include_root" not in inspect.signature(powerio.parse).parameters


def test_source_energy_prices_and_draft_schema_digest_survive_ir():
    import hashlib

    document = {
        "meta": {"schema_version": "0.2.0"},
        "bus": {"b": {"terminal_names": ["a", "n"]}},
        "voltage_source": {"s": {
            "bus": "b", "terminal_map": ["a", "n"],
            "v_magnitude": [230, 0], "v_angle": [0, 0],
            "energy_cost_rate": [0.1],
        }},
    }
    module = powerio.parse(io.StringIO(json.dumps(document)), format="bmopf-json")
    restored = _parse_module(_emit_module(module))
    assert restored.value.voltage_sources[0]["energy_cost_rate"] == [0.1]
    output = json.loads(powerio.emit(restored, "bmopf-json@0.2.0").text)
    provenance = output["meta"]["provenance"]["powerio_bmopf"]
    schema = (DATA / "bmopf" / "bmopf-0.2.0.schema.json").read_bytes()
    assert provenance["schema_sha256"] == hashlib.sha256(schema).hexdigest()
    assert provenance["schema_status"] == "proposal"
    assert provenance["schema_commit"] in output["meta"]["$schema"]
    legacy = json.loads(powerio.emit(restored, "bmopf-json@0.1.0").text)
    assert "energy_cost_rate" not in legacy["voltage_source"]["s"]
    assert legacy["extras"]["voltage_source"]["s"]["energy_cost_rate"] == [0.1]
