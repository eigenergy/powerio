# asv: powerio parse performance over time

[airspeed velocity](https://asv.readthedocs.io) tracks powerio's own Python parse and
matrix timings across git history and renders a dashboard. This answers "did we regress
vs our past?", which the cross-tool table in [../RESULTS.md](../RESULTS.md) does not (it
compares powerio against other parsers at one commit). It runs locally, not in CI —
absolute timings need a quiet machine, the same reason the speed tables aren't gated in
CI. For a noise-robust PR check, the Julia binding uses AirspeedVelocity.jl's
same-runner PR-vs-base comparison instead.

`asv.conf.json` builds the wheel with maturin per commit, so a Rust toolchain must be on
PATH.

```
pip install asv
cd benchmarks/asv
asv run main^!          # benchmark the current commit
asv run main~10..main   # or a range of history
asv publish && asv preview
```

The first `asv run` is also what validates the maturin `build_command`; if the wheel
build needs a tweak for your platform, it surfaces there.
