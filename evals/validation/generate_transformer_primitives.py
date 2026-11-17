"""Synthetic OpenDSS 0.9.4 terminal admittances; no external feeder data."""

import json
from pathlib import Path

import opendssdirect as dss

cases = []
for conn in ["wye wye", "wye delta", "wye wye delta", "delta wye wye"]:
    for ppm in [None, 0, 1, 50, -50]:
        n = len(conn.split())
        buses = ["src.1.2.3.4", "load.1.2.3.4", "third.1.2.3.4"][:n]
        source = f"clear\nnew circuit.test basekv=11\nnew transformer.t phases=3 windings={n} buses=[{' '.join(buses)}] conns=[{conn}] kvs=[{' '.join(['11', '.4', '4.16'][:n])}] kvas=[{' '.join(['1000'] * n)}] taps=[{' '.join(['1.025', '.975', '1.01'][:n])}] %rs=[{' '.join(['.2', '.3', '.4'][:n])}] xhl=4 xht=5 xlt=6 ppm_antifloat={ppm}\n"
        if ppm is None:
            source = source.replace(" ppm_antifloat=None", "")
        ctx = dss.NewContext()
        for line in source.splitlines():
            ctx.Text.Command(line)
        ctx.Text.Command("set controlmode=off")
        ctx.Solution.Solve()
        ctx.Circuit.SetActiveElement("transformer.t")
        nodes = ctx.CktElement.NodeOrder()
        counts = ctx.CktElement.NumConductors()
        names = ctx.CktElement.BusNames()
        labels = [
            f"{names[i // counts].split('.')[0]}.{v}" for i, v in enumerate(nodes)
        ]
        matrix = ctx.CktElement.YPrim()
        size = len(labels)
        # Ignore ground and unused delta-neutral rows; combine shared physical terminals.
        active = sorted(set(x for x in labels if not x.endswith(".0")))
        out = [[[0.0, 0.0] for _ in active] for _ in active]
        for i, label in enumerate(labels):
            if label not in active:
                continue
            for j, other in enumerate(labels):
                if other not in active:
                    continue
                for k in [0, 1]:
                    out[active.index(label)][active.index(other)][k] += matrix[
                        2 * (j * size + i) + k
                    ]
        cases.append(
            dict(
                name=f"{conn.replace(' ', '-')}-{ppm}",
                source=source,
                nodes=active,
                matrix=out,
            )
        )
p = (
    Path(__file__).resolve().parents[2]
    / "powerio-dist/tests/data/transformer_primitives.json"
)
p.parent.mkdir(exist_ok=True)
p.write_text(
    json.dumps(dict(engine=dss.Basic.Version(), cases=cases), separators=(",", ":"))
    + "\n"
)
print(p.stat().st_size)
