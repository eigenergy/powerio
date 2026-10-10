//! Canonical grounded service circuits derived from current typed coefficients.
use super::{
    METERS_PER_MILE, Writer, complex_literal, phase_name, unsupported, validate_line_matrices,
};
use crate::{
    Configuration, DistLine, DistLineCode, DistLoad, DistLoadVoltageModel, DistLocationTarget,
    DistTransformer, DistWindingConn, MulticonductorNetwork,
};
use num_complex::Complex64 as Complex;
use powerio_core::Error;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;

#[derive(Default)]
pub(super) struct Services<'a> {
    pub centers: BTreeMap<&'a str, Center<'a>>,
    pub buses: BTreeMap<&'a str, usize>,
    pub core_buses: BTreeSet<&'a str>,
    pub core_loads: BTreeSet<&'a str>,
}
pub(super) struct Center<'a> {
    pub phase: usize,
    pub core_bus: &'a str,
    pub core_load: &'a str,
    primary: f64,
    secondary: f64,
    rating: f64,
    z: [Complex; 3],
    shunt: Complex,
}

impl<'a> Services<'a> {
    pub(super) fn discover(net: &'a MulticonductorNetwork) -> Result<Self, Error> {
        let mut plan = Self::default();
        for t in net.transformers().iter().filter(|t| t.windings.len() == 4) {
            let center = center(net, t)?;
            let bus = t.windings[1].bus.as_str();
            if plan
                .buses
                .insert(bus, center.phase)
                .is_some_and(|p| p != center.phase)
                || !plan.core_buses.insert(center.core_bus)
                || !plan.core_loads.insert(center.core_load)
            {
                return Err(unsupported(
                    "service transformers have conflicting anchors or shared core circuits",
                ));
            }
            plan.centers.insert(&t.name, center);
        }
        let mut neighbors: BTreeMap<&str, Vec<&DistLine>> = BTreeMap::new();
        for line in net.lines() {
            neighbors.entry(&line.bus_from).or_default().push(line);
            neighbors.entry(&line.bus_to).or_default().push(line);
        }
        let mut queue: VecDeque<_> = plan.buses.keys().copied().collect();
        while let Some(bus) = queue.pop_front() {
            let phase = plan.buses[bus];
            for line in neighbors.get(bus).into_iter().flatten() {
                if line.terminal_map_from != line.terminal_map_to
                    || !matches!(
                        line.terminal_map_from
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .as_slice(),
                        ["1", "2"] | ["1", "2", "n"]
                    )
                {
                    return Err(unsupported(format!(
                        "service line {} needs another conductor/connection mapping",
                        line.name
                    )));
                }
                let neighbor = if line.bus_from == bus {
                    line.bus_to.as_str()
                } else {
                    line.bus_from.as_str()
                };
                match plan.buses.get(neighbor) {
                    Some(&p) if p != phase => {
                        return Err(unsupported(
                            "service line connects different primary anchors",
                        ));
                    }
                    Some(_) => {}
                    None => {
                        plan.buses.insert(neighbor, phase);
                        queue.push_back(neighbor);
                    }
                }
            }
        }
        if net
            .shunts()
            .iter()
            .any(|s| plan.buses.contains_key(s.bus.as_str()))
        {
            return Err(unsupported(
                "service shunts need a qualified triplex admittance mapping",
            ));
        }
        if net.switches().iter().any(|s| {
            plan.buses.contains_key(s.bus_from.as_str())
                || plan.buses.contains_key(s.bus_to.as_str())
        }) {
            return Err(unsupported(
                "service switches need a qualified triplex contact mapping",
            ));
        }
        Ok(plan)
    }
}

