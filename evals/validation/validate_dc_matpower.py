"""Independent MATPOWER makeBdc oracle.

Checks PowerIO's incidence, bus and branch susceptance matrices, phase shift
injection, branch flow, weighted Laplacian, and PTDF against MATPOWER's own
makeBdc and makePTDF. The case is read with matpowercaseframes (pinned in
evals/validation/requirements.txt), not powerio, and the formulas below are
transcribed from the published MATPOWER source rather than derived from
powerio's incidence or susceptance construction, so a bug shared between the
reader and the matrix builder cannot cancel out in this comparison.

Fixtures: case118.m and case30.m exercise the base comparison (susceptance,
shift, shift injection, weighted Laplacian, one PTDF check). Matrix rows use
the input order of in-service branches and columns use the parsed BUS_I order;
the oracle checks both mappings before comparing values. Neither small case
has a phase shifting transformer, so the shift sign is also exercised on a
real sized grid with shifters: tests/data/large/case13659pegase.m or
case_ACTIVSg10k.m (gitignored; fetch with evals/validation/fetch_cases.sh).
PTDF is skipped on that case because forming the dense n x n sensitivity matrix
for 10000+ buses is not worth the wall time here, and the makeBdc/shift
checks alone are what the large case is for.

Every quantity is asserted at ABS_TOL below; the largest error observed
while writing this oracle was 3e-13, three orders of magnitude inside the
gate.

Sign trap: PowerIO's weighted Laplacian with `tap_adjusted_reactance` equals
+Bbus from makeBdc, not -Bbus. makeBdc's b = stat / x / tap, and for an ordinary
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
    module = powerio.parse(path)
    net = module.value

    pio_bus_ids = np.array([int(bus["id"]) for bus in net.buses])
    if not np.array_equal(pio_bus_ids, mp["bus_i"]):
        failures.append("bus axis differs from MATPOWER's BUS_I order")
        return failures
    print(f"  bus axis matches ({len(pio_bus_ids)} buses)")

    idx = np.flatnonzero(mp["keep"])
    active_branches = [branch for branch in net.branches if branch["in_service"]]
    pio_from = np.array([int(branch["from_id"]) for branch in active_branches])
    pio_to = np.array([int(branch["to_id"]) for branch in active_branches])
    expected_from = mp["bus_i"][mp["f"][idx]]
    expected_to = mp["bus_i"][mp["t"][idx]]
    if not (
        np.array_equal(pio_from, expected_from)
        and np.array_equal(pio_to, expected_to)
    ):
        failures.append("active branch axis differs from MATPOWER branch order")
        return failures
    print(f"  branch axis matches ({len(idx)} in-service branches)")

    for pio_formula, formula_b in [
        ("tap_adjusted_reactance", mp["b"]),
        ("series_susceptance", None),
    ]:
        incidence = net.calc_incidence_matrix(pio_formula).toarray()
        branch_matrix = net.calc_branch_susceptance_matrix(pio_formula).toarray()
        bus_matrix = net.calc_bus_susceptance_matrix(pio_formula).toarray()
        inj = np.asarray(net.calc_phase_shift_injection(pio_formula), float)
        branch_flow = np.asarray(
            net.calc_branch_flow_dc(np.zeros(mp["nb"]), pio_formula), float
        )
        bus_injection = np.asarray(
            net.calc_bus_injection_dc(np.zeros(mp["nb"]), pio_formula), float
        )
        if formula_b is None:
            r = cf.branch["BR_R"].astype(float).to_numpy()
            x = cf.branch["BR_X"].astype(float).to_numpy()
            series_b = x / (r * r + x * x)  # -imag(inv(r+jx)) = +x/(r^2+x^2)
        else:
            series_b = formula_b
        exp_b = -series_b[idx]  # PowerModels sign = -MATPOWER b
        expected_incidence = mp["Cft"][idx, :].toarray()
        expected_branch_matrix = exp_b[:, None] * expected_incidence
        expected_bus_matrix = expected_incidence.T @ expected_branch_matrix
        b_from_matrix = branch_matrix[np.arange(len(idx)), mp["f"][idx]]
        dinc = np.max(np.abs(incidence - expected_incidence))
        dbf = np.max(np.abs(branch_matrix - expected_branch_matrix))
        dbus = np.max(np.abs(bus_matrix - expected_bus_matrix))
        sus = b_from_matrix
        db = np.max(np.abs(sus - exp_b) / np.maximum(1.0, np.abs(exp_b)))
        exp_shift = mp["shift"][idx] * np.pi / 180.0
        expected_branch_flow = exp_b * exp_shift
        expected_injection = expected_incidence.T @ expected_branch_flow
        dflow = np.max(np.abs(branch_flow - expected_branch_flow))
        dinj = np.max(np.abs(inj - expected_injection))
        dbusinj = np.max(np.abs(bus_injection - expected_injection))
        nshift = int((np.abs(exp_shift) > 0).sum())
        print(
            f"  {pio_formula:<24} rows={len(idx):<6} shifted={nshift:<4} "
            f"dA={dinc:.3e} dBf={dbf:.3e} dB={dbus:.3e} "
            f"db={db:.3e} dflow={dflow:.3e} dPshift={dinj:.3e} "
            f"dPbus={dbusinj:.3e}"
        )
        if dinc >= ABS_TOL:
            failures.append(
                f"{pio_formula}: incidence diff {dinc:.3e} >= {ABS_TOL:.0e}"
            )
        if dbf >= ABS_TOL:
            failures.append(
                f"{pio_formula}: branch matrix diff {dbf:.3e} >= {ABS_TOL:.0e}"
            )
        if dbus >= ABS_TOL:
            failures.append(
                f"{pio_formula}: bus matrix diff {dbus:.3e} >= {ABS_TOL:.0e}"
            )
        if db >= ABS_TOL:
            failures.append(
                f"{pio_formula}: susceptance diff {db:.3e} >= {ABS_TOL:.0e}"
            )
        if dflow >= ABS_TOL:
            failures.append(
                f"{pio_formula}: branch flow diff {dflow:.3e} >= {ABS_TOL:.0e}"
            )
        if dinj >= ABS_TOL:
            failures.append(
                f"{pio_formula}: shift injection diff {dinj:.3e} >= {ABS_TOL:.0e}"
            )
        if dbusinj >= ABS_TOL:
            failures.append(
                f"{pio_formula}: bus injection diff {dbusinj:.3e} >= {ABS_TOL:.0e}"
            )

    # Sign trap: see the module docstring. makeBdc's b is already positive,
    # so the matpower formula's weighted Laplacian equals +Bbus, not -Bbus.
    L = net.calc_weighted_laplacian("tap_adjusted_reactance").toarray()
    dL = np.max(np.abs(L - mp["Bbus"].toarray()))
    diag_positive = bool((np.diag(L) > 0).all())
    print(
        "  calc_weighted_laplacian('tap_adjusted_reactance') vs +makeBdc Bbus: "
        f"max abs diff = {dL:.3e}"
        f"   (diag>0: {diag_positive})"
    )
    if dL >= ABS_TOL:
        failures.append(f"weighted Laplacian: diff {dL:.3e} >= {ABS_TOL:.0e}")

    if want_ptdf:
        H = matpower_ptdf(mp)
        P = net.calc_ptdf("tap_adjusted_reactance").toarray()
        dP = np.max(np.abs(P - H[idx, :]))
        print(
            "  calc_ptdf('tap_adjusted_reactance') vs makePTDF: "
            f"max abs diff = {dP:.3e}  shape {P.shape}"
        )
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
        except Exception as err:
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
