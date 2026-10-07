//! Recover a canonical native transformer from a complete typed conductor
//! primitive. No source rows or provenance metadata supply electrical values.
use std::collections::BTreeSet;

use num_complex::Complex64;

use super::{
    write::{ExperimentalMulticonductorOptions, Result, error},
    write_transformer::coil_primitive,
};
use crate::{DistShunt, DistTransformer, DistWinding, DistWindingConn, MulticonductorNetwork};

type Primitive = [[Complex64; 6]; 6];
struct Port {
    bus: String,
    coordinates: Vec<String>,
    switch: usize,
}

/// Auxiliary six-coordinate buses have two distinct native ports, so they
/// cannot appear as a single entry in the returned native node-ID map.
pub(super) fn canonicalize(
    net: &MulticonductorNetwork,
    options: &ExperimentalMulticonductorOptions,
) -> Result<Option<(MulticonductorNetwork, ExperimentalMulticonductorOptions)>> {
    if net.shunts().is_empty() {
        return Ok(None);
    }
    let auxiliaries = net.shunts().iter().map(|s| &s.bus).collect::<BTreeSet<_>>();
    if options
        .nominal_ll_volts
        .iter()
        .any(|(name, v)| net.bus(name).is_none() || !v.is_finite() || *v <= 0.0)
        || net
            .buses()
            .iter()
            .any(|b| !auxiliaries.contains(&b.id) && !options.nominal_ll_volts.contains_key(&b.id))
    {
        return Err(error(
            "nominal_ll_volts must name every non-auxiliary bus with a positive finite voltage",
        ));
    }
    let mut out = net.clone();
    let mut levels = options.nominal_ll_volts.clone();
    let mut switches = BTreeSet::new();
    let mut buses = BTreeSet::new();
    for shunt in net.shunts() {
        let ports = ports(net, shunt)?;
        let primitive = primitive(shunt, &ports)?;
        let volts = [levels[&ports[0].bus], levels[&ports[1].bus]];
        let mut candidate = equivalent(&primitive, volts)?;
        candidate.name = format!("primitive:{}", shunt.name);
        for (side, w) in candidate.windings.iter_mut().enumerate() {
            w.bus.clone_from(&ports[side].bus);
            if w.conn == DistWindingConn::Wye {
                // Reuse an existing ground, or create a fresh solid star.
                // Never ground an existing floating conductor.
                let bus = out.buses_mut().iter_mut().find(|b| b.id == w.bus).unwrap();
                let ground = if let Some(g) = bus.grounded.first() {
                    g.clone()
                } else {
                    let mut name = "sincal:writer:earth".to_owned();
                    while bus.terminals.contains(&name) {
                        name.push('_');
                    }
                    bus.terminals.push(name.clone());
                    bus.grounded.push(name.clone());
                    name
                };
                w.terminal_map.push(ground);
            }
        }
        out.transformers_mut().push(candidate);
        switches.extend(ports.iter().map(|p| p.switch));
        buses.insert(shunt.bus.clone());
        levels.remove(&shunt.bus);
    }
    out.shunts_mut().clear();
    let mut index = 0;
    out.switches_mut().retain(|_| {
        let retain = !switches.contains(&index);
        index += 1;
        retain
    });
    out.buses_mut().retain(|b| !buses.contains(&b.id));
    Ok(Some((
        out,
        ExperimentalMulticonductorOptions {
            nominal_ll_volts: levels,
        },
    )))
}

fn ports(net: &MulticonductorNetwork, shunt: &DistShunt) -> Result<[Port; 2]> {
    let bus = net
        .bus(&shunt.bus)
        .ok_or_else(|| error("missing shunt bus"))?;
    if shunt.terminal_map.len() != 6
        || bus.terminals.len() != 6
        || !bus.grounded.is_empty()
        || shunt.terminal_map.iter().collect::<BTreeSet<_>>() != bus.terminals.iter().collect()
        || shunt.extras.keys().any(|key| key != "sincal_transformer")
    {
        return Err(error(
            "shunt is not a complete six-coordinate transformer candidate",
        ));
    }
    // The auxiliary bus must be exclusive to this primitive and its two
    // switches. Otherwise replacing it would discard attached circuitry.
    if net.shunts().iter().filter(|s| s.bus == bus.id).count() != 1
        || net.loads().iter().any(|l| l.bus == bus.id)
        || net.sources().iter().any(|s| s.bus == bus.id)
        || net
            .lines()
            .iter()
            .any(|l| l.bus_from == bus.id || l.bus_to == bus.id)
        || net
            .transformers()
            .iter()
            .any(|t| t.windings.iter().any(|w| w.bus == bus.id))
    {
        return Err(error(
            "transformer auxiliary coordinates have additional attached equipment",
        ));
    }
    let mut ports = Vec::new();
    for (index, s) in net.switches().iter().enumerate() {
        let (external, phases, coordinates) = if s.bus_to == bus.id {
            (&s.bus_from, &s.terminal_map_from, &s.terminal_map_to)
        } else if s.bus_from == bus.id {
            (&s.bus_to, &s.terminal_map_to, &s.terminal_map_from)
        } else {
            continue;
        };
        if s.open || external == &bus.id || phases != &["1", "2", "3"] || coordinates.len() != 3 {
            return Err(error(
                "transformer primitive requires two closed, ordered three-phase ports",
            ));
        }
        ports.push(Port {
            bus: external.clone(),
            coordinates: coordinates.clone(),
            switch: index,
        });
    }
    if ports
        .iter()
        .any(|p| net.shunts().iter().any(|s| s.bus == p.bus))
    {
        return Err(error(
            "transformer ports cannot be another primitive's auxiliary coordinates",
        ));
    }
    // Stable order follows the first coordinate in the typed primitive. A
    // reversed switch direction changes neither electrical port nor polarity.
    ports.sort_by_key(|p| {
        shunt
            .terminal_map
            .iter()
            .position(|c| c == &p.coordinates[0])
    });
    let [a, b]: [Port; 2] = ports
        .try_into()
        .map_err(|_| error("transformer primitive needs exactly two terminal switches"))?;
    let all = a
        .coordinates
        .iter()
        .chain(&b.coordinates)
        .collect::<BTreeSet<_>>();
    if all.len() != 6 || all != shunt.terminal_map.iter().collect() {
        return Err(error(
            "transformer ports overlap or omit auxiliary coordinates",
        ));
    }
    Ok([a, b])
}