#[allow(clippy::float_cmp, clippy::too_many_lines)] // Exact winding topology/base certificate.
fn center<'a>(
    net: &'a MulticonductorNetwork,
    transformer: &'a DistTransformer,
) -> Result<Center<'a>, Error> {
    let fail = || {
        unsupported(format!(
            "transformer {} needs another center-tapped winding/core mapping",
            transformer.name
        ))
    };
    let [
        primary_winding,
        positive_winding,
        negative_winding,
        core_winding,
    ] = transformer.windings.as_slice()
    else {
        return Err(fail());
    };
    if transformer.phases != 1
        || transformer.xsc_pct.len() != 6
        || transformer
            .extras
            .iter()
            .any(|(k, v)| k != "ppm_antifloat" || v.as_f64() != Some(0.0))
        || primary_winding.terminal_map.len() != 2
        || primary_winding.terminal_map[1] != "n"
        || positive_winding.terminal_map != ["1", "n"]
        || negative_winding.terminal_map != ["n", "2"]
        || core_winding.terminal_map != ["1", "n"]
        || positive_winding.bus != negative_winding.bus
        || primary_winding.bus == positive_winding.bus
        || core_winding.bus == primary_winding.bus
        || core_winding.bus == positive_winding.bus
        || core_winding.r_pct != 0.0
        || transformer.windings.iter().any(|w| {
            w.conn != DistWindingConn::Wye
                || !w.v_ref.is_finite()
                || w.v_ref <= 0.0
                || !w.tap.is_finite()
                || w.tap <= 0.0
                || !w.s_rating.is_finite()
                || w.s_rating <= 0.0
                || w.s_rating != primary_winding.s_rating
                || w.r_neutral.is_some_and(|r| r != 0.0)
                || w.x_neutral.is_some_and(|x| x != 0.0)
        })
    {
        return Err(fail());
    }
    if !net.buses().iter().any(|bus| {
        bus.id == core_winding.bus && bus.terminals == ["1", "n"] && bus.grounded == ["n"]
    }) {
        return Err(fail());
    }
    let phase = super::phases(&primary_winding.terminal_map[..1])?[0];
    let x = [
        transformer.xsc_pct[2],
        transformer.xsc_pct[4],
        transformer.xsc_pct[5],
    ];
    if transformer.xsc_pct != [x[0] + x[1], x[0] + x[2], x[0], x[1] + x[2], x[1], x[2]] {
        return Err(fail());
    }
    let z = [primary_winding, positive_winding, negative_winding]
        .map(|w| w.r_pct / 100.0)
        .into_iter()
        .zip(x)
        .map(|(r, x)| Complex::new(r, x / 100.0))
        .collect::<Vec<_>>();
    if z.iter()
        .any(|v| !v.re.is_finite() || !v.im.is_finite() || v.re <= 0.0 || v.im <= 0.0)
    {
        return Err(fail());
    }
    let primary = primary_winding.v_ref * primary_winding.tap;
    let secondary = positive_winding.v_ref * positive_winding.tap;
    if negative_winding.v_ref * negative_winding.tap != secondary {
        return Err(fail());
    }
    let loads = net
        .loads()
        .iter()
        .filter(|l| l.bus == core_winding.bus)
        .collect::<Vec<_>>();
    let [load] = loads.as_slice() else {
        return Err(fail());
    };
    let DistLoadVoltageModel::ConstantImpedance { v_nom } = &load.voltage_model else {
        return Err(fail());
    };
    if load.configuration != Configuration::SinglePhase
        || load.terminal_map != ["1", "n"]
        || load.p_nom.len() != 1
        || load.q_nom.len() != 1
        || v_nom.len() != 1
        || !load.extras.is_empty()
        || [load.p_nom[0], load.q_nom[0], v_nom[0]]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
        || net
            .lines()
            .iter()
            .any(|l| l.bus_from == core_winding.bus || l.bus_to == core_winding.bus)
        || net
            .switches()
            .iter()
            .any(|s| s.bus_from == core_winding.bus || s.bus_to == core_winding.bus)
        || net.sources().iter().any(|s| s.bus == core_winding.bus)
        || net.shunts().iter().any(|s| s.bus == core_winding.bus)
        || net.generators().iter().any(|g| g.bus == core_winding.bus)
        || net.ibrs().iter().any(|g| g.bus == core_winding.bus)
        || net.capacitors().iter().any(|g| g.bus == core_winding.bus)
        || net.transformers().iter().any(|other| {
            other.name != transformer.name
                && other.windings.iter().any(|w| w.bus == core_winding.bus)
        })
    {
        return Err(fail());
    }
    let core = core_winding.v_ref * core_winding.tap;
    let scale = primary_winding.s_rating * (v_nom[0] / core).powi(2);
    let shunt = Complex::new(scale / load.p_nom[0], scale / load.q_nom[0]);
    if [primary, secondary, shunt.re, shunt.im]
        .iter()
        .any(|v| !v.is_finite() || *v <= 0.0)
    {
        return Err(fail());
    }
    Ok(Center {
        phase,
        core_bus: &core_winding.bus,
        core_load: &load.name,
        primary,
        secondary,
        rating: primary_winding.s_rating,
        z: [z[0], z[1], z[2]],
        shunt,
    })
}

