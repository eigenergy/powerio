"""Tests for the PSS/E contingency analysis values and their network methods."""

from pathlib import Path

import pytest

import powerio as pio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
CONTINGENCY = DATA / "psse" / "contingency"


@pytest.mark.parametrize(
    ("name", "value_class"),
    [
        ("resolve_cases.con", pio.ContingencySet),
        ("expand.con", pio.ContingencySet),
        ("selectors.sub", pio.SubsystemSet),
        ("psse35_area.sub", pio.SubsystemSet),
        ("generated.mon", pio.MonitoredSet),
        ("blocks.mon", pio.MonitoredSet),
    ],
)
def test_each_file_parses_to_its_own_value_class(name, value_class):
    module = pio.parse(CONTINGENCY / name)
    value = module.value
    assert isinstance(value, value_class)
    assert value.text == (CONTINGENCY / name).read_text()


def test_emit_returns_the_parsed_file(tmp_path):
    module = pio.parse(CONTINGENCY / "resolve_cases.con")
    result = pio.emit(module, "psse-con")
    assert result.text == (CONTINGENCY / "resolve_cases.con").read_text()

    written = tmp_path / "cases.con"
    pio.emit(module, "psse-con", written)
    assert written.read_bytes() == (CONTINGENCY / "resolve_cases.con").read_bytes()


def test_resolve_contingencies_counts_every_case():
    network = pio.parse(CONTINGENCY / "resolve_v33.raw").value
    cases = pio.parse(CONTINGENCY / "resolve_cases.con").value

    resolution = network.resolve_contingencies(cases.text)
    assert resolution["cases"] == 20
    assert resolution["resolved"] == 17
    assert resolution["unresolved"] == 3
    assert resolution["unrecognized_statements"] == 1
    assert len(resolution["case_results"]) == 20

    first = resolution["case_results"][0]
    assert first["name"] == "BR_1_2_C1"
    assert first["resolved"] is True
    assert first["unresolved"] == []
    assert first["components"] == [
        {"type": "branch", "id": "1-2", "row": 0, "in_service": True}
    ]

    # Each case that bound to nothing states one reason and the statement the
    # `.con` writer produces for the action that named no element.
    unresolved = {
        case["name"]: case["unresolved"]
        for case in resolution["case_results"]
        if not case["resolved"]
    }
    assert unresolved == {
        "BR_MISSING": [
            {
                "action": "OPEN LINE FROM BUS      3 TO BUS      4 CIRCUIT 1",
                "reason": "no_such_branch",
            }
        ],
        "MACHINE_MISSING": [
            {
                "action": "REMOVE MACHINE 9 FROM BUS      1",
                "reason": "no_such_machine",
            }
        ],
        "UNRECOGNIZED": [
            {
                "action": "PARALLEL BRANCH FROM BUS      1 TO BUS      2",
                "reason": "unrecognized",
            }
        ],
    }

    # One statement of the fixture is kept as text, so the reader reports it.
    assert resolution["diagnostics"]
    assert all(
        isinstance(diagnostic, pio.Diagnostic) for diagnostic in resolution["diagnostics"]
    )


def test_resolve_contingencies_refuses_a_malformed_file():
    network = pio.parse(CONTINGENCY / "resolve_v33.raw").value
    with pytest.raises(pio.PowerIOParseError):
        network.resolve_contingencies("CONTINGENCY 'A'\nCONTINGENCY 'B'\nEND\nEND\n")


def test_expand_contingencies_states_every_generated_case():
    network = pio.parse(CONTINGENCY / "select_v33.raw").value
    cases = pio.parse(CONTINGENCY / "expand.con").value
    groups = pio.parse(CONTINGENCY / "selectors.sub").value

    expanded, notes = network.expand_contingencies(cases.text, groups.text)
    names = [
        line.split("'")[1]
        for line in expanded.splitlines()
        if line.startswith("CONTINGENCY '")
    ]
    assert names == [
        "EXPLICIT",
        "L_101_102_1",
        "L_102_103_1",
        "T_101_102_103_1",
        "G_101_1",
        "G_101_2",
        "L_103_201_1",
        "L_101_102_1+L_102_103_1",
    ]
    # The specification naming a subsystem the `.sub` file does not state stays.
    assert "SINGLE BRANCH IN SUBSYSTEM 'NOSUCH'" in expanded
    # The notes carry the two readers' findings and then the expansion's own.
    # `selectors.sub` holds one TARA line the subsystem reader keeps as text.
    codes = [note.code for note in notes]
    assert codes == ["READ.SUB.STATEMENT_UNRECOGNIZED", "BUILD.CON.SUBSYSTEM_UNKNOWN"]
    assert "'NOSUCH'" in notes[-1].message


def test_select_subsystem_buses_names_the_buses_of_one_group():
    network = pio.parse(CONTINGENCY / "select_v33.raw").value
    groups = pio.parse(CONTINGENCY / "selectors.sub").value

    assert network.select_subsystem_buses(groups.text, "A1") == [101, 102, 103]
    assert network.select_subsystem_buses(groups.text, "AREARANGE") == [
        101,
        102,
        103,
        201,
        202,
    ]
    assert network.select_subsystem_buses(groups.text, "KV") == [101, 102, 103, 201]

    with pytest.raises(ValueError):
        network.select_subsystem_buses(groups.text, "NOSUCH")
