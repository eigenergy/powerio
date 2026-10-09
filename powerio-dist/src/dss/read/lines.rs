//! Snapshot Line lowering into existing conductor-resolved records.

use super::*;
use crate::dss::line;

type LineResult<T> = std::result::Result<T, String>;

impl Reader<'_> {
    pub(super) fn resolved_lines(&mut self, raw: &RawDss) -> LineResult<()> {
        // Source definitions remain available in the IR, but are not live
        // electrical dependencies: each native Line copied its own state.
        for object in raw.of_class("linecode") {
            self.net
                .untyped_objects_mut()
                .push(UntypedObject::from(object));
            self.warn(&C::READ_DSS_RETAINED_SOURCE_ONLY, format!(
                "LineCode.{} is retained as a source definition; line-specific effective matrices carry the electrical snapshot", object.name,
            ));
        }
        let mut used: BTreeSet<_> = raw
            .of_class("linecode")
            .map(|o| o.name.to_ascii_lowercase())
            .collect();
        let mut resolved = Vec::new();
        for (index, object) in raw
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.class == "line")
        {
            let mut state = line::resolve(raw, raw.line_snapshot(index))
                .map_err(|e| format!("Line.{}: {e}", object.name))?;
            if state.enabled {
                let matrices = state
                    .primitive(self.net.base_frequency())
                    .map_err(|e| format!("Line.{}: {e}", object.name))?;
                let from = checked_bus(&state.bus[0])?;
                let to = checked_bus(&state.bus[1])?;
                resolved.push((object, state, matrices, from, to));
            } else {
                let mut retained = UntypedObject::from(object);
                retained.props.push((Some("enabled".into()), "no".into()));
                self.net.untyped_objects_mut().push(retained);
                self.warn(
                    &C::READ_DSS_RETAINED_SOURCE_ONLY,
                    format!(
                        "Line.{} is disabled: retained without an electrical branch",
                        object.name,
                    ),
                );
            }
        }
        for (object, state, matrices, from, to) in resolved {
            let base = format!("_pio_line_{}", object.name);
            let mut name = base.clone();
            let mut suffix = 0;
            while !used.insert(name.to_ascii_lowercase()) {
                suffix += 1;
                name = format!("{base}_{suffix}");
            }
            let length = state.physical_length().unwrap_or(1.0);
            if !length.is_finite() || length <= 0.0 {
                return Err(format!("Line.{} has invalid physical length", object.name));
            }
            let mut code = DistLineCode::new(
                &name,
                scale_mat(&matrices.r, 1.0 / length),
                scale_mat(&matrices.x, 1.0 / length),
            );
            code.b_from = scale_mat(&matrices.b, 0.5 / length);
            code.b_to = code.b_from.clone();
            code.source = Some(format!("OpenDSS Line.{} effective snapshot", object.name));
            let n = state.n;
            let terminal_from = self.terminals(&from, n, n, n);
            let terminal_to = self.terminals(&to, n, n, n);
            let mut line = DistLine::new(
                &object.name,
                from.name,
                to.name,
                terminal_from,
                terminal_to,
                name,
                length,
            );
            line.i_max = Some(vec![state.emergamps; n]);
            line.extras.insert("normamps".into(), state.normamps.into());
            line.extras
                .insert("dss_switch".into(), state.is_switch.into());
            line.extras
                .insert("dss_source_length".into(), state.length.into());
            line.extras
                .insert("dss_source_units".into(), state.units.into());
            line.extras.insert(
                "dss_flattened_properties".into(),
                serde_json::json!(
                    object
                        .props
                        .iter()
                        .map(|p| (&p.name, &p.value.text))
                        .collect::<Vec<_>>()
                ),
            );
            if state.physical_length().is_none() {
                self.warn(&C::READ_DSS_RETAINED_SOURCE_ONLY,format!(
                    "Line.{} has no declared length unit: a one-meter electrical equivalent is used and the source length is retained in extras; no geographic distance is inferred",object.name,
                ));
            }
            self.net.line_codes_mut().push(code);
            self.net.lines_mut().push(line);
        }
        Ok(())
    }
}

fn checked_bus(text: &str) -> LineResult<BusSpec> {
    let bus = Value::new(text).to_bus_spec();
    if bus.name.is_empty() || bus.nodes.iter().any(|&n| n < 0 || n == i32::MAX) {
        return Err(format!("invalid or unmaterializable Line bus {text:?}"));
    }
    Ok(bus)
}

#[test]
fn finite_switch_and_linecode_history_survive_typed_lowering() {
    let raw = crate::dss::raw::__parse_raw_with(
        "New Circuit.c\nNew Linecode.c nphases=1 rmatrix=[1] xmatrix=[0.2] cmatrix=[0] units=km\nNew Line.a bus1=sourcebus.1 bus2=end.1 linecode=c length=2\nEdit Linecode.c rmatrix=[7]\nNew Line.b bus1=sourcebus.1 bus2=end.1 phases=1 switch=yes\n",
        "test.dss",
        &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
    );
    let net = sources::staged_from_raw(&raw).unwrap();
    let a = net.lines().iter().find(|l| l.name == "a").unwrap();
    assert_eq!(a.length.to_bits(), 2000.0_f64.to_bits());
    let code = net
        .line_codes()
        .iter()
        .find(|c| c.name == a.linecode)
        .unwrap();
    assert!((code.r_series[0][0] * a.length - 2.0).abs() < 1e-14);
    let b = net.lines().iter().find(|l| l.name == "b").unwrap();
    let code = net
        .line_codes()
        .iter()
        .find(|c| c.name == b.linecode)
        .unwrap();
    assert!((code.r_series[0][0] * b.length - 0.001).abs() < 1e-14);
    assert!(net.switches().is_empty());
}

#[test]
fn invalid_line_never_returns_a_partial_typed_network() {
    let raw = crate::dss::raw::__parse_raw_with(
        "New Circuit.c\nNew Line.a phases=1 r1=0 x1=0 rg=0 xg=0",
        "test.dss",
        &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
    );
    assert!(
        sources::staged_from_raw(&raw)
            .unwrap_err()
            .iter()
            .any(|d| d.code() == C::READ_DSS_LINE_UNRESOLVED.code)
    );
}
