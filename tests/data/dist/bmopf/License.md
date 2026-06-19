# License

The schema and example networks in this directory are vendored byte exact
from <https://github.com/frederikgeth/bmopf-report> at the commit pinned in
`../README.md`. That repository carries no license file at the pinned
commit; this directory tracks whatever license the IEEE PES Task Force on
Benchmarking Multiconductor OPF publishes for it, and the files here are
vendored for interoperability testing with the task force's knowledge
(see the review thread on eigenergy/powerio#82).

Underlying data lineage:

- `example_enwl_n1_f2.json` derives from the four wire low voltage network
  dataset: Heidarihaei, Rahmatollah; Geth, Frederik; & Claeys, Sander
  (2024), v1, CSIRO Data Collection, <https://doi.org/10.25919/jaae-vc35>,
  released under the Creative Commons Attribution 4.0 International
  license. The derivative carries the same license.
- `example_ieee13.json` derives from the IEEE 13 node test feeder of the
  IEEE PES Distribution Test Feeder Working Group, as distributed with
  OpenDSS (see `../opendss/License.txt` for the distribution license of
  the `.dss` source). The task force has noted it may replace this example.
