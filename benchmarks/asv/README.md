# asv: powerio parse performance over time

[airspeed velocity](https://asv.readthedocs.io) tracks powerio's Python parse and
matrix timings across git history and renders a dashboard. This answers "did we
regress against our past?", while `benchmarks/RESULTS.md` compares several
parsers at one commit. It runs locally, not in CI, because
absolute timings need a quiet machine.

`asv.conf.json` builds the wheel with maturin per commit, so a Rust toolchain must be on
PATH.

```
pip install asv
cd benchmarks/asv
asv check -E existing:../../.venv/bin/python
asv run --quick --show-stderr -E existing:../../.venv/bin/python --dry-run
asv run main^!          # benchmark the current commit
asv run main~10..main   # or a range of history
asv publish && asv preview
```

The `existing` commands validate the benchmark package and do one unsaved smoke
run against the local editable wheel, useful before committing changes. The
first real `asv run` is also what validates the maturin `build_command`; if the
wheel build needs a tweak for your platform, it surfaces there.
