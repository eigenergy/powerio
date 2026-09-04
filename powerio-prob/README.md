# powerio-prob

Operating points, calculation instances, and solutions over the two network
families. An instance is the complete input for one named calculation and
contains or shares its network. A solution is one calculation's result and
shares the instance it solves. A DOE GO Challenge 3 input/problem data file
maps to `AcScucInstance`; optional format fields outside the Challenge 3
formulation do not. When one source contains the problem and its matching
solution, the universal parser uses the problem's identities and time axis to
produce `AcScucSolution`. DeepMind OPFData parsing (`AcOpfSolution`) also lives
here. BMOPF electrical data
parses to `MulticonductorNetwork`; callers then construct `McAcPfInstance` or
`McAcOpfInstance` explicitly.

The crate stays matrix free: sparse operators over these instances belong to `powerio-matrix`, and the ordinary way in is the `powerio` facade's one parse.

```rust,ignore
let module = powerio::parse(powerio::Source::open("scenario_002.json")?, None)?;
let powerio::PioValue::AcScucInstance(instance) = &module.value else {
    panic!("expected an AC SCUC instance");
};
assert_eq!(instance.inputs().interval_durations().len(), 24);
# Ok::<(), Box<dyn std::error::Error>>(())
```
