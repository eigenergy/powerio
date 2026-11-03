"""Native fixed Line terminal states through PowerIO import, IR and fresh DSS.

Covers 1/2/3/4 conductors, partial and complete opens at both ends, reclosure,
MakeLike, conductor resizing, LineCode reassignment, explicit neutral and zero
charging. Every opened typed admittance is independently compared to native
YPrim. Every case compares original bus voltages and physical line-port
currents/powers after both first and repeated canonical conversion. A wrong
fixed state must fail. No source echoes or external dataset files are used.
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
import opendssdirect as dss
from oracle import LOCK, add, compare

cases = {
    "from_all": "open line.main 1",
    "to_one": "open line.main 2 1",
    "both_one": "open line.main 1 1\nopen line.main 2 1",
    "all": "open line.main 1\nopen line.main 2",
    "reclose": "open line.main 1\nclose line.main 1",
    "same_phases": "open line.main 2 1\nedit line.main phases={n}",
    "resize": "open line.main 2 1\nedit line.main phases=2\nedit line.main phases={n}",
    "code_same": "open line.main 2 1\nnew linecode.c nphases={n} r1=.2 x1=.4 r0=.6 x0=.9 c1=100 c0=70\nedit line.main linecode=c",
    "copy_same": "new line.other phases={n} enabled=no\nopen line.main 2 1\nedit line.main like=other",
}


def pairs(a):
    return np.array(a[::2]) + 1j * np.array(a[1::2])


def run(p):
    e = dss.NewContext()
    for k in ("AllowChangeDir", "AllowDOScmd", "AllowEditor", "AllowForms"):
        getattr(e.Basic, k)(False)
    assert LOCK["dss"]["version"] in e.Basic.Version()
    e.Text.Command(f'redirect "{p}"')
    assert e.Solution.Converged()
    return e


def snap(e, n, ports):
    s = {}
    for bus in ("sourcebus", "sink"):
        e.Circuit.SetActiveBus(bus)
        for node, v in zip(e.Bus.Nodes(), pairs(e.Bus.Voltages()), strict=True):
            add(s, f"{bus}.{node}", "V", v)
    for term, (name, start) in enumerate(ports):
        e.Circuit.SetActiveElement(name)
        for q, u, a in [
            ("current", "A", e.CktElement.Currents()),
            ("power", "VA", np.array(e.CktElement.Powers()) * 1000),
        ]:
            for i, v in enumerate(pairs(a)[start : start + n]):
                add(s, f"main.{term}.{i}.{q}", u, v)
    return s


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    bridge = args.bridge.resolve()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "bridge_sha256": hashlib.sha256(bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for n in (1, 2, 3, 4):
        for name, commands in list(cases.items()) + (
            [
                ("neutral_open", "open line.main 2 4"),
                ("neutral_all", "open line.main 1"),
                ("zero_cap", "edit line.main c1=0 c0=0\nopen line.main 2 1"),
            ]
            if n == 4
            else []
        ):
            if name == "resize" and n == 2:
                continue
            folder = out / f"{n}-{name}"
            folder.mkdir()
            path = folder / "input.dss"
            deck = f"clear\nnew circuit.c phases={n} basekv=12.47 bus1=sourcebus\nnew line.main phases={n} bus1=sourcebus bus2=sink r1=.2 x1=.4 r0=.6 x0=.9 c1=100 c0=70 length=1\n"
            for i in range(1, n + 1):
                deck += f"new load.l{i} phases=1 bus1=sink.{i}.0 kv=7.2 kw={10 + i} kvar=3 model=2\n"
            if name.startswith("neutral_"):
                deck = deck.replace("circuit.c phases=4", "circuit.c phases=3").replace(
                    "bus1=sourcebus bus2=sink",
                    "bus1=sourcebus.1.2.3.4 bus2=sink.1.2.3.4",
                )
                deck = (
                    "\n".join(
                        line
                        for line in deck.splitlines()
                        if not line.startswith("new load.l4")
                    )
                    + "\n"
                )
                for i in (1, 2, 3):
                    deck = deck.replace(f"bus1=sink.{i}.0", f"bus1=sink.{i}.4")
                deck += "new line.ground phases=1 bus1=sourcebus.4 bus2=sourcebus.0 r1=.3 x1=0 r0=.3 x0=0 c1=0 c0=0 length=1\n"
            deck += (
                commands.format(n=n)
                + "\nset controlmode=off tolerance=1e-12 maxiterations=1000\nsolve\n"
            )
            path.write_text(deck)
            row = {"input_sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            try:
                native = run(path)
                row["engine"] = native.Basic.Version()
                result = subprocess.run(
                    [str(bridge), str(path), str(folder / "import")],
                    text=True,
                    capture_output=True,
                    check=False,
                    timeout=120,
                )
                (folder / "stderr.txt").write_text(result.stderr)
                result.check_returncode()
                j = json.loads(result.stdout)
                (folder / "bridge.json").write_text(result.stdout)
                assert j["targets"]["dss"]["ok"], j["targets"]["dss"]
                actual = run(folder / "import/dss/case.dss")
                sh = [
                    s
                    for s in j["network"]["shunts"]
                    if s["name"].startswith("_pio_open_line_main")
                ]
                ports = (
                    [("line.main", 0), ("line.main", n)]
                    if not sh
                    else [
                        (f"line.{sh[0]['name']}_from", 0),
                        (f"line.{sh[0]['name']}_to", 0),
                    ]
                )
                expected = snap(native, n, [("line.main", 0), ("line.main", n)])
                got = snap(actual, n, ports)
                row["comparison"] = compare(expected, got)
                if sh:
                    native.Circuit.SetActiveElement("line.main")
                    yp = pairs(native.CktElement.YPrim()).reshape(
                        (2 * n, 2 * n), order="F"
                    )
                    typed = np.array(sh[0]["g"]) + 1j * np.array(sh[0]["b"])
                    er, ar = {}, {}
                    for i in range(2 * n):
                        for k in range(2 * n):
                            add(er, f"{i}.{k}", "S", yp[i, k])
                            add(ar, f"{i}.{k}", "S", typed[i, k])
                    row["primitive"] = compare(er, ar)
                repeated = subprocess.run(
                    [
                        str(bridge),
                        str(folder / "import/dss/case.dss"),
                        str(folder / "reimport"),
                    ],
                    text=True,
                    capture_output=True,
                    check=False,
                    timeout=120,
                )
                (folder / "reimport.stderr.txt").write_text(repeated.stderr)
                repeated.check_returncode()
                row["reimport_comparison"] = compare(
                    expected, snap(run(folder / "reimport/dss/case.dss"), n, ports)
                )
                native.Circuit.SetActiveElement("line.main")
                was_open = any(
                    native.CktElement.IsOpen(t, c)
                    for t in (1, 2)
                    for c in range(1, n + 1)
                )
                wrong_commands = (
                    "close line.main 1\nclose line.main 2"
                    if was_open
                    else "open line.main 2 1"
                )
                wrong_path = folder / "wrong-state.dss"
                wrong_path.write_text(
                    deck.removesuffix("solve\n") + wrong_commands + "\nsolve\n"
                )
                native = run(wrong_path)
                negative = compare(
                    expected, snap(native, n, [("line.main", 0), ("line.main", n)])
                )
                row["wrong_state_negative"] = {
                    "passed": not negative["passed"],
                    "comparison": negative,
                }
                row["passed"] = (
                    row["comparison"]["passed"]
                    and row.get("primitive", {"passed": True})["passed"]
                    and row["reimport_comparison"]["passed"]
                    and row["wrong_state_negative"]["passed"]
                )
                (folder / "expected.json").write_text(json.dumps(expected, indent=2))
                (folder / "actual.json").write_text(json.dumps(got, indent=2))
            except Exception as e:
                row = {"passed": False, "error": str(e)}
            report["cases"][folder.name] = row
            (out / "report.json").write_text(json.dumps(report, indent=2))
            print(
                folder.name,
                row["passed"],
                row.get("error", row.get("comparison", {}).get("failures", [])[:1]),
                flush=True,
            )
    report["passed"] = all(c["passed"] for c in report["cases"].values())
    (out / "report.json").write_text(json.dumps(report, indent=2))

    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
