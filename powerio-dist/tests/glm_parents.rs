use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};

fn chain(class: &str, depth: usize, leaf_first: bool, solver: &str) -> String {
    let mut objects = Vec::new();
    let mut parent = "sink".to_owned();
    for i in 1..depth {
        let name = format!("tier{i}");
        objects.push(format!(
            "object {class} {{ name {name}; parent {parent}; phases ABCN; nominal_voltage 240; }}"
        ));
        parent = name;
    }
    objects.push(format!("object load {{ name nested; parent {parent}; phases ABN; nominal_voltage 240; constant_impedance_AB 80+20j; latitude 0; longitude 151; }}"));
    if leaf_first {
        objects.reverse();
    }
    format!("module powerflow {{ solver_method {solver}; }}
object node {{ name source; phases ABCN; bustype SWING; nominal_voltage 240; }}
object node {{ name sink; phases ABCN; nominal_voltage 240; }}
object line_configuration {{ name code; z11 1+0.5j; z22 1+0.5j; z33 1+0.5j; }}
object overhead_line {{ name line; phases ABCN; from source; to sink; length 1 mile; configuration code; }}
{}", objects.join("\n"))
}

#[test]
fn parent_chains_obey_the_selected_fork_and_solver_profile() {
    for class in ["node", "meter", "load"] {
        for depth in [1, 2, 3] {
            for leaf_first in [false, true] {
                for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
                    for solver in ["NR", "FBS"] {
                        let text = chain(class, depth, leaf_first, solver);
                        let result = parse(
                            Source::from_memory("case.glm", text.as_bytes()).unwrap(),
                            PreprocessOptions {
                                dialect,
                                ..Default::default()
                            },
                        );
                        if dialect == GlmDialect::Arras4_3 && solver == "NR" && depth > 1 {
                            let error = result.unwrap_err();
                            assert!(error.to_string().contains("nested electrical parents"));
                            continue;
                        }
                        let network = result.unwrap().into_value();
                        assert_eq!(network.buses().len(), 2);
                        assert_eq!(network.loads().len(), 1);
                        assert_eq!(network.loads()[0].bus, "sink");
                        assert_eq!(network.loads()[0].name, "nested");
                        assert_eq!(network.component_locations()[0].point.latitude, Some(0.0));
                        let output =
                            emit_canonical(&network, Destination::memory("fresh.glm").unwrap())
                                .unwrap();
                        let EmittedOutput::Memory { artifacts } = output.output() else {
                            panic!("not memory")
                        };
                        // Canonical loads attach directly to root buses, so
                        // either fork's NR profile can read this output.
                        let again = parse(
                            Source::from_memory("fresh.glm", artifacts[0].bytes()).unwrap(),
                            PreprocessOptions {
                                dialect: GlmDialect::Arras4_3,
                                ..Default::default()
                            },
                        )
                        .unwrap()
                        .into_value();
                        assert_eq!(again.component_locations(), network.component_locations());
                        assert_eq!(again.loads()[0].terminal_map, ["1", "2"]);
                    }
                }
            }
        }
    }
}

#[test]
fn intermediate_loads_keep_their_own_voltage_laws_when_the_chain_is_flattened() {
    let text = chain("load", 3, true, "NR")
        .replace("name tier1;", "name tier1; constant_power_AN 150+30j;")
        .replace("name tier2;", "name tier2; constant_current_BN -0.3-0.6j;");
    let module = parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap();
    let net = module.value();
    assert_eq!(net.loads().len(), 3);
    assert!(net.loads().iter().all(|l| l.bus == "sink"));
    for (name, law) in [
        ("tier1", "power"),
        ("tier2", "current"),
        ("nested", "impedance"),
    ] {
        let load = net.loads().iter().find(|l| l.name == name).unwrap();
        let actual = match load.voltage_model {
            powerio_dist::DistLoadVoltageModel::ConstantPower { .. } => "power",
            powerio_dist::DistLoadVoltageModel::ConstantCurrent { .. } => "current",
            powerio_dist::DistLoadVoltageModel::ConstantImpedance { .. } => "impedance",
            _ => panic!("unexpected law"),
        };
        assert_eq!(actual, law);
    }
}
