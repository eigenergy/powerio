# AC/DC validation follow-ups

Tracking for [PowerIO #819](https://github.com/eigenergy/powerio/issues/819) and
[reader PR #822](https://github.com/eigenergy/powerio/pull/822). Resume this checklist
when developing the coupled mathematical models. The MATPOWER-extension reader's
whole-case and reference solve comparisons are implemented in this directory.

## Native mathematical models

- [ ] Evaluate a PowerModelsACDC solution directly against equations built from
  PowerIO's typed equipment, without invoking the MATPOWER writer. Check DC
  branch equations and KCL, pole factors, base conversions, transformer/reactor
  drops, filter reactive power, converter current and losses. This must detect
  importer/exporter mistakes that cancel in a round trip.
- [ ] Explicitly check passive PCC P/Q, valve-side P/Q and DC terminal signs,
  rectifier/inverter operation, AC RMS current bases, and both directional loss
  coefficients. Keep source declarations distinct from reference preprocessing
  (including Imax enlargement and loss-coefficient selection).
- [ ] Build a parameter-variation matrix: reversed power flow, asymmetric Q
  limits, non-unit taps, independent converter/PCC voltage bases, pole counts
  1/2, bypassed transformer/filter/reactor, disabled equipment, parallel branches,
  shared PCCs, disconnected DC grids and back-to-back stations.
- [ ] Qualify supported controls individually in PF and OPF, including DC voltage,
  AC voltage/reactive-power and DC droop settings. Type-4 droop/Pacset and LCC
  calculation support need their own tests before being claimed.
- [ ] Compare native coupled PF/OPF feasibility and objectives with the pinned
  reference. Check independent residuals and constraint activity; allow distinct
  optimal dispatch where OPF is nonunique. Lift existing unsupported-calculation
  guards only for model combinations that pass these checks.

## Future JSON adapter

- [ ] Add the natural JSON variant based on PowerModelsACDC IR/data-dictionary
  serialization as a separate source adapter. The current `matpower-acdc` token
  denotes the MATPOWER-extension variant only; PowerIO IR serialization is a
  different operation.
- [ ] Specify raw versus preprocessed dictionaries, `per_unit` handling, identity
  and table keys, required fields and supported extensions. Reuse the typed
  equipment and capability checks, but qualify the JSON mapping independently;
  do not infer units from the file extension or silently apply preprocessing twice.