impl Writer<'_> {
    #[allow(clippy::too_many_lines)] // Root declaration includes current-law and point grouping.
    pub(super) fn service_bus(&mut self, bus: &crate::DistBus, phase: usize) -> Result<(), Error> {
        if self.source_voltages.contains_key(bus.id.as_str()) {
            return Err(unsupported(
                "triplex SWING sources need an additional output mapping",
            ));
        }
        let currents = self
            .net
            .loads()
            .iter()
            .filter(|load| {
                load.bus == bus.id
                    && matches!(
                        load.voltage_model,
                        DistLoadVoltageModel::ConstantCurrent { .. }
                    )
            })
            .collect::<Vec<_>>();
        let mut coefficients: BTreeMap<&str, Complex> = BTreeMap::new();
        let mut location = self
            .locations
            .get(&(DistLocationTarget::Bus, bus.id.as_str()))
            .copied();
        for load in &currents {
            let channels = service_channels(load)?;
            let DistLoadVoltageModel::ConstantCurrent { v_nom } = &load.voltage_model else {
                unreachable!()
            };
            if load.p_nom.len() != channels.len()
                || load.q_nom.len() != channels.len()
                || v_nom.len() != channels.len()
                || v_nom.iter().any(|v| !v.is_finite() || *v <= 0.0)
                || !load.extras.is_empty()
            {
                return Err(unsupported(
                    "triplex current loads require complete finite coefficients",
                ));
            }
            for (i, channel) in channels.into_iter().enumerate() {
                let power = Complex::new(load.p_nom[i], load.q_nom[i]);
                let value = power.conj() / v_nom[i];
                if !value.re.is_finite()
                    || !value.im.is_finite()
                    || (power != Complex::default() && value == Complex::default())
                {
                    return Err(unsupported(
                        "triplex current coefficient overflows or underflows",
                    ));
                }
                *coefficients.entry(channel).or_default() += value;
            }
            if let Some(point) = self
                .locations
                .get(&(DistLocationTarget::Load, load.name.as_str()))
                .copied()
            {
                if location.is_some_and(|p| p.point != point.point || p.kind != point.kind) {
                    return Err(unsupported(
                        "grouped triplex current loads have distinct geographic points",
                    ));
                }
                location = Some(point);
            }
        }
        // The NR bus invokes the root triplex_load's load-update callback each
        // iteration. A child current load instead retains its sync-time angle.
        let _ = writeln!(
            self.out,
            "object triplex_load {{ name \"{}\"; phases {}S; nominal_voltage {}; maximum_voltage_error 1e-9;",
            bus.id,
            phase_name(phase),
            self.nominal[bus.id.as_str()]
        );
        if let Some(point) = location {
            self.coordinates(point.target, &point.name);
        }
        for (channel, coefficient) in coefficients {
            let _ = writeln!(
                self.out,
                "constant_current_{channel} {};",
                complex_literal(coefficient)?
            );
        }
        self.out.push_str("}\n");
        if let Some(point) = self
            .locations
            .get(&(DistLocationTarget::Bus, bus.id.as_str()))
        {
            self.written_locations
                .insert((DistLocationTarget::Bus, point.name.as_str()));
        }
        for load in currents {
            self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,
                format!("load {}: current terms are combined on root triplex_load {} so native NR updates their voltage angle every iteration; the independent load identity becomes a root coefficient", load.name, bus.id));
            if self
                .locations
                .contains_key(&(DistLocationTarget::Load, load.name.as_str()))
            {
                self.written_locations
                    .insert((DistLocationTarget::Load, load.name.as_str()));
                self.dropped(format!("load {}: its coincident point is attached to grouped triplex_load {}; the independent attachment identity is not retained",load.name,bus.id));
            }
        }
        Ok(())
    }
    pub(super) fn center_transformer(&mut self, t: &DistTransformer) -> Result<(), Error> {
        let center = &self.services.centers[t.name.as_str()];
        let phase = center.phase;
        let primary = center.primary;
        let secondary = center.secondary;
        let rating = center.rating;
        let z = center.z;
        let shunt = center.shunt;
        let config = self.generated_name(&format!("{}__configuration", t.name));
        let _ = writeln!(
            self.out,
            "object transformer_configuration {{ name \"{config}\"; connect_type SINGLE_PHASE_CENTER_TAPPED; primary_voltage {primary}; secondary_voltage {secondary}; power_rating {}; power{}_rating {}; impedance {}; impedance1 {}; impedance2 {}; shunt_impedance {}; }}",
            rating / 1000.0,
            phase_name(phase),
            rating / 1000.0,
            complex_literal(z[0])?,
            complex_literal(z[1])?,
            complex_literal(z[2])?,
            complex_literal(shunt)?
        );
        let _ = writeln!(
            self.out,
            "object transformer {{ name \"{}\"; phases {}S; from \"{}\"; to \"{}\"; configuration \"{config}\";",
            t.name,
            phase_name(phase),
            t.windings[0].bus,
            t.windings[1].bus
        );
        self.coordinates(DistLocationTarget::Transformer, &t.name);
        self.out.push_str("}\n");
        self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,format!("transformer {}: current winding ratios/leakage and private impedance core load become one center-tapped circuit; auxiliary bus and core-load identities are internal to the native transformer",t.name));
        Ok(())
    }
    pub(super) fn service_load(&mut self, load: &DistLoad) -> Result<(), Error> {
        self.service_load_with_target(load, DistLocationTarget::Load)
    }
    pub(super) fn service_load_with_target(
        &mut self,
        load: &DistLoad,
        target: DistLocationTarget,
    ) -> Result<(), Error> {
        let fail = || {
            unsupported(format!(
                "service load {} needs another voltage-law/connection mapping",
                load.name
            ))
        };
        if !load.extras.is_empty() {
            return Err(fail());
        }
        if matches!(
            load.voltage_model,
            DistLoadVoltageModel::ConstantCurrent { .. }
        ) {
            return Ok(());
        }
        let channels = service_channels(load)?;
        let (law, v) = match &load.voltage_model {
            DistLoadVoltageModel::ConstantPower { v_nom } => ("power", v_nom),
            DistLoadVoltageModel::ConstantImpedance { v_nom } => ("impedance", v_nom),
            _ => return Err(fail()),
        };
        if load.p_nom.len() != channels.len()
            || load.q_nom.len() != channels.len()
            || (law != "power"
                && (v.len() != channels.len() || v.iter().any(|v| !v.is_finite() || *v <= 0.0)))
        {
            return Err(fail());
        }
        let phase = self.services.buses[load.bus.as_str()];
        let _ = writeln!(
            self.out,
            "object triplex_load {{ name \"{}\"; parent \"{}\"; phases {}S; nominal_voltage {};",
            load.name,
            load.bus,
            phase_name(phase),
            self.nominal[load.bus.as_str()]
        );
        self.coordinates(target, &load.name);
        for (i, channel) in channels.iter().enumerate() {
            let power = Complex::new(load.p_nom[i], load.q_nom[i]);
            let value = match law {
                "power" => power,
                _ if power == Complex::default() => Complex::default(),
                _ => v[i] * v[i] / power.conj(),
            };
            if power != Complex::default() && value == Complex::default() {
                return Err(unsupported("triplex load coefficient underflows"));
            }
            let _ = writeln!(
                self.out,
                "constant_{law}_{channel} {};",
                complex_literal(value)?
            );
        }
        self.out.push_str("}\n");
        Ok(())
    }
    pub(super) fn service_line(
        &mut self,
        line: &DistLine,
        code: &DistLineCode,
    ) -> Result<(), Error> {
        validate_line_matrices(line, code, code.n_conductors)?;
        if !line.extras.is_empty()
            || !code.extras.is_empty()
            || [&code.g_from, &code.g_to, &code.b_from, &code.b_to]
                .iter()
                .any(|m| m.iter().flatten().any(|v| *v != 0.0))
        {
            return Err(unsupported(
                "triplex line shunts/extras need an additional output mapping",
            ));
        }
        let phase = self.services.buses[line.bus_from.as_str()];
        let config = self.generated_name(&format!("{}__glm_configuration", line.name));
        if code.n_conductors == 2 {
            let _ = writeln!(
                self.out,
                "object triplex_line_configuration {{ name \"{config}\";"
            );
            self.coordinates(DistLocationTarget::LineCode, &code.name);
            for i in 0..2 {
                for j in 0..2 {
                    let z = Complex::new(code.r_series[i][j], code.x_series[i][j])
                        * METERS_PER_MILE
                        * if i == 0 { 1.0 } else { -1.0 };
                    let _ = writeln!(self.out, "z{}{} {};", i + 1, j + 1, complex_literal(z)?);
                }
            }
            self.out.push_str("}\n");
            self.dropped(format!("line {}: two hot-conductor equations emit a direct triplex matrix; no physical neutral-current model is inferred",line.name));
        } else {
            let geometry = triplex_geometry(code, self.net.base_frequency())?;
            let mut wires = Vec::new();
            for i in 0..3 {
                let name = self.generated_name(&format!("{}__glm_wire_{i}", line.name));
                let _ = writeln!(
                    self.out,
                    "object triplex_line_conductor {{ name \"{name}\"; resistance {}; geometric_mean_radius {}; }}",
                    geometry.resistance[i], geometry.gmr[i]
                );
                wires.push(name);
            }
            let _ = writeln!(
                self.out,
                "object triplex_line_configuration {{ name \"{config}\"; conductor_1 \"{}\"; conductor_2 \"{}\"; conductor_N \"{}\"; diameter {}; insulation_thickness {};",
                wires[0], wires[1], wires[2], geometry.diameter, geometry.insulation
            );
            self.coordinates(DistLocationTarget::LineCode, &code.name);
            self.out.push_str("}\n");
            self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,format!("line {}: a native triplex geometry is derived from current full matrices to preserve both hot and grounded-neutral currents; source geometry provenance is not recovered",line.name));
        }
        let _ = writeln!(
            self.out,
            "object triplex_line {{ name \"{}\"; phases {}S; from \"{}\"; to \"{}\"; length {} m; configuration \"{config}\";",
            line.name,
            phase_name(phase),
            line.bus_from,
            line.bus_to,
            line.length
        );
        self.coordinates(DistLocationTarget::Line, &line.name);
        self.out.push_str("}\n");
        if line.route.is_some()
            || line.i_max.is_some()
            || line.s_max.is_some()
            || code.i_max.is_some()
            || code.s_max.is_some()
        {
            self.dropped(format!(
                "line {}: routes and thermal ratings are not emitted in this triplex profile",
                line.name
            ));
        }
        Ok(())
    }
}
fn service_channels(load: &DistLoad) -> Result<Vec<&'static str>, Error> {
    let channels = match (
        load.configuration,
        load.terminal_map
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice(),
    ) {
        (Configuration::SinglePhase | Configuration::Delta, ["1", "n"] | ["n", "1"]) => {
            vec!["1"]
        }
        (Configuration::SinglePhase | Configuration::Delta, ["2", "n"] | ["n", "2"]) => {
            vec!["2"]
        }
        (Configuration::SinglePhase | Configuration::Delta, ["1", "2"] | ["2", "1"]) => {
            vec!["12"]
        }
        (Configuration::Wye, ["1", "2", "n"]) => vec!["1", "2"],
        _ => {
            return Err(unsupported(format!(
                "service load {} needs another connection mapping",
                load.name
            )));
        }
    };
    Ok(channels)
}

