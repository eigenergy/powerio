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
    via_str = dist.convert_str(text, "dss", "pmd-json")
    via_file = dist.convert_file(FOURWIRE, "pmd-json")
    assert via_str.text == via_file.text
    assert isinstance(via_str, powerio.Conversion)


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
    # Matches the transmission surface: io errors map to the precise OSError
    # subclass, not the package base error.
    with pytest.raises(FileNotFoundError):
        dist.parse_file(DATA / "does_not_exist.dss")


def test_one_shot_convert_carries_parse_warnings():
    conv = dist.convert_str(
        "clear\n"
        "new circuit.w basekv=12.47 bus1=src\n"
        "new line.l1 bus1=src bus2=b2 length=1 units=furlong\n",
        "dss",
        "bmopf-json",
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
