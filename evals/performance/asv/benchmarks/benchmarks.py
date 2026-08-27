"""asv suite: powerio's own parse and matrix performance across git history.

This tracks self-regression over commits (does a change make powerio slower than its
past?), which the cross-tool snapshot in evals/performance/RESULTS.md does not: that compares
powerio against ExaPowerIO/PowerModels/pandapower at one commit. The Rust hot path also
has criterion coverage (powerio/benches/parse.rs); this watches the user-facing Python
wheel.

Run from evals/performance/asv/ — see README.md. asv builds the wheel with maturin per commit
(asv.conf.json build_command), so a Rust toolchain must be on PATH.
"""

from pathlib import Path

import powerio

# This file sits four levels below the repo root, at
# evals/performance/asv/benchmarks/benchmarks.py. That inner benchmarks/ name
# is fixed by asv.conf.json's benchmark_dir setting; do not rename it.
CASE = str(Path(__file__).resolve().parents[4] / "tests" / "data" / "case2869pegase.m")


class Parse:
    def time_parse(self):
        powerio.parse(CASE, value_type=powerio.BalancedNetwork)


class Matrices:
    def setup(self):
        self.case = powerio.parse(CASE, value_type=powerio.BalancedNetwork)

    def time_ybus(self):
        self.case.ybus()

    def time_bprime(self):
        self.case.bprime()
