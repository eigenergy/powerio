Vendored for testing only. `goc3_small.json` is powerio's own synthetic case.
`goc3_14bus_20220707.json` is third party and carries no license to
redistribute, so `powerio-prob/Cargo.toml` excludes it from the published
crate; it exists in this repository and nowhere downstream.

`goc3_small.json` is a synthetic case derived from the hand checked test data
in PowerIO.jl.

`goc3_14bus_20220707.json` is vendored byte exact from GOCompetition's
[C3DataUtilities](https://github.com/GOCompetition/C3DataUtilities) validation
data (`test_data/14bus_20220707.json`, commit
`bb5df337553b21ab8be89ae5f9106958541730d4`), sha256
`ad16973416243f38b5286efcf770f5e4b4493e89fdf7ffa6de678d3974b87e49`.

The two cases disagree on nearly every assumption a GOC3 reader can make, so a
claim tested on only one of them is untested. Official Challenge 3 scenario
files use `<prefix>_<0 based index>` uids in document order. This one names its
devices instead: `"Gen Bus 1 #1"`, `"Shunt Bus 6"`, `"Bus 1"`. The digits in
those names are bus numbers, so they do not identify a device: the 17
dispatchable devices carry only 13 distinct numbers, because a generator and a
load at one bus collide. Document order is therefore the only index rule that
addresses this case. The file also omits `e_vio_cost`, and its devices declare
a reactive capability mode, which the official scenario files do not.
