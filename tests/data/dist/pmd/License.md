# License

The JSON files in this directory are derivative works: ENGINEERING model
exports generated with PowerModelsDistribution from `.dss` cases (the
generation commands are recorded in `../README.md`). Each file carries the
license of its source case:

- `ieee13.json` derives from `../opendss/ieee13/`, distributed under the
  BSD 3 clause license in `../opendss/License.txt`.
- `fourwire_linecode.json` derives from `../micro/fourwire_linecode.dss`,
  CC BY 4.0 per `../micro/License.md`.
- `case3_balanced.json` and `case3_balanced_multinetwork.json` derive from
  `test/data/opendss/case3_balanced.dss` in the PowerModelsDistribution
  v0.16.0 package, Copyright (c) 2018, Triad National Security, LLC, under
  the BSD 3 clause license reproduced in `PowerModelsDistribution-LICENSE.md`.
  They are modified from PMD's output as `../README.md` records (the
  multinetwork file keeps two load shape points, and both replace a local
  file path).
