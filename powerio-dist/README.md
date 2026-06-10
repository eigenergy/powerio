# powerio-dist

`powerio-dist` parses multiconductor distribution network cases into a typed
model in wire coordinates and converts between OpenDSS `.dss`,
PowerModelsDistribution ENGINEERING JSON, and the draft BMOPF schema from the
IEEE PES Task Force on Benchmarking Multiconductor OPF
(<https://github.com/frederikgeth/bmopf-report>).

Writing back to the source format reproduces the file byte for byte; every
cross-format conversion reports each field the target cannot represent.

The workspace README covers the CLI, Python package, C ABI, and the
transmission crates: <https://github.com/eigenergy/powerio>.
