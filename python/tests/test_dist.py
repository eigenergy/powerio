"""The powerio.dist surface: parse, echo, convert, warnings, errors."""

import json
import warnings
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


def test_parse_file_counts_and_source_format():
    case = dist.parse_file(FOURWIRE)
    assert case.source_format == "dss"
    assert case.n_buses > 0
    assert case.n_lines > 0
    assert isinstance(case.warnings, list)


def test_complete_read_only_tables_and_power_system_counts():
    case = dist.parse_file(FOURWIRE)
    assert case.base_frequency == 60.0
    assert case.n_voltage_sources == case.n_sources == len(case.voltage_sources)
    assert case.sources == case.voltage_sources
    assert case.linecodes == case.line_codes
    assert case.untyped == case.untyped_objects
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
    case = dist.parse_file(FOURWIRE)
    assert isinstance(case, dist.MulticonductorNetwork)
    assert "MulticonductorNetwork" in dist.__all__
    assert "DistCase" not in dist.__all__
    # DistNetwork is the 0.8 bridge alias: same object, DeprecationWarning,
    # gone at 1.0.0. DistCase stays removed.
    assert not hasattr(dist, "DistNetwork")
    assert "DistNetwork" not in dist.__all__
    assert not hasattr(dist, "DistCase")


def test_same_format_write_echoes_source():
    case = dist.parse_file(FOURWIRE)
    conv = case.to_format("dss")
    assert conv.text == FOURWIRE.read_text()
    assert conv.warnings == []


def test_cross_format_writes():
    case = dist.parse_file(FOURWIRE)
    pmd = case.to_format("pmd-json")
    assert json.loads(pmd.text)["data_model"] == "ENGINEERING"
    bmopf = case.to_format("bmopf-json")
    assert "bus" in json.loads(bmopf.text)


def test_graph_projection():
    graph = dist.parse_file(FOURWIRE).to_graph()
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
    case = dist.parse_file(FOURWIRE)
    for fmt in ("pmd-json", "bmopf-json"):
        text = case.to_format(fmt).text
        p = tmp_path / f"case_{fmt}.json"
        p.write_text(text)
        again = dist.parse_file(p)
        assert again.source_format == fmt
        assert again.n_buses == case.n_buses


def test_convert_str_and_convert_file():
    text = FOURWIRE.read_text()
    via_str = dist.convert_str(text, "pmd-json", "dss")
    via_file = dist.convert_file(FOURWIRE, "pmd-json")
    assert via_str.text == via_file.text
    assert isinstance(via_str, powerio.Conversion)


def test_top_level_conversion_routes_distribution_inputs():
    text = FOURWIRE.read_text()
    via_str = powerio.convert_str(text, "pmd-json", "dss")
    via_file = powerio.convert_file(FOURWIRE, "pmd-json")
    assert json.loads(via_str.text)["data_model"] == "ENGINEERING"
    assert via_str.text == via_file.text
    assert via_str.diagnostics is via_str.warnings


def test_pio_module_distribution_writer_infers_source_format(tmp_path):
    module = powerio.parse(FOURWIRE)
    assert json.loads(module.to_format("pmd-json").text)["data_model"] == "ENGINEERING"
    out = tmp_path / "echo.dss"
    assert module.write_file(out) == []
    assert out.read_bytes() == FOURWIRE.read_bytes()


def test_dist_write_file_writes_sidecars_beside_the_case(tmp_path):
    # A dss write that re-serializes (rather than echoing a retained source)
    # emits `Buscoords <name>` for a case with coordinates, and returns the
    # CSV as a sidecar. The sidecar must reach disk beside the case, or
    # OpenDSS cannot compile the result. `apply_geo_layer` drops the retained
    # source, so the write takes the re-serializing path.
    source = dist.parse_file(DATA / "opendss" / "ieee13" / "IEEE13Nodeckt.dss")
    placed, _ = source.apply_geo_layer(json.dumps(source.geo_layer()))
    out = tmp_path / "ieee13.dss"
    placed.write_file(out, "dss")
    text = out.read_text()
    names = [
        line.split()[1]
        for line in text.splitlines()
        if line.lower().startswith("buscoords")
    ]
    assert names, "the write emitted no Buscoords directive"
    for name in names:
        assert (tmp_path / name).is_file(), f"{name} was not written beside the case"
        assert (tmp_path / name).read_text().strip(), f"{name} is empty"


