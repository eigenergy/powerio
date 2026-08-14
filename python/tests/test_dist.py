"""The powerio.dist surface: parse, echo, convert, warnings, errors."""

import json
from pathlib import Path

import pytest

import powerio
from powerio import dist

DATA = Path(__file__).resolve().parents[2] / "tests" / "data" / "dist"
FOURWIRE = DATA / "micro" / "fourwire_linecode.dss"


def test_parse_file_counts_and_source_format():
    case = dist.parse_file(FOURWIRE)
    assert case.source_format == "dss"
    assert case.n_buses > 0
    assert case.n_lines > 0
    assert isinstance(case.warnings, list)


def test_multiconductor_is_the_only_model_name():
    case = dist.parse_file(FOURWIRE)
    assert isinstance(case, dist.MulticonductorNetwork)
    assert "MulticonductorNetwork" in dist.__all__
    assert "DistCase" not in dist.__all__
    assert not hasattr(dist, "DistNetwork")
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
    graph = dist.parse_file(FOURWIRE).graph()
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
    assert any("furlong" in w for w in conv.warnings)


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
