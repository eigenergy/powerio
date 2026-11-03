#!/usr/bin/env python3
"""Qualify CIM tank conversion with native OpenDSS stamps and loaded phasors.

The reference decks are committed OpenDSS fixtures and hand-stated regulator
nameplates, never output from PowerIO's DSS parser. Both sides use the same
controlled source/load circuit. This covers the supported clock convention;
other clocks and disconnected/inactive tank states must refuse conversion.
"""

import argparse
from pathlib import Path
import re
import subprocess
import tempfile

import numpy as np
import opendssdirect as dss

ROOT = Path(__file__).resolve().parents[2]
TANKS = ROOT / "tests/data/dist/cim/tank_feeder.xml"
MICRO = ROOT / "tests/data/dist/micro"


def transformer_block(text, name):
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if re.match(rf"new\s+transformer\.{re.escape(name)}(?:\s|$)", line, re.I):
            block = [line]
            for following in lines[i + 1:]:
                if not following.lstrip().startswith("~"):
                    break
                block.append(following)
            return "\n".join(block)
    raise AssertionError(f"missing Transformer.{name}")


def complex_values(values):
    pairs = np.asarray(values).reshape(-1, 2)
    return pairs[:, 0] + 1j * pairs[:, 1]


def native(block, name, load_fraction=0.1):
    dss.Basic.ClearAll()
    dss.Text.Command("new circuit.control basekv=12.47 bus1=temporary")
    for command in block.splitlines():
        dss.Text.Command(command)
    dss.Transformers.Name(name)
    phases = dss.CktElement.NumPhases()
    buses = dss.CktElement.BusNames()
    windings = dss.Transformers.NumWindings()
    settings = []
    for winding in range(1, windings + 1):
        dss.Transformers.Wdg(winding)
        settings.append((dss.Transformers.kV(), dss.Transformers.kVA(), dss.Transformers.IsDelta()))
    primary_kv, _, primary_delta = settings[0]
    source_kv = primary_kv * (np.sqrt(3) if phases == 1 and not primary_delta else 1)
    primary_bus = buses[0].split(".")[0]
    dss.Basic.ClearAll()
    dss.Text.Command("set defaultbasefrequency=60")
    dss.Text.Command(f"new circuit.control phases=3 bus1={primary_bus}.1.2.3 basekv={source_kv} pu=1 r1=.00001 x1=.00001 r0=.00001 x0=.00001")
    for command in block.splitlines():
        dss.Text.Command(command)
    for i, (kv, kva, delta) in enumerate(settings[1:], 1):
        conn = "delta" if delta else "wye"
        dss.Text.Command(f"new load.l{i} bus1={buses[i]} phases={phases} conn={conn} kv={kv} kw={kva * load_fraction} kvar={kva * load_fraction / 3} model=1 vminpu=.1 vmaxpu=2")
    dss.Text.Command("set maxiterations=100")
    dss.Solution.Solve()
    assert dss.Solution.Converged(), name
    assert dss.Circuit.SetActiveElement(f"Transformer.{name}"), name
    nodes = dss.CktElement.NodeOrder()
    ncond = dss.CktElement.NumConductors()
    stamp = complex_values(dss.CktElement.YPrim()).reshape(len(nodes), len(nodes), order="F")
    # Ground node 0 is prescribed zero voltage. Explicit neutral grounding
    # can add a large diagonal there without changing any active nodal stamp.
    active = np.flatnonzero(np.asarray(nodes) != 0)
    stamp = stamp[np.ix_(active, active)]
    volts = complex_values(dss.CktElement.Voltages()).reshape(windings, ncond)
    coils = []
    for v, (_, _, delta) in zip(volts, settings):
        if phases == 1:
            coils.append(v[:1] - v[1])
        elif delta:
            coils.append(v[:phases] - np.roll(v[:phases], -1))
        else:
            coils.append(v[:phases] - v[-1])
    lag = -np.angle(volts[1, 0] / volts[0, 0], deg=True)
    return stamp, np.concatenate(coils), nodes, lag


def references():
    for fixture, pairs, core in [
        ("xfmr_center_tap.dss", [("t1", "ct_tank")], ""),
        ("xfmr_open_wye_open_delta.dss", [("t1", "od_t1"), ("t2", "od_t2")], ""),
        ("xfmr_single_phase.dss", [("t1", "sp_tank")], " %noloadloss=.2 %imag=.5"),
        ("xfmr_delta_wye.dss", [("t1", "dy_tank")], ""),
        ("xfmr_wye_delta.dss", [("t1", "wd")], " %noloadloss=.1 %imag=.4"),
    ]:
        text = (MICRO / fixture).read_text()
        for old, new in pairs:
            yield new, old, transformer_block(text, old) + core
    for name, phase, tap in [("rega", 1, 1.0625), ("regb", 2, 1.05), ("regc", 3, 1.06875)]:
        yield name, name, f"new transformer.{name} phases=1 windings=2 buses=(sourcebus.{phase}.0 reg_out.{phase}.0) conns=(wye wye) kvs=(2.4 2.4) kvas=(1666 1666) %rs=(.005 .005) xhl=.01 taps=(1 {tap})"