def test_dist_write_file_refuses_existing_case_and_sidecar_entries(tmp_path):
    source = dist.parse_file(DATA / "opendss" / "ieee13" / "IEEE13Nodeckt.dss")
    placed, _ = source.apply_geo_layer(json.dumps(source.geo_layer()))

    # An existing entry at the case path refuses and keeps its bytes.
    blocked = tmp_path / "blocked.dss"
    blocked.write_text("precious")
    with pytest.raises(Exception) as refusal:
        placed.write_file(blocked, "dss")
    assert "already exists" in str(refusal.value)
    assert blocked.read_text() == "precious"

    # An existing entry at a sidecar name refuses the whole write, keeps the
    # sidecar's bytes, and removes the case file this call created.
    ok = tmp_path / "ok.dss"
    placed.write_file(ok, "dss")
    sidecar_names = [
        line.split()[1]
        for line in ok.read_text().splitlines()
        if line.lower().startswith("buscoords")
    ]
    assert sidecar_names
    nested = tmp_path / "nested"
    nested.mkdir()
    (nested / sidecar_names[0]).write_text("precious sidecar")
    with pytest.raises(Exception) as refusal:
        placed.write_file(nested / "case.dss", "dss")
    assert "already exists" in str(refusal.value)
    assert (nested / sidecar_names[0]).read_text() == "precious sidecar"
    assert not (nested / "case.dss").exists()


def test_lower_to_balanced_refusal_carries_code_and_diagnostics():
    # IEEE13Nodeckt is an unbalanced feeder (single- and two-phase laterals,
    # single-phase regulators, a wye-wye substation transformer): the
    # positive sequence projection refuses it outright rather than guess.
    module = powerio.parse(DATA / "opendss" / "ieee13" / "IEEE13Nodeckt.dss")
    report = module.to_balanced_report(base_mva=100.0)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert module.to_balanced_inspect(base_mva=100.0) == report
    assert caught == []
    with pytest.raises(powerio.PowerIODataError) as excinfo:
        module.to_balanced(base_mva=100.0)
    error = excinfo.value
    assert error.code
    assert error.diagnostics
    assert error.diagnostics[0]["code"] == error.code
    for diagnostic in error.diagnostics:
        assert diagnostic.keys() == {"code", "severity", "message", "target"}
        assert diagnostic["code"].startswith("TRANSFORM.")
        # The lowering records are 1.0 diagnostics from the branch below;
        # legacy severities never reach this surface any more.
        assert diagnostic["severity"] in ("error", "warning", "remark", "note")
        assert diagnostic["message"]
    # The refusal leaves the handle usable, still carrying its module.
    assert module.kind == "multiconductor_network"


def test_successful_lowering_leaves_the_source_module_usable_and_records_intact():
    module = powerio.PioModule.from_str(LOWERABLE_DSS, "dss")
    doc = json.loads(module.to_json())
    doc["producer"] = {"name": "python-binding-test", "version": "1"}
    doc["history"] = [
        {"id": "before-lowering", "kind": "parse", "name": "test_parse"}
    ]
    doc["extensions"] = {"org.example.test": {"kept": True}}
    module = powerio.PioModule.from_json(json.dumps(doc))
    before = json.loads(module.to_json())

    lowered = module.to_balanced()

    # The native transform owns a record-complete sibling, not this handle.
    assert module.kind == "multiconductor_network"
    assert json.loads(module.to_json()) == before
    assert module.value.n_buses > 0

    lowered_doc = json.loads(lowered.to_json())
    assert lowered.kind == "balanced_network"
    assert lowered_doc["producer"] == before["producer"]
    assert lowered_doc["extensions"] == before["extensions"]
    assert lowered_doc["history"][0] == before["history"][0]
    assert any(
        entry["name"] == "lower_multiconductor_to_balanced"
        for entry in lowered_doc["history"]
    )


