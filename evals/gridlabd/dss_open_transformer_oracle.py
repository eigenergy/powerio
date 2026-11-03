"""Native open-transformer ports through import, IR, canonical DSS and typed edits."""
import argparse
import hashlib
import json
from pathlib import Path

from european_lv_import_oracle import node_inventory, snapshot, source_inventory
from ieee4_import_oracle import evaluate, run
from open_transformer_ports import interfaces
from oracle import compare


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--check-edits", action="store_true")
    args = parser.parse_args()
    if args.check_edits:
        edits(args.output)
        return
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(), cases={})
    cases = []
    for phases, configurations in [(3, ["wye wye", "delta wye", "delta delta", "wye delta"]),
                                   (2, ["wye wye", "delta wye", "delta delta", "wye delta"]),
                                   (1, ["wye wye", "delta delta"])]:
        for conns in configurations:
            for opening in ([0], [1], [phases + 1], [1, phases + 1]):
                for winding in (1, 2):
                    cases.append((phases, conns, opening, winding))
    cases.extend((3, "wye wye wye", opening, 3) for opening in ([0], [1], [4], [1, 4]))
    for phases, conns, opening, winding in cases:
        name = f"p{phases}-{conns.replace(' ', '-')}-w{winding}-c" + "_".join(map(str, opening))
        folder = args.output / name
        folder.mkdir()
        delta = phases == 1 and conns.startswith("delta")
        mapping = "1.2" if delta else "1.0" if phases == 1 else "1.2.3.0"
        count = len(conns.split())
        maps = [("1.2.3" if conn == "delta" else "1.2.0") if phases == 2 else mapping for conn in conns.split()]
        buses = [f"{bus}.{nodes}" for bus, nodes in zip(["primary", "secondary", "tertiary"][:count], maps, strict=True)]
        rating = [12.47, .48] + ([4.16] if count == 3 else [])
        text = ("New Circuit.c basekv=12.47 r1=.1 x1=.2 r0=.2 x0=.4\n"
                "New Line.feed bus1=sourcebus bus2=primary phases=3 length=1 units=m r1=.1 x1=.2 r0=.2 x0=.4 c1=0 c0=0\n"
                f"New Transformer.t phases={phases} windings={count} buses=[{' '.join(buses)}] conns=[{conns}] "
                f"kvs={json.dumps(rating)} kvas={json.dumps([1000] * count)} %rs={json.dumps([.2, .3] + ([.4] if count == 3 else []))} "
                "xhl=5 xht=7 xlt=9 %noloadloss=.1 %imag=.2 ppm_antifloat=1\n"
                "New Vsource.other bus1=secondary basekv=.48 pu=.99 angle=-20 r1=.003 x1=.005 r0=.004 x0=.006\n"
                "New Load.l bus1=secondary.1.0 phases=1 kv=.277128 kw=10 kvar=3 model=2\n"
                "New Load.input bus1=primary.2.0 phases=1 kv=7.2 kw=20 kvar=5 model=2\n")
        if count == 3:
            text += "New Vsource.third bus1=tertiary basekv=4.16 pu=1.01 angle=15 r1=.03 x1=.05 r0=.04 x0=.06\n"
        text += "".join(f"Open Transformer.t {winding} {c}\n" for c in opening)
        # Ordinary parameter edits must preserve terminal state.
        text += "Edit Transformer.t %imag=.25\n"
        source = folder / "native.dss"
        source.write_text(text)
        row = evaluate(source.resolve(), args.bridge.resolve(), folder.resolve(), 1e-11)
        row["windings"] = count
        report["cases"][name] = row
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(name, row["passed"], row.get("error", ""), flush=True)
    (args.output / "cases.json").write_text(json.dumps(list(report["cases"]), indent=2) + "\n")
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")


def edits(root):
    original_report = json.loads((root / "report.json").read_text())
    report = dict(cases={})
    for name in json.loads((root / "cases.json").read_text()):
        folder = root / name
        count = original_report["cases"][name]["windings"]
        path = folder / "native-edited.dss"
        path.write_text((folder / "native.dss").read_text()
                        + f"Edit Transformer.t %rs={json.dumps([v * 1.17 for v in [.2, .3, .4][:count]])} "
                        + f"taps={json.dumps([1.03] + [1] * (count - 1))}\n")
        native = run(path, 1e-11)
        buses = [bus.lower() for bus in native.Circuit.AllBusNames()]
        elements = [n.lower() for n in native.Circuit.AllElementNames() if n.lower().startswith(("line.", "transformer.", "load."))]
        expected = snapshot(native, buses, elements, canonical=False)
        network = json.loads((folder / "imported/bridge.json").read_text())["network"]
        port_maps = interfaces(native, network)
        actual = snapshot(run(folder / "typed-edited/case.dss", 1e-11), buses, elements, canonical=True,
                          port_maps=port_maps, bus_nodes=node_inventory(native, buses), source_port=source_inventory(native))
        comparison = compare(expected, actual)
        unchanged = snapshot(run(folder / "native.dss", 1e-11), buses, elements, canonical=False)
        negative = compare(unchanged, actual)
        row = dict(comparison=comparison, unchanged_negative=negative,
                   passed=comparison["passed"] and not negative["passed"])
        report["cases"][name] = row
        (root / "edited-report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(name, row["passed"], flush=True)
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (root / "edited-report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
