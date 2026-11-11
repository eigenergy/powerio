"""Check XIIDM/JIIDM extension targets with native PowSybl (#837).

Run: python evals/powsybl/check_extension_targets.py /path/to/powerio
Requires pypowsybl (tested with 1.16.1). All inputs are synthetic and temporary.
IR edits represent partial hierarchy and disconnected equipment; output always
uses fresh serialization. Negative controls restore the invalid references.
"""

import copy
import json
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path

import pypowsybl as pp


IIDM = "http://www.powsybl.org/schema/iidm/1_17"
APC = "http://www.powsybl.org/schema/iidm/ext/active_power_control/1_2"
SLACK = "http://www.powsybl.org/schema/iidm/ext/slack_terminal/1_5"
SOURCE = f'''<network xmlns="{IIDM}" xmlns:apc="{APC}" xmlns:slt="{SLACK}"
 id="extension-targets" caseDate="2026-01-01T00:00:00Z" forecastDistance="0"
 sourceFormat="synthetic" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <substation id="S">
    <voltageLevel id="VL1" nominalV="110" topologyKind="BUS_BREAKER">
      <busBreakerTopology><bus id="B1" v="110" angle="0"/></busBreakerTopology>
      <generator id="G" energySource="OTHER" minP="0" maxP="50"
       voltageRegulatorOn="true" targetP="20" targetQ="2" targetV="110"
       bus="B1" connectableBus="B1"><minMaxReactiveLimits minQ="-10" maxQ="10"/></generator>
      <battery id="BAT" targetP="3" targetQ="1" minP="-5" maxP="5"
       bus="B1" connectableBus="B1"><minMaxReactiveLimits minQ="-2" maxQ="2"/></battery>
    </voltageLevel>
    <voltageLevel id="VL2" nominalV="110" topologyKind="BUS_BREAKER">
      <busBreakerTopology><bus id="B2" v="110" angle="0"/></busBreakerTopology>
    </voltageLevel>
  </substation>
  <line id="LINE" r="1" x="10" g1="0" b1="0" g2="0" b2="0"
   bus1="B1" connectableBus1="B1" voltageLevelId1="VL1"
   bus2="B2" connectableBus2="B2" voltageLevelId2="VL2"/>
  <extension id="G"><apc:activePowerControl participate="true" droop="4" participationFactor="2"/></extension>
  <extension id="BAT"><apc:activePowerControl participate="false" droop="5" participationFactor="3"/></extension>
  <extension id="VL1"><slt:slackTerminal id="G"/></extension>
</network>'''