def test_dist_write_file_echoes_bytes(tmp_path):
    case = dist.parse_file(FOURWIRE)
    out = tmp_path / "echo.dss"
    warnings = case.write_file(out, "dss")
    assert warnings == []
    assert out.read_bytes() == FOURWIRE.read_bytes()


def test_parse_warnings_surface():
    case = dist.parse_str(
        "clear\n"
        "new circuit.w basekv=12.47 bus1=src\n"
        "new line.l1 bus1=src bus2=b2 length=1 units=furlong\n",
        "dss",
    )
    assert any("furlong" in w for w in case.warnings)


def test_unknown_format_raises_value_error():
    with pytest.raises(ValueError, match="unknown distribution format"):
        dist.parse_str("clear\n", "matpower")
    case = dist.parse_file(FOURWIRE)
    with pytest.raises(ValueError, match="unknown distribution format"):
        case.to_format("matpower")


def test_malformed_json_raises_parse_error():
    with pytest.raises(powerio.PowerIOParseError):
        dist.parse_str("{not json", "bmopf-json")


def test_missing_file_raises_precise_oserror():
    # Io errors map to the precise OSError subclass with the path attached.
    with pytest.raises(FileNotFoundError) as exc:
        dist.parse_file(DATA / "does_not_exist.dss")
    assert exc.value.filename and "does_not_exist.dss" in str(exc.value.filename)


def test_one_shot_convert_carries_parse_warnings():
    conv = dist.convert_str(
        "clear\n"
        "new circuit.w basekv=12.47 bus1=src\n"
        "new line.l1 bus1=src bus2=b2 length=1 units=furlong\n",
        "bmopf-json",
        "dss",
    )
    assert any("furlong" in w.message for w in conv.warnings)


def test_bmopf_containing_data_model_string_routes_to_bmopf(tmp_path):
    # The sniff keys on a TOP LEVEL data_model key; a nested occurrence is
    # not the marker.
    case = dist.parse_file(FOURWIRE)
    text = case.to_format("bmopf-json").text
    doc = json.loads(text)
    doc["bus"]["data_model"] = doc["bus"][next(iter(doc["bus"]))]
    p = tmp_path / "nested_marker.json"
    p.write_text(json.dumps(doc))
    assert dist.parse_file(p).source_format == "bmopf-json"


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


def test_include_root_admits_a_shared_sibling_include(tmp_path):
    root = _split_case(tmp_path)
    deck = root / "feeder" / "f.dss"
    confined = dist.parse_file(deck)
    assert any("escapes the case directory" in w for w in confined.warnings)
    widened = dist.parse_file(deck, include_root=root)
    assert widened.warnings == []
    assert widened.n_lines == 1


def test_include_root_still_refuses_escapes_past_it(tmp_path):
    root = _split_case(tmp_path)
    (tmp_path / "secret.dss").write_text("New Line.leaked bus1=x bus2=y\n")
    deck = root / "feeder" / "f.dss"
    deck.write_text(
        deck.read_text().replace("../shared/linecodes.dss", "../../secret.dss")
    )
    net = dist.parse_file(deck, include_root=root)
    assert any("escapes the include root" in w for w in net.warnings)


def test_case_file_outside_the_include_root_is_refused(tmp_path):
    root = _split_case(tmp_path)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "f.dss").write_text("New Circuit.out\n")
    with pytest.raises(powerio.PowerIOError, match="outside the requested acquisition root"):
        dist.parse_file(elsewhere / "f.dss", include_root=root)
