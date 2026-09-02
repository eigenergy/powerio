# Synthetic two authority CGMES 2.4.15 set

A hand written common grid model on CIM16: two individual grid models from
different modeling authorities plus the boundary set they share. No ENTSO-E
data is copied here; every identity, name, and value is invented.

| File | Profile | `Model.modelingAuthoritySet` | `Model.DependentOn` |
| --- | --- | --- | --- |
| `authority-a_EQ.xml` | EquipmentCore | `http://example.org/cgmes/authority-a` | boundary EQ |
| `authority-a_TP.xml` | Topology | authority A | authority A EQ, boundary TP |
| `authority-a_SSH.xml` | SteadyStateHypothesis | authority A | authority A EQ |
| `authority-b_EQ.xml` | EquipmentCore | `http://example.org/cgmes/authority-b` | boundary EQ |
| `authority-b_TP.xml` | Topology | authority B | authority B EQ, boundary TP |
| `authority-b_SSH.xml` | SteadyStateHypothesis | authority B | authority B EQ |
| `boundary_EQ_BD.xml` | EquipmentBoundary | `http://example.org/cgmes/boundary` | none |
| `boundary_TP_BD.xml` | TopologyBoundary | boundary | boundary EQ |

Each authority models one 400 kV voltage level with two TopologicalNodes,
one line between them, one synchronous machine, one load, one ACLineSegment
to the boundary node `X node A-B`, and one EquivalentInjection at that node.
The boundary set defines the 400 kV BaseVoltage, the Line container, the
boundary ConnectivityNode, and its TopologicalNode.

Assembled, the set has four calculation buses and three branches: the two
internal lines and one tie line joining `Tie A to X` (1 + j8 ohm, 0.0001 S)
and `Tie B to X` (3 + j24 ohm, 0.0003 S) at the boundary node. The
EquivalentInjections (-60 MW and +60 MW) become the boundary line setpoints.

Regenerate the files with the same values by editing them in place; there is
no generator script in the repository.
