# CGMES node breaker fixture without a TP profile

`NodeBreaker_EQ.xml` and `NodeBreaker_SSH.xml` are hand written CGMES 2.4.15
(CIM16) documents original to this repository under its code license. The EQ
document declares the EquipmentCore and EquipmentOperation profiles and no TP
document exists, so the reader calculates the buses from the ConnectivityNode
graph and the switch positions.

The equipment:

- substation `S1` with voltage levels `VL1` and `VL2` on the 110 kV base
  voltage; bay `BAY1` inside `VL1` contains node `N2`, breaker `BRK1`, and
  load `L1`;
- connectivity nodes `N1`, `N2`, `N3` in `VL1` and `N4`, `N5` in `VL2`;
- busbar sections `BB1` at `N1` and `BB2` at `N4`;
- breaker `BRK1` between `N1` and `N2`, closed in SSH;
- disconnector `DSC1` between `N2` and `N3`, `normalOpen` false in EQ but
  open in SSH, so the SSH position wins and `N3` is a bus of its own;
- breaker `BRK2` between `N4` and `N5`, closed in SSH;
- loads `L1` at `N2`, `L2` at `N3`, and `L3` at `N5`, whose terminal is
  disconnected in SSH;
- generator `G1` at `N1` with generating unit `GU1`;
- line `LINE1` from `N1` to `N4`.

The expected calculation view has three buses: `{N1, N2}` named after `BB1`,
`{N3}`, and `{N4, N5}` named after `BB2`. Closing `DSC1` in SSH merges the
first two.
