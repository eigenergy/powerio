# Migrating

One guide per release that breaks something, named for the release you are moving to. A release with nothing to migrate gets no page; read the [changelog](https://github.com/eigenergy/powerio/blob/main/CHANGELOG.md) for those.

| guide | covers |
|---|---|
| [To 1.0](migration-v1.md) | OPF objectives and constraint masks, stable analysis identities, economic result derivatives, and the retired regularization term |
| [To 0.10](migration-v0.10.md) | the module value model, the one way 0.9 document reader, C ABI 6, and the Python and Julia module interfaces |
| [To 0.9](migration-v0.9.md) | the Rust, Python and CLI surfaces: renamed types, the DC susceptance default, `ReferenceBuses`, the split error types, and `.pio.json` documents written before 0.9 |
| [To 0.7](migration-v0.7.md) | DC OPF problem data moving from `powerio-matrix` to `powerio-prob` |

A C or Julia consumer wants [the ABI 5 guide](abi-v5.md) instead, and probably both: the ABI carries the C signatures, and 0.9.0 breaks things above it that the ABI integer says nothing about.

## Version rule

Every wire form powerio authors carries `powerio_version`, the release that wrote it, and every one of them is on the library's own version. There is no second version register to track.

A document loads when it shares this build's lineage: the major once the major
reaches 1, and the major and minor pair while the major is 0. The public beta
is an explicit exception: 1.x reads 0.10 documents and applies the directed
semantic upgrades described in the 1.0 migration guide. A `.pio.json` written by
0.9 upgrades one way through the separate legacy decoder, as the
[0.10 migration guide](migration-v0.10.md) describes. Foreign formats keep their
own versions untouched — powerio implements case formats and authors none, so
pandapower's `3.0.0` and the BMOPF `$schema` are reproduced rather than set.