def run(powerio, *args):
    return subprocess.run([powerio, *map(str, args)], capture_output=True, text=True)


def refusals(powerio, work):
    source = TANKS.read_text()
    variants = []
    for cls, ident in [("TransformerTank", "_ct_tank"), ("PowerTransformer", "_ct")]:
        tag = f'<cim:{cls} rdf:ID="{ident}">'
        assert tag in source
        for prop in ("inService", "normallyInService"):
            variants.append(source.replace(tag, f"{tag}<cim:Equipment.{prop}>false</cim:Equipment.{prop}>", 1))
    # Derive the terminal ID through the end's association, not spelling.
    for number in (1, 2):
        end = source.index(f'<cim:TransformerTankEnd rdf:ID="_ct_tank_e{number}">')
        terminal = re.search(r'TransformerEnd.Terminal rdf:resource="#([^\"]+)"', source[end:]).group(1)
        tag = f'<cim:Terminal rdf:ID="{terminal}">'
        variants.append(source.replace(tag, f"{tag}<cim:ACDCTerminal.connected>false</cim:ACDCTerminal.connected>", 1))
    start = source.index('<cim:TransformerEndInfo rdf:ID="_dy_info_e2">')
    end = source.index('</cim:TransformerEndInfo>', start)
    for clock in ("11", "1.5"):
        block = source[start:end].replace("phaseAngleClock>1<", f"phaseAngleClock>{clock}<")
        variants.append(source[:start] + block + source[end:])
    for i, text in enumerate(variants):
        xml, out = work / f"refuse{i}.xml", work / f"refuse{i}.dss"
        xml.write_text(text)
        done = run(powerio, "convert", xml, "--to", "dss", "-o", out)
        assert done.returncode != 0 and not out.exists(), done.stderr
    print(f"PASS {len(variants)} inactive/disconnected/unsupported-clock conversion refusals")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--powerio", required=True)
    args = parser.parse_args()
    powerio = str(Path(args.powerio).resolve())
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        ir, output = work / "tank.pio.json", work / "tank.dss"
        for command in [("serialize", TANKS, "-o", ir), ("convert", ir, "--to", "dss", "-o", output)]:
            done = run(powerio, *command)
            assert done.returncode == 0, done.stderr
        texts = [("reader", output.read_text())]
        canonical = work / "fresh.xml"
        fresh_ir, fresh_output = work / "fresh.pio.json", work / "fresh.dss"
        for command in [("convert", ir, "--to", "cim-dist", "-o", canonical),
                        ("serialize", canonical, "-o", fresh_ir),
                        ("convert", fresh_ir, "--to", "dss", "-o", fresh_output)]:
            done = run(powerio, *command)
            assert done.returncode == 0, done.stderr
        texts.append(("fresh writer", fresh_output.read_text()))
        for route, text in texts:
            for name, reference_name, block in references():
                expected = native(block, reference_name)
                got = native(transformer_block(text, name), name)
                assert expected[2] == got[2], name
                np.testing.assert_allclose(got[0], expected[0], rtol=1e-10, atol=1e-8, err_msg=f"{route} {name} grounded YPrim")
                np.testing.assert_allclose(got[1], expected[1], rtol=1e-9, atol=1e-6, err_msg=f"{route} {name} loaded coil phasors")
                print(f"PASS {route} {name}: grounded YPrim and loaded coil phasors; max voltage error {np.max(np.abs(got[1] - expected[1])):.3g} V")
        dy = transformer_block(text, "dy_tank")
        clock1 = native(dy, "dy_tank", load_fraction=0)
        clock11 = native(dy + " leadlag=lead", "dy_tank", load_fraction=0)
        assert abs(clock1[3] - 30) < 1e-4 and abs(clock11[3] + 30) < 1e-4
        assert not np.allclose(clock1[0], clock11[0]), "clock control did not change the native stamp"
        print("PASS independent clock control: lag=clock 1; lead=clock 11 has a different native stamp")
        refusals(powerio, work)
    print(f"OpenDSSDirect.py {dss.__version__}; {dss.Basic.Version()}")


if __name__ == "__main__":
    main()