def check(binary):
    cases = negative_controls = 0
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        source = root / "source.xiidm"
        source.write_text(SOURCE)
        # Independently check the starting fixture before mutating its IR.
        pp.network.load(str(source))
        ir = root / "source.pio.json"
        subprocess.run([binary, "serialize", str(source), "-o", str(ir)],
                       check=True, capture_output=True, text=True)
        original = json.loads(ir.read_text())
        for scenario in ("complete", "disconnected", "omitted-injections", "omitted-branch"):
            document = copy.deepcopy(original)
            network = document["value"]["data"]
            detailed = network["detailed_connectivity"]
            if scenario == "disconnected":
                for table in ("generators", "storage"):
                    network[table][0]["in_service"] = False
                for terminal in detailed["terminals"]:
                    if terminal["equipment"]["local_id"] in ("G", "BAT"):
                        terminal.pop("bus")
                        terminal["connected"] = False
            elif scenario.startswith("omitted"):
                detailed["voltage_levels"] = [v for v in detailed["voltage_levels"]
                                              if v["component"]["local_id"] == "VL2"]
                for table in ("bus_breaker_buses", "terminals"):
                    detailed[table] = [v for v in detailed[table]
                                       if v["voltage_level"]["local_id"] == "VL2"]
                if scenario == "omitted-branch":
                    network["generators"] = []
                    network["storage"] = []
                    for bus in network["buses"]:
                        bus["kind"] = "REF" if bus["uid"] == "B2" else "PQ"
            ir.write_text(json.dumps(document))
            for target in ("xiidm", "jiidm"):
                output = root / f"{scenario}.{target}"
                result = subprocess.run([binary, "convert", str(ir), "--to", target, "-o", str(output)],
                                        check=True, capture_output=True, text=True)
                native = pp.network.load(str(output))
                if scenario in ("complete", "disconnected"):
                    assert set(native.get_generators().index) == {"G"}
                    assert set(native.get_batteries().index) == {"BAT"}
                    controls = native.get_extensions("activePowerControl")
                    # The Python 1.16.1 extension dataframe lists generators
                    # only. Native IIDM serialization also exposes batteries.
                    assert set(controls.index) == {"G"}
                    assert controls.loc["G", "participate"]
                    assert controls.loc["G", "droop"] == 4
                    assert controls.loc["G", "participation_factor"] == 2
                    native_xml = ET.fromstring(native.save_to_string("XIIDM"))
                    battery_control = native_xml.find(f"{{{IIDM}}}extension[@id='BAT']/{{{APC}}}activePowerControl")
                    assert battery_control is not None
                    assert battery_control.attrib["participate"] == "false"
                    assert float(battery_control.attrib["droop"]) == 5
                    assert float(battery_control.attrib["participationFactor"]) == 3
                    connected = scenario == "complete"
                    assert bool(native.get_generators().loc["G", "connected"]) == connected
                    assert bool(native.get_batteries().loc["BAT", "connected"]) == connected
                    # An in-service line takes precedence over disconnected
                    # injections while stating the same reference bus.
                    expected_slack = "G" if connected else "LINE"
                    assert native.get_extensions("slackTerminal").loc["VL1", "element_id"] == expected_slack
                else:
                    assert native.get_generators().empty
                    assert native.get_batteries().empty
                    assert native.get_lines().empty
                    assert native.get_extensions("activePowerControl").empty
                    assert native.get_extensions("slackTerminal").empty
                    assert f"EMIT.{target.upper()}.FIELD_DROPPED" in result.stderr
                    assert f"EMIT.{target.upper()}.RECORD_DROPPED" in result.stderr
                    # Restoring each old orphan reference must fail the native
                    # importer. Check both encodings and each extension kind.
                    targets = ("G", "BAT") if scenario == "omitted-injections" else ("LINE",)
                    for missing in targets:
                        mutant = root / f"negative-{missing}.{target}"
                        extension_name = "slackTerminal" if missing == "LINE" else "activePowerControl"
                        owner = "VL2" if missing == "LINE" else missing
                        attributes = ({"id": "LINE", "side": "TWO"} if missing == "LINE"
                                      else {"participate": True, "droop": 4.0})
                        if target == "xiidm":
                            xml = ET.fromstring(output.read_text())
                            extension = ET.SubElement(xml, f"{{{IIDM}}}extension", id=owner)
                            namespace = SLACK if missing == "LINE" else APC
                            ET.SubElement(extension, f"{{{namespace}}}{extension_name}",
                                          {k: str(v).lower() if isinstance(v, bool) else str(v)
                                           for k, v in attributes.items()})
                            mutant.write_bytes(ET.tostring(xml))
                        else:
                            data = json.loads(output.read_text())
                            data.setdefault("extensions", []).append({"id": owner, extension_name: attributes})
                            mutant.write_text(json.dumps(data))
                        try:
                            pp.network.load(str(mutant))
                        except pp.PyPowsyblError as error:
                            assert missing.lower() in str(error).lower(), str(error)
                        else:
                            raise AssertionError(f"native importer accepted {extension_name} for missing {missing}")
                        negative_controls += 1
                cases += 1
    print(f"PASS: {cases} XIIDM/JIIDM cases, {negative_controls} rejected orphan references; "
          f"native PowSybl {pp.__version__}")


if __name__ == "__main__":
    check(str(Path(sys.argv[1]).resolve()))
