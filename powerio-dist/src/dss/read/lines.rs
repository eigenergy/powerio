//! Snapshot Line lowering into existing conductor-resolved records.

#[cfg(test)]
use super::sources;
use super::sources::unique_name;
use super::{
    BTreeSet, BusSpec, C, DistLine, DistLineCode, RawDss, Reader, UntypedObject, Value, scale_mat,
};
use crate::dss::{line, line_history::Snapshot, linecode};
use std::collections::HashMap;

type LineResult<T> = std::result::Result<T, String>;

/// Per-meter linecodes shared by Lines that still carry exactly what they
/// fetched from one OpenDSS LineCode prefix.
#[derive(Default)]
struct SharedCodes {
    // A Line that fetched the prefix and nothing else; None if it no longer
    // resolves on its own, which leaves its Lines on per-Line codes.
    references: HashMap<Snapshot, Option<line::State>>,
    // (prefix, meters per declared length unit) -> typed linecode name.
    names: HashMap<(Snapshot, u64), String>,
    // LineCode names already given to a shared code, lowercase.
    claimed: BTreeSet<String>,
}

// Divide once: a separately rounded reciprocal introduces another rounding
// before the total impedance is reconstructed from density and length.
fn per_meter(matrix: &[Vec<f64>], length: f64) -> Vec<Vec<f64>> {
    matrix
        .iter()
        .map(|row| row.iter().map(|v| v / length).collect())
        .collect()
}

