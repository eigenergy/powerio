"""Independent MATPOWER makePTDF/makeLODF oracle, all three susceptance
formulas.

Case read with matpowercaseframes (pinned in
evals/validation/requirements.txt), not powerio; makePTDF and makeLODF below
are transcribed from the published MATPOWER source, never from powerio's own
incidence or susceptance construction. Fixtures are case118.m and case30.m:
both are small enough to form the dense n x n sensitivity matrices directly,
and their identity mapping is MATPOWER's own BUS_I bus order plus the input
order of in-service branches. The oracle checks both mappings. The large real
grid fixtures used by validate_dc_matpower.py for the shift sign leg are
skipped here — a dense PTDF over 10000+ buses is not worth the wall time, and
case118/case30
exercise every formula and the bridge branch case already.

Every PTDF and LODF entry is asserted at ABS_TOL below; the largest error
observed while writing this oracle was 3e-13.

Sign trap: a bridge branch (removing it islands the network) has an
undefined LODF by the raw formula L = Hh / (1 - h): division by (1 - h) with
h = 1 blows up. powerio's documented rule classifies a column as a bridge by
|1 - h| < LODF_ISLAND_TOLERANCE (1e-9) and reports the whole column as zero
off the diagonal, keeping the structural -1 self term every LODF diagonal
carries by definition. This oracle classifies bridge columns the same way —
never by checking whether the raw formula's output is finite, which is
fragile (a near bridge can produce a large but finite value that is just as
meaningless) — excludes those columns from the value comparison against the
oracle's own (otherwise undefined) numbers, and separately asserts powerio's
structural convention on them directly.
"""

import os
import sys

import numpy as np
import scipy.sparse as sp
from matpowercaseframes import CaseFrames

import powerio

ABS_TOL = 1e-9
# powerio's own bridge column classification tolerance (LODF_ISLAND_TOLERANCE);
# duplicated here so the oracle classifies the same columns powerio does.
LODF_ISLAND_TOLERANCE = 1e-9


def build(cf, formula):
    bus, br = cf.bus, cf.branch
    nb = len(bus)
    bus_i = bus["BUS_I"].astype(np.int64).to_numpy()
    e2i = {int(b): k for k, b in enumerate(bus_i)}
    f = np.array([e2i[int(v)] for v in br["F_BUS"].astype(np.int64).to_numpy()])
    t = np.array([e2i[int(v)] for v in br["T_BUS"].astype(np.int64).to_numpy()])
    r = br["BR_R"].astype(float).to_numpy()
    x = br["BR_X"].astype(float).to_numpy()
    tap = br["TAP"].astype(float).to_numpy().copy()
    tap[tap == 0.0] = 1.0
    stat = br["BR_STATUS"].astype(float).to_numpy() != 0.0
    if formula == "series":  # +x/(r^2+x^2) = -imag(inv(r+jx))
        b = x / (r * r + x * x)
    elif formula == "matpower":  # 1/(x*tap)
        b = 1.0 / (x * tap)
    elif formula == "reactance_only":  # 1/x
        b = 1.0 / x
    else:
        raise ValueError(f"unknown formula {formula!r}")
    b = np.where(stat, b, 0.0)
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
    refs = np.where(bus["BUS_TYPE"].astype(int).to_numpy() == 3)[0]
    return dict(
        nb=nb,
        nl=nl,
        f=f,
        t=t,
        b=b,
        stat=stat,
        Bf=Bf.toarray(),
        Bbus=Bbus.toarray(),
        refs=refs,
    )


def ptdf(m):
    nb, Bf, Bbus, refs = m["nb"], m["Bf"], m["Bbus"], m["refs"]
    noslack = np.setdiff1d(np.arange(nb), refs)
    H = np.zeros_like(Bf)
    H[:, noslack] = np.linalg.solve(
        Bbus[np.ix_(noslack, noslack)].T, Bf[:, noslack].T
    ).T
    return H


def lodf(m, H):
    nl, nb = m["nl"], m["nb"]
    Cft = sp.csr_matrix(
        (
            np.concatenate([np.ones(nl), -np.ones(nl)]),
            (np.concatenate([m["f"], m["t"]]), np.concatenate([np.arange(nl), np.arange(nl)])),
        ),
        shape=(nb, nl),
    ).toarray()
    Hh = H @ Cft
    h = np.diag(Hh).copy()
    with np.errstate(divide="ignore", invalid="ignore"):
        # A bridge branch (h == 1) divides by zero here; that column is nan/inf
        # by construction and is excluded from the value comparison below by
        # powerio's own tolerance rule, not by this warning suppression.
        L = Hh / (1.0 - np.ones((nl, 1)) @ h[None, :])
        L = L - np.diag(np.diag(L)) - np.eye(nl)
    return L, h


