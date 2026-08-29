"""Independent MATPOWER makeBdc oracle.

Checks powerio's DC surface — dc_data susceptance/shift/shift injection and
weighted_laplacian — against MATPOWER's own makeBdc, plus one PTDF spot check
against makePTDF. The case is read with matpowercaseframes (pinned in
evals/validation/requirements.txt), not powerio, and the formulas below are
transcribed from the published MATPOWER source rather than derived from
powerio's incidence or susceptance construction, so a bug shared between the
reader and the matrix builder cannot cancel out in this comparison.

Fixtures: case118.m and case30.m exercise the base comparison (susceptance,
shift, shift injection, weighted_laplacian, one PTDF check); their identity
mapping is MATPOWER's own BUS_I bus order and a `branches:<i>` row order that
dc_data reports directly, so no separate reindexing table is needed. Neither
case has a phase shifting transformer, so the shift sign is only exercised on
a real sized grid with shifters: tests/data/large/case13659pegase.m or
case_ACTIVSg10k.m (gitignored; fetch with evals/validation/fetch_cases.sh).
PTDF is skipped on that case — forming the dense n x n sensitivity matrix
for 10000+ buses is not worth the wall time here, and the makeBdc/shift
checks alone are what the large case is for.

Every quantity is asserted at ABS_TOL below; the largest error observed
while writing this oracle was 3e-13, three orders of magnitude inside the
gate.

Sign trap: powerio's weighted_laplacian('matpower') equals +Bbus from
makeBdc, not -Bbus. makeBdc's b = stat / x / tap, and for an ordinary
positive reactance branch that is already a positive quantity; only the
public series susceptance (the PowerModels convention, checked above it)
carries the sign flip -imag(1/(r+jx)). The diagonal is printed as an
informational diag>0 flag, not asserted: case13659pegase.m carries 16
branches with a negative BR_X (equivalent series capacitive elements from
the network reduction that produced it), which can and does make a low
degree bus's diagonal entry negative in both MATPOWER's own Bbus and
powerio's Laplacian alike. That is a fact about the case, not a
disagreement between the two, and case118/case30 (all positive reactances)
show diag>0 true.
"""

import os
import sys

import numpy as np
import scipy.sparse as sp
from matpowercaseframes import CaseFrames

import powerio

F_BUS, T_BUS = "F_BUS", "T_BUS"
ABS_TOL = 1e-9


def matpower_dc(cf):
    bus = cf.bus
    br = cf.branch
    nb = len(bus)
    bus_i = bus["BUS_I"].astype(np.int64).to_numpy()
    e2i = {int(b): k for k, b in enumerate(bus_i)}
    stat = br["BR_STATUS"].astype(float).to_numpy()
    keep = stat != 0.0
    f = np.array([e2i[int(v)] for v in br[F_BUS].astype(np.int64).to_numpy()])
    t = np.array([e2i[int(v)] for v in br[T_BUS].astype(np.int64).to_numpy()])
    x = br["BR_X"].astype(float).to_numpy()
    tap = br["TAP"].astype(float).to_numpy().copy()
    tap[tap == 0.0] = 1.0
    shift = br["SHIFT"].astype(float).to_numpy()
    # makeBdc: b = stat ./ x ./ tap
    b = np.where(keep, 1.0 / x, 0.0) / tap
    nl = len(b)
    i = np.concatenate([np.arange(nl), np.arange(nl)])
    Cft = sp.csr_matrix(
        (np.concatenate([np.ones(nl), -np.ones(nl)]), (i, np.concatenate([f, t]))),
        shape=(nl, nb),
    )
    Bf = sp.csr_matrix(
        (np.concatenate([b, -b]), (i, np.concatenate([f, t]))), shape=(nl, nb)
    )
    Bbus = (Cft.T @ Bf).tocsr()
    Pfinj = b * (-shift * np.pi / 180.0)
    Pbusinj = np.asarray(Cft.T @ Pfinj).ravel()
    return dict(
        nb=nb,
        f=f,
        t=t,
        b=b,
        keep=keep,
        shift=shift,
        Cft=Cft,
        Bf=Bf,
        Bbus=Bbus,
        Pfinj=Pfinj,
        Pbusinj=Pbusinj,
        bus_i=bus_i,
        bus_type=bus["BUS_TYPE"].astype(int).to_numpy(),
    )


def matpower_ptdf(mp):
    """makePTDF with the reference slack set (MATPOWER's single slack form)."""
    nb, Bbus, Bf = mp["nb"], mp["Bbus"].toarray(), mp["Bf"].toarray()
    refs = np.where(mp["bus_type"] == 3)[0]
    noslack = np.setdiff1d(np.arange(nb), refs)
    H = np.zeros_like(Bf)
    H[:, noslack] = np.linalg.solve(
        Bbus[np.ix_(noslack, noslack)].T, Bf[:, noslack].T
    ).T
    return H


def append_result(case: str, leg: str, mark: str) -> None:
    out = os.environ.get("PIO_RESULTS_TSV")
    if out:
        with open(out, "a", encoding="utf-8") as fh:
            fh.write(f"{case}\t{leg}\t{mark}\n")