struct Geometry {
    resistance: [f64; 3],
    gmr: [f64; 3],
    diameter: f64,
    insulation: f64,
}
#[allow(clippy::float_cmp)] // Native geometry algebra, with only reconstruction roundoff admitted.
fn triplex_geometry(code: &DistLineCode, f: f64) -> Result<Geometry, Error> {
    let fail = || {
        unsupported(format!(
            "linecode {} requires another full-neutral triplex matrix equivalent",
            code.name
        ))
    };
    if code.n_conductors != 3 {
        return Err(fail());
    }
    let earth = (100.0 / f).ln() / 2.0 + 7.6786;
    let r_earth = 0.001_588_36 * f;
    let x_scale = 0.002_022_37 * f;
    if (0..3).any(|i| {
        (0..3).any(|j| {
            code.r_series[i][j] != code.r_series[j][i]
                || code.x_series[i][j] != code.x_series[j][i]
                || (i != j && code.r_series[i][j] != r_earth / METERS_PER_MILE)
        })
    }) || code.x_series[0][2] != code.x_series[1][2]
    {
        return Err(fail());
    }
    let resistance = std::array::from_fn(|i| code.r_series[i][i] * METERS_PER_MILE - r_earth);
    let gmr =
        std::array::from_fn(|i| (earth - code.x_series[i][i] * METERS_PER_MILE / x_scale).exp());
    let distance =
        |i: usize, j: usize| (earth - code.x_series[i][j] * METERS_PER_MILE / x_scale).exp();
    let d12 = distance(0, 1);
    let dn = distance(0, 2);
    let diameter = 12.0 * (2.0 * dn - d12);
    let insulation = 12.0 * (d12 - dn);
    if resistance
        .iter()
        .chain(&gmr)
        .chain([diameter, insulation].iter())
        .any(|v| !v.is_finite() || *v <= 0.0)
    {
        return Err(fail());
    }
    // Reconstruct the full primitive before emission; never fit or silently
    // replace a matrix outside this native geometry family.
    let distances = [
        [
            gmr[0],
            (diameter + 2.0 * insulation) / 12.0,
            (diameter + insulation) / 12.0,
        ],
        [
            (diameter + 2.0 * insulation) / 12.0,
            gmr[1],
            (diameter + insulation) / 12.0,
        ],
        [
            (diameter + insulation) / 12.0,
            (diameter + insulation) / 12.0,
            gmr[2],
        ],
    ];
    for (i, row) in distances.iter().enumerate() {
        for (j, distance) in row.iter().enumerate() {
            let r = (r_earth + if i == j { resistance[i] } else { 0.0 }) / METERS_PER_MILE;
            let x = x_scale * (earth - distance.ln()) / METERS_PER_MILE;
            for (a, b) in [(r, code.r_series[i][j]), (x, code.x_series[i][j])] {
                if (a - b).abs() > 8.0 * f64::EPSILON * a.abs().max(b.abs()) {
                    return Err(fail());
                }
            }
        }
    }
    Ok(Geometry {
        resistance,
        gmr,
        diameter,
        insulation,
    })
}
