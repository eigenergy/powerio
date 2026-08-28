# powerio-prob

Operating points, the seven calculation instances, and the seven solutions, over the two network families. A calculation instance is the complete input for one named calculation and contains or shares its network; a solution is one calculation's result and shares the instance it solves. DOE GO Challenge 3 parsing (`AcScucInstance`), DeepMind OPFData parsing (`AcOpfSolution`), and BMOPF instance assembly (`McAcOpfInstance`) live here.

The crate stays matrix free: sparse operators over these instances belong to `powerio-matrix`, and the ordinary way in is the `powerio` facade's one parse.

```rust,ignore
let module = powerio::parse(powerio_core::Source::open("scenario_002.json")?)?;
assert_eq!(module.value().kind().as_str(), "ac_scuc_instance");
```
