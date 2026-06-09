"""asv suite: powerio's own parse and matrix performance across git history.

This tracks self-regression over commits (does a change make powerio slower than its
past?), which the cross-tool snapshot in benchmarks/RESULTS.md does not: that compares
powerio against ExaPowerIO/PowerModels/pandapower at one commit. The Rust hot path also
has criterion coverage (powerio/benches/parse.rs); this watches the user-facing Python
wheel.

Run from benchmarks/asv/ — see README.md. asv builds the wheel with maturin per commit
(asv.conf.json build_command), so a Rust toolchain must be on PATH.
"""

from pathlib import Path

import powerio

# benchmarks/asv/benchmarks/benchmarks.py -> repo root is four parents up.
CASE = str(Path(__file__).resolve().parents[3] / "tests" / "data" / "case2869pegase.m")


class Parse:
    def time_parse(self):
        powerio.parse_file(CASE)


class Matrices:
    def setup(self):
        self.case = powerio.parse_file(CASE)

    def time_ybus(self):
        self.case.ybus()

    def time_bprime(self):
        self.case.bprime()
