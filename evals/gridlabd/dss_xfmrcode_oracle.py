"""Capture assignment-time transformer-code states with the pinned native engine."""

import argparse
import json
import re
from pathlib import Path

import opendssdirect as dss
from oracle import LOCK


def cases():
    common = "new xfmrcode.c phases=1 windings=3 kvs=[7.2 .12 .12] kvas=[2000 2000 2000] %rs=[.6 1.2 1.2] xhl=2.04 xht=2.04 xlt=1.36 %imag=.5 %noloadloss=.2\n"
    cases = {
        "center_tap": common,
        "connection_abbreviations": "new xfmrcode.c windings=3 conns=[w wy del] xhl=5\n",
        "scalar_connection_abbreviations": "new xfmrcode.c conn=delt wdg=2 conn=y xhl=6\n",
        "positional": "new xfmrcode.c 1 2 1 delta 12.47 1500 1.02 .35 2 3\n~ wdg=2 conn=wye kv=.48\n~ xhl=4\n",
        "scalar_rating": "new xfmrcode.c windings=3 wdg=1 kva=500 wdg=3 kva=100\n~ wdg=2 %r=.7 rneut=2 xneut=3\n~ xhl=5\n",
        "two_winding_rating": "new xfmrcode.c wdg=2 kva=300 xhl=6\n",
        "array_order": "new xfmrcode.c windings=3 xhl=5 xscarray=[1 2 3]\n~ xscarray=[4 5 6]\n",
        "windings_reset": common + "edit xfmrcode.c windings=3\n",
        "changed_size": common
        + "edit xfmrcode.c windings=4\n~ xscarray=[1 2 3 4 5 6] kvs=[12 4 2 1]\n",
        "copy_defaults": common
        + "new xfmrcode.empty xhl=7\nedit xfmrcode.c like=empty\n",
        "copy_omissions": common
        + "new xfmrcode.target %imag=.9 ppm_antifloat=7 wdg=2\n~ like=c kv=.24\nnew transformer.other xfmrcode=target buses=[a b c]\n",
        "self_copy": common + "edit xfmrcode.c like=c\n",
        "historical": common
        + "new transformer.early xfmrcode=c buses=[a b c]\nedit xfmrcode.c kvs=[12 .24 .24] %imag=.8\n",
        "zero_default": "new xfmrcode.c\n",
        "scalar_zero": "new xfmrcode.c xhl=0\n",
    }
    return {
        k: "clear\nnew circuit.c\n" + v + "new transformer.final xfmrcode=c\n"
        for k, v in cases.items()
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    observations = {}
    for name, script in cases().items():
        engine = dss.NewContext()
        for flag in ("AllowChangeDir", "AllowDOScmd", "AllowEditor", "AllowForms"):
            getattr(engine.Basic, flag)(False)
        if LOCK["dss"]["version"] not in engine.Basic.Version():
            raise ValueError("unpinned engine")
        engine.Basic.DataPath(str(args.output.resolve()))
        engine.Text.Commands(script)
        transformers = {}
        for transformer in engine.Transformers.AllNames():
            engine.Transformers.Name(transformer)
            windings = []
            for i in range(1, engine.Transformers.NumWindings() + 1):
                engine.Transformers.Wdg(i)
                windings.append(
                    dict(
                        kv=engine.Transformers.kV(),
                        kva=engine.Transformers.kVA(),
                        tap=engine.Transformers.Tap(),
                        r=engine.Transformers.R(),
                        rneut=engine.Transformers.Rneut(),
                        xneut=engine.Transformers.Xneut(),
                        delta=bool(engine.Transformers.IsDelta()),
                    )
                )
            engine.Circuit.SetActiveElement("transformer." + transformer)
            xsc = re.split(
                r"[,\s]+", engine.Properties.Value("xscarray").strip("[]() ")
            )
            transformers[transformer] = dict(
                phases=engine.CktElement.NumPhases(),
                windings=windings,
                xsc=[float(v) for v in xsc if v.strip()],
                extras={
                    k: float(engine.Properties.Value(k))
                    for k in ("%imag", "%noloadloss", "ppm_antifloat")
                },
            )
        observations[name] = dict(
            script=script, engine=engine.Basic.Version(), transformers=transformers
        )
    (args.output / "observations.json").write_text(
        json.dumps(observations, indent=2) + "\n"
    )
    print(f"Captured {len(observations)} cases; run the Rust comparison separately.")


if __name__ == "__main__":
    main()
