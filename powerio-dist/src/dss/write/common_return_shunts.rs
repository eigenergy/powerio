//! Recognize a uniform wye star from typed admittance, without replaying kvar.
use super::{Configuration, DssWriter, ElementKv, MulticonductorNetwork, extras_usize, num};
use crate::model::DistShunt;

/// The return is the last terminal. Exact comparison is deliberate: an edited
/// unequal or coupled matrix must take the general full-admittance writer.
#[allow(
    clippy::float_cmp,
    reason = "only an exact star may use one scalar bank"
)]
fn susceptance(shunt: &DistShunt) -> Option<(usize, f64)> {
    // A single-phase delta has the same two-node matrix but one native
    // terminal. Preserve its existing emitter and observation layout.
    if super::shunt_stashed_delta(shunt) {
        return None;
    }
    let phases = extras_usize(&shunt.extras, "phases")?;
    let n = phases.checked_add(1)?;
    if phases == 0
        || shunt.terminal_map.len() != n
        || [&shunt.g, &shunt.b]
            .iter()
            .any(|m| m.len() != n || m.iter().any(|r| r.len() != n))
        || shunt.g.iter().flatten().any(|&g| g != 0.0)
        || shunt
            .terminal_map
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != n
    {
        return None;
    }
    let b = shunt.b[0][0];
    if !b.is_finite() || b <= 0.0 {
        return None;
    }
    let total = (0..phases).fold(0.0, |sum, _| sum + b);
    for i in 0..n {
        for j in 0..n {
            let expected = if i == phases && j == phases {
                total
            } else if i == j {
                b
            } else if i == phases || j == phases {
                -b
            } else {
                0.0
            };
            if shunt.b[i][j] != expected {
                return None;
            }
        }
    }
    Some((phases, b))
}

impl DssWriter {
    pub(super) fn common_return_shunt(
        &mut self,
        net: &MulticonductorNetwork,
        shunt: &DistShunt,
    ) -> bool {
        let Some((phases, b)) = susceptance(shunt) else {
            return false;
        };
        let kv = self.element_kv(
            &shunt.extras,
            ElementKv {
                bus: &shunt.bus,
                phases,
                configuration: Configuration::Wye,
                name: &shunt.name,
                class: "capacitor",
                typed_kv: None,
            },
        );
        let voltage = if phases == 2 || phases == 3 {
            kv * 1e3 / 3f64.sqrt()
        } else {
            kv * 1e3
        };
        let kvar = b * voltage.powi(2) * phases as f64 / 1e3;
        self.check_name("capacitor", &shunt.name);
        let returns = vec![shunt.terminal_map[phases].clone(); phases];
        let mut line = format!(
            "New Capacitor.{} bus1={} bus2={} phases={phases} conn=wye kv={} kvar={} basefreq={}",
            shunt.name,
            self.bus_ref(&shunt.bus, &shunt.terminal_map[..phases]),
            self.bus_ref(&shunt.bus, &returns),
            num(kv),
            num(kvar),
            num(net.base_frequency())
        );
        let mut extras = shunt.extras.clone();
        super::strip_shunt_extras(&mut extras);
        extras.remove("basefreq");
        line.push_str(&self.extras_tail("capacitor", &shunt.name, &extras));
        self.line_out(&line);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::parse_dss_str;

    #[test]
    fn edited_admittance_overrides_stale_kvar() {
        let mut net = parse_dss_str(
            "new circuit.c basekv=13.2\nnew capacitor.c bus1=b.3.1.2 bus2=b.10.10.10 kvar=1200 kv=13.2\n",
        );
        assert_eq!(net.shunts().len(), 1);
        let shunt = &mut net.shunts_mut()[0];
        assert_eq!(shunt.terminal_map, ["3", "1", "2", "10"]);
        for b in shunt.b.iter_mut().flatten() {
            *b *= 2.0;
        }
        shunt.extras.insert("kvar".into(), 999.0.into());
        let expected = shunt.b.clone();
        let out = super::super::emit_dss_text(&net);
        assert!(
            out.text.contains("bus2=b.10.10.10 phases=3 conn=wye"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("kvar=999"));
        let repeated = parse_dss_str(&out.text);
        for (a, b) in expected
            .iter()
            .flatten()
            .zip(repeated.shunts()[0].b.iter().flatten())
        {
            assert!((a - b).abs() < 1e-14);
        }
        net.shunts_mut()[0].b[0][0] += 0.1;
        assert!(susceptance(&net.shunts()[0]).is_none());
        let out = super::super::emit_dss_text(&net);
        assert!(!out.text.contains("New Capacitor.c "));
        assert!(out.text.contains("_pio_shunt_0_"));
    }

    #[test]
    fn unsupported_common_return_state_is_never_ready() {
        // Inspect the retained model directly: terminal commands can already
        // make the higher-level snapshot parser refuse the source.
        fn read(text: &str) -> MulticonductorNetwork {
            let raw = crate::dss::__parse_raw_with(text, "test.dss", &mut |_: &std::path::Path| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "test"))
            });
            crate::dss::read::to_network_from_raw(&raw).0
        }
        for suffix in [
            "states=(0)",
            "numsteps=2",
            "cuf=10",
            "r=.1",
            "xl=.3",
            "harm=5",
            "conn=delta",
            "\nedit capacitor.c bus1=elsewhere",
            "\nopen capacitor.c 1 1",
            "\nnew capacitor.copy like=c bus1=b.1.2.3 bus2=b.10.10.10",
        ] {
            let net = read(&format!(
                "new circuit.c\nnew capacitor.c bus1=b.1.2.3 bus2=b.10.10.10 kvar=1200 kv=13.2 {suffix}\n"
            ));
            assert!(
                crate::audit_electrical_readiness(&net)
                    .blockers()
                    .any(|f| f.code == "READINESS.DSS.CAPACITOR_UNRESOLVED"),
                "{suffix}"
            );
        }
        let text = "new circuit.c\nnew capacitor.c bus1=b.1.2.3 bus2=b.10.10.10 enabled=no\n";
        let net = read(text);
        assert_eq!(net.shunts().as_slice(), []);
        assert!(crate::audit_electrical_readiness(&net).is_ready());
        let net = read(&format!("{text}enable capacitor.c\n"));
        assert!(!crate::audit_electrical_readiness(&net).is_ready());
    }
}
