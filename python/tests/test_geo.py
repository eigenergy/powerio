"""Tests for the geographic layer surface and the AC OPF instance."""

from pathlib import Path

import pytest

import powerio as pio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"

BUSCOORDS = "1, -89.6, 40.6\n2, -89.2, 39.8\n"

DSS_MASTER = (
    "New Circuit.c1 bus1=sourcebus basekv=12.47\n"
    "New Line.l1 bus1=sourcebus bus2=loadbus length=1 units=km\n"
)


def test_parse_geo_normalizes_a_buscoords_sidecar():
    parsed = pio.parse_geo(BUSCOORDS)
    doc = parsed["geojson"]
    assert doc["type"] == "FeatureCollection"
    assert doc["powerio_geo"]["space"] == "geographic"
    assert len(doc["features"]) == 2
    assert parsed["diagnostics"] == []
    assert "warnings" not in parsed


def test_parse_geo_rejects_input_without_coordinates():
    with pytest.raises(pio.PowerIOParseError):
        pio.parse_geo("not a geo file")


def test_network_apply_and_extract_round_trip():
    net = pio.parse_file(DATA / "case9.m", value_type=pio.BalancedNetwork).value
    assert net.to_geo_layer()["features"] == []

    placed, report = net.apply_geo_layer(BUSCOORDS)
    assert report["matched_buses"] == 2
    assert report["unmatched_features"] == 0
    # The counts cover the whole case, so a caller sees what the layer left.
    assert report["unlocated_buses"] == 7
    assert report["unlocated_branches"] == 9
    # The input case is unchanged; the placed copy carries the layer.
    assert net.to_geo_layer()["features"] == []
    layer = placed.to_geo_layer()
    assert not hasattr(placed, "geo_layer")
    assert len(layer["features"]) == 2


def test_dist_apply_returns_a_placed_copy():
    net = pio.parse_text(
        DSS_MASTER,
        name="master.dss",
        format="dss",
    ).value
    placed, report = net.apply_geo_layer(
        "sourcebus, -89.6, 40.6\nloadbus, -89.2, 39.8\n"
    )
    assert report["matched_buses"] == 2
    layer = placed.to_geo_layer()
    assert len(layer["features"]) == 2
