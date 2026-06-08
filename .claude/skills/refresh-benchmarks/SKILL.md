---
name: refresh-benchmarks
description: >-
  Re-run the local speed benchmarks (Julia + Python) and the correctness
  validators, regenerate the marker-delimited speed tables in README.md and
  benchmarks/RESULTS.md from machine-readable JSON, and show the diff. Heavy,
  side-effecting, and machine-specific — run it on the benchmarking laptop, not in
  CI. Does not commit.
disable-model-invocation: true
---

# refresh-benchmarks

Stop hand-copying benchmark numbers into two files. The bench scripts emit JSON;
`render_tables.py` rewrites only the `<!-- BENCH:* -->` table regions from it.

Scope: the three speed tables (`README.md` BENCH:speed-main, `benchmarks/RESULTS.md`
BENCH:speed-julia and BENCH:speed-pandapower). The correctness matrix and the
version block in RESULTS.md are hand-maintained — call them out if they changed, but
don't rewrite them. The numbers are per-machine, so this never runs in CI; CI gates
only correctness (`run_validation.sh`, exit code).

## Steps

1. **Prereqs.** Confirm the build inputs exist; offer to run any that are missing
   before doing heavy installs:
   - `cargo build --release -p powerio-capi` (the C ABI the Julia bench calls)
   - `maturin develop --release` (the `powerio` wheel for the Python bench)
   - `julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'`
   - `pip install -r benchmarks/requirements.txt` (pandapower + egret oracles)

2. **Large cases.** The speed tables cover cases up to 54 MB / 192768 buses that
   live in gitignored `tests/data/large/`. Offer to run `bash benchmarks/fetch_cases.sh`.
   If the user declines, the missing cases are skipped and `render_tables.py` leaves
   any table that needs them unchanged (with a warning) rather than shrinking it.

3. **Run the producers with `--json`:**
   ```
   julia --project=benchmarks benchmarks/bench_julia.jl --json
   python benchmarks/bench_parse.py --json \
       tests/data/case2869pegase.m \
       tests/data/large/case9241pegase.m \
       tests/data/large/case13659pegase.m \
       tests/data/large/case193k.m
   ```
   They write `benchmarks/results/{speed_julia,speed_python}.json` (gitignored).

4. **Regenerate the tables:** `python benchmarks/render_tables.py`. It rewrites only
   the marked regions; prose outside them is untouched.

5. **Re-run correctness:** `bash benchmarks/run_validation.sh`. Report the pass/fail
   summary. A FAIL is a regression — surface it prominently.

6. **Report, don't commit.** `git diff -- README.md benchmarks/RESULTS.md`, summarize
   what moved, and note if the hand-maintained correctness matrix or version block in
   RESULTS.md now looks stale (new fixture, changed tool version). Leave the changes
   staged for the user to review; do not commit or push.
