# Handoff: PowerFactory DGS reader

Branch `agent/powerfactory-dgs`, closes #150. The worker that wrote this branch was stopped by a usage limit before it could run every gate, so the state below is what a recovery pass measured, not what the worker claimed.

## Measured on 2026-09-02

- `cargo check -p powerio-tx -p powerio`: passes.
- `cargo test -p powerio-tx --test dgs`: 13 passed. `powerio/tests/dgs.rs` and `evals/powsybl/check_dgs.py` exist but were not run.
- Not yet run on this branch: `cargo test -p powerio-tx -p powerio -p powerio-cli` in full, `RUSTUP_TOOLCHAIN=1.98.0 cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check`, the Python format list test (`python/tests/test_powerio.py`), `mdbook build docs`, and the PowSybl gate (`bash evals/powsybl/run.sh <binary> <PyPowSybl 1.16.1 python> <powsybl-core worktree at 0939bfcc>`).

## Next steps

1. Rebase onto the head of the stack (`origin/agent/ir-reference` or whichever branch is now last; see the release stack issue) and resolve conflicts in shared files (`CHANGELOG.md`, `powerio/src/formats.rs`, `powerio-tx/src/format/routing.rs`, `powerio-tx/src/format/mod.rs`, `powerio-cli/src/main.rs`, `docs/src/format-fidelity.md`).
2. Run every gate above and fix what fails.
3. Split this single work-in-progress commit into atomic commits (reader, fixtures and tests, gate cases, docs) with `git reset --soft` and re-commit; keep the co-author trailers.
4. Open the PR stacked on the branch below it in the stack, body stating why it lands before 1.0.0 (the closed issue's body has the reason), `Closes #150`.
5. Add the format to the conversion matrix baseline (`powerio-cli/tests/conversion_matrix_report.rs`) and to the PowerIO.jl format tests.

Wording rules for anything added: plain technical English; no "roundtrip", "byte exact", "source bytes", "echo", "rooted", "envelope", "contract", "boundary" (except the CGMES boundary set), "wire form", "companion file", or "path" except a file path; no em dashes; no history narration in comments.
