# PowSyBl import check

This check emits XIIDM, CGMES, PSS/E RAW revision 35, and PSS/E RAWX revision
35 from the existing MATPOWER case9 fixture. PyPowSyBl loads every fresh output
with PowSyBl, runs IIDM validation, and checks equipment counts and references.

The dependency is PyPowSyBl 1.16.1. Its PowSyBl Dependencies 2026.1.0 release
pins PowSyBl Core 7.3.0, the latest published Core release when this check was
added. The gate also checks the Core release commit
`0939bfcc2c0c094de907dc818dd688b4cbfb7281`. The local Core checkout used during development was
`a795bfac3c1a1c62a09494f2d3d6cbfeec9c7789`, the 7.4.0 development tree. CI
uses the published 7.3.0 code instead of compiling the complete development
tree for each pull request.

Run it from the repository root after installing the requirements:

```sh
python3 -m venv .venv-powsybl
.venv-powsybl/bin/pip install -r evals/powsybl/requirements.txt
cargo build -p powerio-cli
bash evals/powsybl/run.sh target/debug/powerio .venv-powsybl/bin/python
```

No PowSyBl fixture is copied into this repository. Every checked file is fresh
PowerIO output derived from the existing BSD licensed case9 fixture.