fn primitive(shunt: &DistShunt, ports: &[Port; 2]) -> Result<Primitive> {
    if [&shunt.g, &shunt.b].iter().any(|m| {
        m.len() != 6
            || m.iter()
                .any(|r| r.len() != 6 || r.iter().any(|v| !v.is_finite()))
    }) {
        return Err(error(
            "transformer primitive must contain two finite 6 by 6 matrices",
        ));
    }
    let indices = ports
        .iter()
        .flat_map(|p| &p.coordinates)
        .map(|c| shunt.terminal_map.iter().position(|t| t == c).unwrap())
        .collect::<Vec<_>>();
    let mut result = [[Complex64::new(0.0, 0.0); 6]; 6];
    for i in 0..6 {
        for j in 0..6 {
            result[i][j] = Complex64::new(
                shunt.g[indices[i]][indices[j]],
                shunt.b[indices[i]][indices[j]],
            );
        }
    }
    Ok(result)
}

/// Project one diagonal block onto positive sequence only to propose scalar
/// parameters. Acceptance then compares EVERY conductor entry, including zero
/// and negative sequence and any sequence coupling. This is not a projection
/// of the network to a balanced model.
fn positive(y: &Primitive, side: usize) -> Complex64 {
    // For a reciprocal transposed block, Y1=Y2=(trace-offdiag/2)/3.
    // Real weights preserve exact zero conductance for lossless windings.
    let mut result = Complex64::new(0.0, 0.0);
    for i in 0..3 {
        for j in 0..3 {
            result += y[3 * side + i][3 * side + j] * if i == j { 1.0 / 3.0 } else { -1.0 / 6.0 };
        }
    }
    result
}

fn equivalent(y: &Primitive, volts: [f64; 2]) -> Result<DistTransformer> {
    let primary = positive(y, 0);
    let secondary = positive(y, 1);
    let z = Complex64::new(1.0, 0.0) / secondary;
    let ratio = (secondary.norm() / primary.norm()).sqrt();
    // A shunt has no nameplate VA rating. This is a canonical parameter base,
    // not a recovered thermal limit. Its choice cancels from the circuit.
    let rating = 1e6;
    let zbase = volts[1] * volts[1] / rating;
    let r_pct = z.re / zbase * 100.0;
    let x_pct = z.im / zbase * 100.0;
    let tap = ratio * volts[1] / volts[0];
    if ![r_pct, x_pct, tap].iter().all(|v| v.is_finite())
        || r_pct < 0.0
        || x_pct < 0.0
        || tap <= 0.0
    {
        return Err(error(
            "transformer primitive has no finite passive leakage candidate",
        ));
    }
    for kind in 0..3 {
        for reverse in [false, true] {
            let mut windings = Vec::new();
            for (side, &voltage) in volts.iter().enumerate() {
                let conn = if kind == side + 1 {
                    DistWindingConn::Wye
                } else {
                    DistWindingConn::Delta
                };
                let mut winding = DistWinding::new(
                    "",
                    ["1", "2", "3"].map(str::to_owned).to_vec(),
                    conn,
                    voltage,
                    rating,
                );
                winding.r_pct = r_pct / 2.0;
                winding.tap = if side == 0 { tap } else { 1.0 };
                windings.push(winding);
            }
            let mut t = DistTransformer::new("", windings, vec![x_pct], 3);
            let lead = reverse ^ (volts[0] < volts[1]);
            t.extras.insert(
                "leadlag".into(),
                serde_json::json!(if lead { "lead" } else { "lag" }),
            );
            let expected = coil_primitive(&t, reverse)?;
            if matches(y, &expected) {
                return Ok(t);
            }
        }
    }
    Err(error(
        "coupled shunt has no verified delta/delta or solid delta/Wye transformer representation",
    ))
}

fn matches(actual: &Primitive, expected: &Primitive) -> bool {
    (0..6).all(|i| {
        (0..6).all(|j| {
            let scale = expected[3 * (i / 3)..3 * (i / 3) + 3]
                .iter()
                .flat_map(|r| &r[3 * (j / 3)..3 * (j / 3) + 3])
                .map(|z| z.norm())
                .fold(0.0_f64, f64::max);
            (actual[i][j] - expected[i][j]).norm() <= 1e-10 * scale
        })
    })
}