def compare(path: str, want_ptdf: bool = True) -> list[str]:
    """Run every check for one case; return its failure messages (empty on a pass)."""
    failures: list[str] = []
    print(f"\n===== {path}")
    cf = CaseFrames(path)
    mp = matpower_dc(cf)
    net = powerio.parse(path).as_balanced_network()

    pio_bus_ids = np.array([int(s) for s in net.dc_data()["bus_ids"]])
    if not np.array_equal(pio_bus_ids, mp["bus_i"]):
        failures.append("bus axis differs from MATPOWER's BUS_I order")
        return failures
    print(f"  bus axis matches ({len(pio_bus_ids)} buses)")

    for pio_formula, formula_b in [
        ("tap_adjusted_reactance", mp["b"]),
        ("series_susceptance", None),
    ]:
        d = net.dc_data(pio_formula)
        rows = d["row_ids"]
        idx = np.array([int(r.split(":")[1]) for r in rows])  # branches:<i>
        sus = np.asarray(d["susceptance"], float)
        sh = np.asarray(d["shift"], float)
        inj = np.asarray(d["shift_injection"], float)
        if formula_b is None:
            r = cf.branch["BR_R"].astype(float).to_numpy()
            x = cf.branch["BR_X"].astype(float).to_numpy()
            series_b = x / (r * r + x * x)  # -imag(inv(r+jx)) = +x/(r^2+x^2)
        else:
            series_b = formula_b
        exp_b = -series_b[idx]  # PowerModels sign = -MATPOWER b
        db = np.max(np.abs(sus - exp_b) / np.maximum(1.0, np.abs(exp_b)))
        exp_sh = mp["shift"][idx] * np.pi / 180.0
        dsh = np.max(np.abs(sh - exp_sh)) if len(sh) else 0.0
        # bus injection: MATPOWER Pbusinj with the SAME b the formula uses
        Pfinj = series_b[idx] * (-exp_sh)
        Pbus = np.zeros(mp["nb"])
        np.add.at(Pbus, np.asarray(d["from_indices"], int), Pfinj)
        np.add.at(Pbus, np.asarray(d["to_indices"], int), -Pfinj)
        dinj = np.max(np.abs(inj - Pbus))
        nshift = int((np.abs(sh) > 0).sum())
        print(
            f"  {pio_formula:<24} rows={len(rows):<6} omitted={len(d['omitted_ids']):<4} "
            f"shifted={nshift:<4} max|Pbusinj|={np.max(np.abs(Pbus)):.4f}  "
            f"max rel db={db:.3e}  max dshift={dsh:.3e}  max dPbusinj={dinj:.3e}"
        )
        if db >= ABS_TOL:
            failures.append(f"{pio_formula}: susceptance diff {db:.3e} >= {ABS_TOL:.0e}")
        if dsh >= ABS_TOL:
            failures.append(f"{pio_formula}: shift diff {dsh:.3e} >= {ABS_TOL:.0e}")
        if dinj >= ABS_TOL:
            failures.append(
                f"{pio_formula}: shift injection diff {dinj:.3e} >= {ABS_TOL:.0e}"
            )

    # Sign trap: see the module docstring. makeBdc's b is already positive,
    # so the matpower formula's weighted Laplacian equals +Bbus, not -Bbus.
    L = net.weighted_laplacian("matpower").toarray()
    dL = np.max(np.abs(L - mp["Bbus"].toarray()))
    diag_positive = bool((np.diag(L) > 0).all())
    print(
        f"  weighted_laplacian('matpower') vs +makeBdc Bbus: max abs diff = {dL:.3e}"
        f"   (diag>0: {diag_positive})"
    )
    if dL >= ABS_TOL:
        failures.append(f"weighted_laplacian: diff {dL:.3e} >= {ABS_TOL:.0e}")

    if want_ptdf:
        H = matpower_ptdf(mp)
        P = net.ptdf("matpower").toarray()
        d = net.dc_data("tap_adjusted_reactance")
        idx = np.array([int(r.split(":")[1]) for r in d["row_ids"]])
        dP = np.max(np.abs(P - H[idx, :]))
        print(f"  ptdf('matpower') vs makePTDF: max abs diff = {dP:.3e}  shape {P.shape}")
        if dP >= ABS_TOL:
            failures.append(f"ptdf: diff {dP:.3e} >= {ABS_TOL:.0e}")

    return failures


def main() -> int:
    paths = sys.argv[1:]
    if not paths:
        print("usage: validate_dc_matpower.py <case.m> [case.m ...]", file=sys.stderr)
        return 1
    exit_code = 0
    for path in paths:
        # The large real grid fixtures are the shift sign leg only: forming
        # the dense PTDF is not worth the wall time at their bus count.
        want_ptdf = not any(tag in path for tag in ("13659", "10k", "SyntheticUSA"))
        try:
            failures = compare(path, want_ptdf=want_ptdf)
        except Exception as err:  # noqa: BLE001
            failures = [str(err)]
        mark = "ok" if not failures else "FAIL"
        append_result(path, "dc_makebdc", mark)
        if failures:
            exit_code = 1
            print(f"  FAILED: {path}")
            for failure in failures:
                print(f"    {failure}")
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
