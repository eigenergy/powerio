# License

`draft_bmopf_schema.json` and the two example networks in this directory are
vendored unchanged from
<https://github.com/distribution-system-opt/bmopf-resources> at the commit
pinned in `../README.md`, which also records their measured hashes. That
repository carries no license file at that commit; this directory tracks
whatever license the IEEE PES Task Force on Benchmarking Multiconductor OPF
publishes for it, and the files here are vendored for interoperation testing
with the task force's knowledge (see the review thread on eigenergy/powerio#82).

`bmopf-0.2.0.schema.json` is vendored unchanged from
<https://github.com/distribution-system-opt/dsopt-schema> at the commit pinned
in `../README.md`, and that repository publishes it under CC BY 4.0.

Underlying data lineage:

- `example_enwl_n1_f2.json` derives from the four wire low voltage network
  dataset: Heidarihaei, Rahmatollah; Geth, Frederik; and Claeys, Sander
  (2024), v1, CSIRO Data Collection, <https://doi.org/10.25919/jaae-vc35>,
  released under the Creative Commons Attribution 4.0 International license
  (CC BY 4.0). The derivative carries the same license.
- `example_ieee13.json` derives from the IEEE 13 node test feeder of the
  IEEE PES Distribution Test Feeder Working Group, as distributed with
  OpenDSS. `../opendss/License.txt` is the distribution license of the `.dss`
  source, and it governs this derivative too, so the file is not CC BY 4.0.
  `bmopf-resources` records that the task force may replace this example; a
  licensed synthetic case would replace it here, and none exists yet.