def append_result(case: str, leg: str, mark: str) -> None:
    out = os.environ.get("PIO_RESULTS_TSV")
    if out:
        with open(out, "a", encoding="utf-8") as fh:
            fh.write(f"{case}\t{leg}\t{mark}\n")


def compare(path: str) -> list[str]:
    """Run PTDF and LODF for all three formulas; return failure messages."""
    failures: list[str] = []
    print(f"\n===== {path}")
    cf = CaseFrames(path)
    module = powerio.parse(path)
    net = module.value
    pio_bus_ids = np.array([int(bus["id"]) for bus in net.buses])
    expected_bus_ids = cf.bus["BUS_I"].astype(np.int64).to_numpy()
    if not np.array_equal(pio_bus_ids, expected_bus_ids):
        return ["bus axis differs from MATPOWER's BUS_I order"]

    active_idx = np.flatnonzero(
        cf.branch["BR_STATUS"].astype(float).to_numpy() != 0.0
    )
    active_branches = [branch for branch in net.branches if branch["in_service"]]
    pio_endpoints = np.array(
        [
            [int(branch["from_id"]), int(branch["to_id"])]
            for branch in active_branches
        ]
    )
    expected_endpoints = cf.branch[["F_BUS", "T_BUS"]].astype(np.int64).to_numpy()[
        active_idx
    ]
    if not np.array_equal(pio_endpoints, expected_endpoints):
        return ["active branch axis differs from MATPOWER branch order"]

    for formula, oracle_name in [
        ("series_susceptance", "series"),
        ("tap_adjusted_reactance", "matpower"),
        ("reactance_only", "reactance_only"),
    ]:
        m = build(cf, oracle_name)
        H = ptdf(m)
        L, hdiag = lodf(m, H)
        P = net.calc_ptdf(formula).toarray()
        Lp = net.calc_lodf(formula).toarray()
        dP = np.max(np.abs(P - H[active_idx, :]))

        # Bridge columns: classify by powerio's own tolerance, exclude them
        # from the value comparison (the oracle's raw formula is undefined
        # there), and separately check powerio's structural convention.
        denom = np.abs(1.0 - hdiag[active_idx])
        good = denom >= LODF_ISLAND_TOLERANCE
        Lo = L[np.ix_(active_idx, active_idx)]
        dL = np.max(np.abs(Lp[:, good] - Lo[:, good])) if good.any() else 0.0
        bad_idx = np.where(~good)[0]
        max_off_diag = 0.0
        max_diag_err = 0.0
        if bad_idx.size:
            sub = Lp[:, bad_idx].copy()
            diag_vals = Lp[bad_idx, bad_idx]
            sub[bad_idx, np.arange(bad_idx.size)] = 0.0
            max_off_diag = float(np.max(np.abs(sub)))
            max_diag_err = float(np.max(np.abs(diag_vals - (-1.0))))

        print(
            f"  {formula:<24} PTDF diff={dP:.3e}   LODF diff={dL:.3e}"
            f"   bridge cols={bad_idx.size} (off diag<={max_off_diag:.1e},"
            f" diag err={max_diag_err:.1e})"
            f"   min|1-h| kept={denom[good].min() if good.any() else float('nan'):.3e}"
        )
        if dP >= ABS_TOL:
            failures.append(f"{formula}: ptdf diff {dP:.3e} >= {ABS_TOL:.0e}")
        if dL >= ABS_TOL:
            failures.append(f"{formula}: lodf diff {dL:.3e} >= {ABS_TOL:.0e}")
        if max_off_diag >= ABS_TOL:
            failures.append(
                f"{formula}: lodf bridge column off diagonal {max_off_diag:.3e}"
                f" >= {ABS_TOL:.0e} (should be zeroed)"
            )
        if max_diag_err >= ABS_TOL:
            failures.append(
                f"{formula}: lodf bridge column diagonal off by {max_diag_err:.3e}"
                " (should stay the structural -1)"
            )

    return failures


def main() -> int:
    paths = sys.argv[1:]
    if not paths:
        print("usage: validate_dc_ptdf_lodf.py <case.m> [case.m ...]", file=sys.stderr)
        return 1
    exit_code = 0
    for path in paths:
        try:
            failures = compare(path)
        except Exception as err:
            failures = [str(err)]
        mark = "ok" if not failures else "FAIL"
        append_result(path, "dc_ptdf_lodf", mark)
        if failures:
            exit_code = 1
            print(f"  FAILED: {path}")
            for failure in failures:
                print(f"    {failure}")
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
