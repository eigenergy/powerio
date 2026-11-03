"""Observe line/linecode copy and constructor-frequency semantics.

Base-frequency decisions are qualified independently of future line matrix
lowering. Native line primitives are retained as evidence for that next step;
this harness does not claim they already match PowerIO's typed line matrices.
"""

import argparse
import hashlib
import json
from pathlib import Path

import opendssdirect as dss
from oracle import LOCK


def layouts():
    line = "New Line.target bus1=feed bus2=load length=1 units=km "
    code = "New Linecode.proto nphases=3 r1=0.2 x1=0.4 r0=0.5 x0=0.9 c1=12 c0=6 "
    return {
        "inline_default": line,
        "inline_explicit": line + "basefreq=45",
        "inline_edit": line + "\nEdit Line.target basefreq=45",
        "code_default": code + "\n" + line + "linecode=proto",
        "code_explicit": code + "basefreq=45\n" + line + "linecode=proto",
        "code_late_edit": code
        + "basefreq=45\n"
        + line
        + "linecode=proto\nEdit Linecode.proto basefreq=35 r1=0.7",
        "code_rebound": code
        + "basefreq=45\n"
        + line
        + "linecode=proto\nEdit Linecode.proto basefreq=35\nEdit Line.target linecode=proto",
        "override_after_code": code
        + "basefreq=45\n"
        + line
        + "linecode=proto basefreq=55",
        "override_before_code": code
        + "basefreq=45\n"
        + line
        + "basefreq=55 linecode=proto",
        "fundamental_before": "Set basefrequency=65\n" + line,
        "fundamental_after": line + "\nSet basefrequency=65",
        "default_before": "Set defaultbasefrequency=65\n" + line,
        "operating_before": "Set frequency=65\n" + line,
        "copy_line": "New Line.template bus1=feed bus2=load basefreq=45\nSet basefrequency=65\n"
        + line
        + "like=template",
        "copy_line_late_edit": "New Line.template bus1=feed bus2=load basefreq=45\n"
        + line
        + "like=template\nEdit Line.template basefreq=35",
        "copy_empty_code": "New Linecode.template\nSet basefrequency=65\nNew Linecode.proto like=template\n"
        + line
        + "linecode=proto",
        "copy_code_late_edit": "New Linecode.template basefreq=45\nNew Linecode.proto like=template\nEdit Linecode.template basefreq=35\n"
        + line
        + "linecode=proto",
        "nested_copy_code": "New Linecode.template basefreq=45\nNew Linecode.mid like=template\nNew Linecode.proto like=mid\nEdit Linecode.mid basefreq=35\n"
        + line
        + "linecode=proto",
        "self_copy_line": line
        + "basefreq=45\nEdit Line.target basefreq=55 like=target",
        "self_copy_code": "New Linecode.proto basefreq=45\nEdit Linecode.proto basefreq=55 like=proto\n"
        + line
        + "linecode=proto",
        "empty_self_copy": "New Linecode.proto\nEdit Linecode.proto like=proto\nEdit Linecode.proto like=proto\n"
        + line
        + "linecode=proto",
        "code_two_consumers": code
        + "basefreq=45\n"
        + line
        + "linecode=proto\nEdit Linecode.proto basefreq=35\nNew Line.later bus1=feed bus2=load linecode=proto",
    }


def observe(script, output):
    output.mkdir(parents=True, exist_ok=False)
    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    engine.Basic.DataPath(str(output.resolve()))
    version = engine.Basic.Version()
    if LOCK["dss"]["version"] not in version:
        raise ValueError("unpinned engine")
    result = {
        "script": script,
        "engine": version,
        "sha256": hashlib.sha256(script.encode()).hexdigest(),
    }
    try:
        for command in script.splitlines():
            engine.Text.Command(command)
        if not engine.Solution.Converged():
            raise ValueError("native circuit did not converge")
        frequencies = {}
        primitives = {}
        for cls in ("linecode", "line"):
            engine.Basic.SetActiveClass(cls)
            names = engine.ActiveClass.AllNames()
            for name in names:
                if name.lower() == "none":
                    continue
                key = f"{cls}.{name}"
                engine.Text.Command(f"? {key}.basefreq")
                frequencies[key] = float(engine.Text.Result())
                if cls == "line":
                    engine.Circuit.SetActiveElement(key)
                    primitives[key] = engine.CktElement.YPrim()
        result.update(frequencies=frequencies, primitives=primitives)
    except Exception as error:
        result["error"] = str(error)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}, "failures": []}
    for label, body in layouts().items():
        for constructor in (50, 60):
            for operating in (50, 60, 70):
                name = f"{label}/{constructor}/{operating}"
                script = (
                    f"Clear\nSet defaultbasefrequency={constructor}\n"
                    "New Circuit.c phases=3 bus1=feed basekv=0.24 z1=[0.1 0.1]\n"
                    f"{body}\nNew Load.test phases=3 bus1=load kv=0.24 kw=1 model=2\n"
                    f"Edit Vsource.source frequency={operating}\n"
                    f"Set frequency={operating} controlmode=off\nSolve\n"
                )
                result = observe(script, args.output / "native" / name)
                if "error" in result:
                    report["failures"].append(name)
                report["cases"][name] = result
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"cases": len(report["cases"]), "failures": report["failures"]}))
    if report["failures"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
