# powerio-prob

`powerio-prob` holds operating points, calculation instances, and solutions
over both network families. An instance is the complete input for one named
calculation and either contains or shares its network; a solution is one
calculation's result and shares the instance it solves. A DOE GO Challenge 3
input/problem data file maps to `AcScucInstance`, and optional format fields
outside the Challenge 3 formulation do not. When one source contains the
problem and its matching solution, the universal parser uses the problem's
identities and time axis to produce an `AcScucSolution`. DeepMind OPFData
parsing (`AcOpfSolution`) also lives here. BMOPF electrical data parses to a
`MulticonductorNetwork` in `powerio-dist`; you then construct the
`McAcPfInstance` or `McAcOpfInstance` here yourself.

The crate stays matrix free; sparse operators over these instances live in
`powerio-matrix`. The ordinary way in is `parse` from the `powerio` facade:

```rust,ignore
use powerio::{PioValue, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = parse("scenario_002.json")?;
    let PioValue::AcScucInstance(instance) = module.value() else {
        panic!("expected an AC SCUC instance");
    };
    assert_eq!(instance.inputs().interval_durations().len(), 24);
    Ok(())
}
```