impl Reader<'_> {
    fn retain_line_definitions(&mut self, raw: &RawDss) {
        for object in raw
            .objects
            .iter()
            .filter(|o| matches!(o.class.as_str(), "line" | "linecode"))
        {
            for prop in object
                .props
                .iter()
                .filter(|p| p.name.as_deref() == Some("units"))
            {
                if crate::dss::linecode::units(&prop.value.text) == 0
                    && !prop.value.text.eq_ignore_ascii_case("none")
                {
                    self.warn(&C::READ_DSS_VALUE_UNSUPPORTED, format!(
                        "{}.{}: unknown units `{}` use native units=none; no physical distance is inferred",
                        object.class, object.name, prop.value.text,
                    ));
                }
            }
        }
        // Source definitions remain available in the IR, but are not live
        // electrical dependencies: each native Line copied its own state.
        for object in raw.of_class("linecode") {
            self.net
                .untyped_objects_mut()
                .push(UntypedObject::from(object));
            self.warn(&C::READ_DSS_RETAINED_SOURCE_ONLY, format!(
                "LineCode.{} is retained as a source definition; per-meter linecodes carry the electrical snapshots Lines fetched from it", object.name,
            ));
        }
    }

    fn line_defaults(&mut self, object: &super::RawObject) {
        if object.get("length").is_none() {
            self.defaulted("line", &object.name, "length");
        }
        if [
            "like", "linecode", "rmatrix", "xmatrix", "cmatrix", "r1", "r0", "x1", "x0", "c1",
            "c0", "b1", "b0", "switch", "geometry", "spacing",
        ]
        .iter()
        .all(|key| object.get(key).is_none())
        {
            self.defaulted("line", &object.name, "r1");
            self.defaulted("line", &object.name, "x1");
        }
    }

    fn retain_deferred_line(&mut self, object: &super::RawObject, reason: Option<&str>) -> bool {
        if !crate::readiness::DEFERRED_GEOMETRY_KEYS
            .iter()
            .any(|key| object.get(key).is_some())
        {
            return false;
        }
        self.warn(
            &C::READ_DSS_GEOMETRY_UNRESOLVED,
            format!(
                "line {}: {}; the source object is retained and electrical readiness is blocked",
                object.name,
                reason.unwrap_or("geometry-derived conductor impedances are unresolved"),
            ),
        );
        self.net
            .untyped_objects_mut()
            .push(UntypedObject::from(object));
        true
    }

    #[expect(
        clippy::too_many_lines,
        reason = "ordered line resolution and typed lowering stay in one pass"
    )]
    pub(super) fn resolved_lines(&mut self, raw: &RawDss) -> LineResult<()> {
        self.retain_line_definitions(raw);
        let mut used: BTreeSet<_> = raw
            .of_class("linecode")
            .map(|o| o.name.to_ascii_lowercase())
            .collect();
        let mut resolved = Vec::new();
        let mut joint = raw
            .of_class("line")
            .any(|o| o.get("geometry").is_some() || o.get("spacing").is_some())
            .then(|| crate::dss::geometry::timeline::resolve(raw));
        for (index, object) in raw
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.class == "line")
        {
            let result = if let Some(joint) = &mut joint {
                joint.remove(&index).ok_or("missing ordered Line state")?
            } else {
                if self.retain_deferred_line(object, None) {
                    continue;
                }
                line::resolve(raw, raw.line_snapshot(index))
            };
            let mut state = match result {
                Ok(state) => state,
                Err(error) if self.retain_deferred_line(object, Some(&error)) => {
                    continue;
                }
                Err(error) => return Err(format!("Line.{}: {error}", object.name)),
            };
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
        let mut shared = SharedCodes::default();
        for (object, state, matrices, from, to) in resolved {
            self.line_defaults(object);
            let length = state.physical_length().unwrap_or(1.0);
            if !length.is_finite() || length <= 0.0 {
                return Err(format!("Line.{} has invalid physical length", object.name));
            }
            let name = if let Some(name) =
                self.shared_line_code(raw, &state, &object.name, &mut shared, &mut used)?
            {
                name
            } else {
                let name = unique_name(&format!("_pio_line_{}", object.name), &mut used);
                let mut code = DistLineCode::new(
                    &name,
                    per_meter(&matrices.r, length),
                    per_meter(&matrices.x, length),
                );
                code.b_from = scale_mat(&matrices.b, 0.5 / length);
                code.b_to.clone_from(&code.b_from);
                code.source = Some(format!("OpenDSS Line.{} effective snapshot", object.name));
                self.net.line_codes_mut().push(code);
                name
            };
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
            if state.emergamps_available {
                line.i_max = Some(vec![state.emergamps; n]);
            }
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
            if state.closed.iter().flatten().any(|closed| !closed) {
                let code = self
                    .net
                    .line_codes()
                    .iter()
                    .rev()
                    .find(|c| c.name == line.linecode)
                    .cloned()
                    .ok_or("missing Line linecode")?;
                self.open_line(raw, &line, &code, &state.closed)?;
            } else {
                self.net.lines_mut().push(line);
            }
        }
        Ok(())
    }

    /// The shared per-meter linecode of a Line whose primitive still follows
    /// from exactly what it fetched from an OpenDSS LineCode, created on
    /// first use; `None` keeps the Line on its own effective snapshot. The
    /// first shared code of a LineCode takes its name, so a written and
    /// reread case keeps it; a later prefix, or the same prefix over another
    /// length unit, takes the next free `_N` suffix.
    fn shared_line_code(
        &mut self,
        raw: &RawDss,
        state: &line::State,
        line: &str,
        shared: &mut SharedCodes,
        used: &mut BTreeSet<String>,
    ) -> LineResult<Option<String>> {
        let Some(point) = state.code() else {
            return Ok(None);
        };
        let Some(reference) = shared
            .references
            .entry(point)
            .or_insert_with(|| {
                linecode::resolve(raw, point)
                    .ok()
                    .map(|code| line::State::fetched(point, &code))
            })
            .as_ref()
        else {
            return Ok(None);
        };
        let Some(meters) = state.fetched_meters(reference) else {
            return Ok(None);
        };
        let key = (point, meters.to_bits());
        if let Some(name) = shared.names.get(&key) {
            return Ok(Some(name.clone()));
        }
        let source = &raw.objects[point.object].name;
        let name = if shared.claimed.insert(source.to_ascii_lowercase()) {
            source.clone()
        } else {
            unique_name(source, used)
        };
        let matrices = reference
            .per_meter(self.net.base_frequency(), meters)
            .map_err(|e| format!("Line.{line}: {e}"))?;
        let mut code = DistLineCode::new(&name, matrices.r, matrices.x);
        code.b_from = scale_mat(&matrices.b, 0.5);
        code.b_to.clone_from(&code.b_from);
        code.source = Some(format!(
            "OpenDSS LineCode.{source} effective snapshot, first fetched by Line.{line}"
        ));
        self.net.line_codes_mut().push(code);
        shared.names.insert(key, name.clone());
        Ok(Some(name))
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
    assert_eq!(net.switches().as_slice(), []);
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

#[test]
fn unrated_dss_line_does_not_acquire_the_default_600_amp_limit() {
    let raw = crate::dss::raw::__parse_raw_with(
        "New Circuit.c\nNew Line.l bus1=sourcebus.1 bus2=end.1 phases=1 r1=0.1 x1=0.2",
        "unrated.dss",
        &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
    );
    let net = sources::staged_from_raw(&raw).unwrap();
    let line = net.lines().iter().find(|line| line.name == "l").unwrap();
    assert_eq!(line.i_max, None);
}

#[test]
fn switch_status_does_not_declare_the_default_600_amp_limit() {
    // A `switch=yes` Line keeps its constructor default emergency rating
    // unstated: the switch replaces the impedance, not the rating (#762).
    let raw = crate::dss::raw::__parse_raw_with(
        "New Circuit.c\nNew Linecode.lc nphases=1 r1=0.1 x1=0.2\nNew Linecode.rated nphases=1 r1=0.1 x1=0.2 emergamps=240\nNew Line.bare bus1=sourcebus.1 bus2=a.1 phases=1 switch=yes\nNew Line.coded bus1=sourcebus.1 bus2=b.1 phases=1 switch=yes linecode=lc\nNew Line.reverted bus1=sourcebus.1 bus2=c.1 phases=1 switch=yes switch=no r1=0.1 x1=0.2\nNew Line.rated bus1=sourcebus.1 bus2=d.1 phases=1 switch=yes linecode=rated\nNew Line.stated bus1=sourcebus.1 bus2=e.1 phases=1 switch=yes emergamps=400",
        "switches.dss",
        &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
    );
    let net = sources::staged_from_raw(&raw).unwrap();
    let i_max = |name: &str| {
        net.lines()
            .iter()
            .find(|line| line.name == name)
            .unwrap()
            .i_max
            .clone()
    };
    assert_eq!(i_max("bare"), None);
    assert_eq!(i_max("coded"), None);
    assert_eq!(i_max("reverted"), None);
    assert_eq!(i_max("rated").as_deref(), Some(&[240.0][..]));
    assert_eq!(i_max("stated").as_deref(), Some(&[400.0][..]));
}

#[test]
fn explicit_and_inherited_linecode_emergency_ratings_remain_typed() {
    let raw = crate::dss::raw::__parse_raw_with(
        "New Circuit.c\nNew Linecode.rated nphases=1 r1=0.1 x1=0.2 emergamps=240\nNew Line.inherited bus1=sourcebus.1 bus2=a.1 phases=1 linecode=rated\nNew Line.explicit bus1=sourcebus.1 bus2=b.1 phases=1 r1=0.1 x1=0.2 emergamps=180",
        "rated.dss",
        &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
    );
    let net = sources::staged_from_raw(&raw).unwrap();
    let inherited = net
        .lines()
        .iter()
        .find(|line| line.name == "inherited")
        .unwrap();
    let explicit = net
        .lines()
        .iter()
        .find(|line| line.name == "explicit")
        .unwrap();
    assert_eq!(inherited.i_max.as_deref(), Some(&[240.0][..]));
    assert_eq!(explicit.i_max.as_deref(), Some(&[180.0][..]));
}

#[test]
fn short_line_retains_native_total_impedance_after_normalization() {
    let raw = crate::dss::raw::__parse_raw_with(
        "new circuit.c\nnew linecode.busbar nphases=3 r1=1e-4 x1=1e-4 r0=1e-4 x0=1e-4 c1=0 c0=0 units=km\nnew line.short bus1=sourcebus bus2=end length=.001 linecode=busbar units=m\nnew line.own bus1=sourcebus bus2=end2 r1=1e-4 x1=1e-4 r0=1e-4 x0=1e-4 c1=0 c0=0 units=km length=1e-6\n",
        "test.dss",
        &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
    );
    let net = sources::staged_from_raw(&raw).unwrap();
    // Pinned native total series impedance is 1e-10 + j1e-10 ohm for both.
    let code = |name: &str| {
        let line = net.lines().iter().find(|l| l.name == name).unwrap();
        let code = net
            .line_codes()
            .iter()
            .find(|c| c.name == line.linecode)
            .unwrap();
        (line.length, code)
    };
    // A Line's own snapshot divides its native total once: a separately
    // rounded reciprocal would add an ulp before reinversion.
    let (length, own) = code("own");
    for matrix in [&own.r_series, &own.x_series] {
        for (i, row) in matrix.iter().enumerate() {
            for (j, value) in row.iter().enumerate() {
                assert_eq!(
                    (value * length).to_bits(),
                    (if i == j { 1e-10_f64 } else { 0.0 }).to_bits()
                );
            }
        }
    }
    // A shared LineCode holds the per-km value over 1000 m, rounded once; its
    // product with a Line length is then within an ulp of the native total.
    let (length, shared) = code("short");
    assert_eq!(shared.name, "busbar");
    for matrix in [&shared.r_series, &shared.x_series] {
        for (i, row) in matrix.iter().enumerate() {
            for (j, value) in row.iter().enumerate() {
                let native = if i == j { 1e-10_f64 } else { 0.0 };
                assert_eq!(
                    value.to_bits(),
                    (if i == j { 1e-4_f64 / 1000.0 } else { 0.0 }).to_bits()
                );
                assert!((value * length - native).abs() <= f64::EPSILON * native);
            }
        }
    }
}
