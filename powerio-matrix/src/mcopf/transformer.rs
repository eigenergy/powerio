//! Winding-current descriptor; no inverse of a leakage impedance is formed.
use super::{
    BTreeSet, Configuration, Context, Deserialize, McAcOpfAssemblyOptions, McOpfBranch, McOpfShunt,
    McOpfTerminal, Result, Serialize, caps, extras, finite, invalid, join, root, unsupported,
};
use powerio_dist::DistWindingConn;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
/// Complex affine expression with no constant term. Coefficients are [real, imaginary].
/// Voltage indices address preparation.terminals; current indices address the
/// owning transformer's coils. Rows of equations are constrained to zero.
pub struct McOpfComplexRow {
    /// Sparse terminal-voltage coefficients in per unit.
    pub voltage: Vec<(usize, [f64; 2])>,
    /// Sparse local winding-current coefficients in per unit.
    pub current: Vec<(usize, [f64; 2])>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
/// A winding coil oriented from positive to negative, with a retained current unknown.
pub struct McOpfTransformerCoil {
    /// Global positive terminal index.
    pub positive: usize,
    /// Global negative terminal index, or physical ground.
    pub negative: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
/// A rated physical terminal/coil port reconstructed from winding currents.
pub struct McOpfTransformerPort {
    /// Winding/terminal port identity within the transformer.
    pub name: String,
    /// Global positive voltage terminal for this port.
    pub positive: usize,
    /// Global return voltage terminal, or physical ground.
    pub negative: Option<usize>,
    /// Complex per-unit port current reconstructed from local coil currents and terminal voltages.
    pub current: McOpfComplexRow,
    /// Optional per-unit current magnitude bound.
    pub current_max: Option<f64>,
    /// Optional per-unit apparent-power bound at this port.
    pub apparent_max: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
/// Winding-current equations, ports and optional continuous tap parameter.
/// Each row is `equations[k] + t * tap_equations[k] = 0`, where `t` is a
/// dimensionless multiplier with nominal value one. The physical tap is
/// `tap_nominal * t`, or `tap_nominal / t` when `tap_inverse` is true.
/// An empty tap_equations vector means the descriptor is fixed.
pub struct McOpfTransformer {
    /// Canonical transformer name.
    pub identity: String,
    /// Local winding-current axis and terminal incidence; these currents enter network KCL.
    pub coils: Vec<McOpfTransformerCoil>,
    /// Complex zero-residual rows using global voltages and local winding currents.
    pub equations: Vec<McOpfComplexRow>,
    /// Physical rated ports, with axes distinct from the internal coil axis.
    pub ports: Vec<McOpfTransformerPort>,
    /// Optional multiplier on the nominal tap coefficient (start = 1).
    pub tap_bounds: Option<[f64; 2]>,
    /// Physical nominal tap used to recover a physical tap from the multiplier.
    pub tap_nominal: f64,
    /// True for the inverse multiplier convention of type-B regulators.
    pub tap_inverse: bool,
    /// Rows multiplied by the tap multiplier, aligned with equations; empty for fixed taps.
    pub tap_equations: Vec<McOpfComplexRow>,
}
fn add_voltage(row: &mut McOpfComplexRow, coil: &McOpfTransformerCoil, value: f64) {
    row.voltage.push((coil.positive, [value, 0.0]));
    if let Some(n) = coil.negative {
        row.voltage.push((n, [-value, 0.0]));
    }
}
fn extra_number(t: &powerio_dist::DistTransformer, key: &str, default: f64) -> Result<f64> {
    let v = t
        .extras
        .get(key)
        .map_or(Some(default), serde_json::Value::as_f64)
        .ok_or_else(|| invalid(&t.name, format!("invalid {key}")))?;
    finite(&t.name, &[v])?;
    Ok(v)
}
fn extra_vector(t: &powerio_dist::DistTransformer, key: &str) -> Result<Option<Vec<f64>>> {
    t.extras
        .get(key)
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|_| invalid(&t.name, format!("invalid {key}")))
        })
        .transpose()
}
fn stamp_shunt(
    shunts: &mut Vec<McOpfShunt>,
    name: String,
    coil: &McOpfTransformerCoil,
    g: f64,
    b: f64,
) {
    if g == 0.0 && b == 0.0 {
        return;
    }
    let (terminals, matrix) = if let Some(n) = coil.negative {
        (
            vec![coil.positive, n],
            vec![vec![1.0, -1.0], vec![-1.0, 1.0]],
        )
    } else {
        (vec![coil.positive], vec![vec![1.0]])
    };
    shunts.push(McOpfShunt {
        identity: name,
        terminals,
        g: matrix
            .iter()
            .map(|r| r.iter().map(|x| x * g).collect())
            .collect(),
        b: matrix
            .iter()
            .map(|r| r.iter().map(|x| x * b).collect())
            .collect(),
    });
}
#[derive(Clone, Copy)]
struct TransformerProfile<'a> {
    t: &'a powerio_dist::DistTransformer,
    subtype: &'a str,
    nw: bool,
    regulator: bool,
    phases: usize,
    zb: f64,
    ib: f64,
    sb: f64,
}
type TerminalRatings = (Option<Vec<f64>>, Option<f64>);
type WindingGeometry = (Vec<super::CoilPairs>, Vec<f64>, Vec<f64>);

pub(super) fn prepare(
    ctx: &Context<'_>,
    bases: &McAcOpfAssemblyOptions,
    branches: &mut Vec<McOpfBranch>,
    shunts: &mut Vec<McOpfShunt>,
    terminals: &[McOpfTerminal],
) -> Result<Vec<McOpfTransformer>> {
    let mut out = Vec::new();
    let sb = bases.power_base_va;
    let zb = bases.voltage_base_v.powi(2) / sb;
    let ib = sb / bases.voltage_base_v;
    for t in ctx.net.transformers() {
        validate_extras(t)?;
        for key in ["bmopf_winding_metadata", "bmopf_delta_rolls"] {
            if let Some(value) = t.extras.get(key)
                && !value.is_object()
            {
                return Err(invalid(&t.name, format!("{key} must be an object")));
            }
        }
        let subtype = t
            .extras
            .get("bmopf_subtype")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| invalid(&t.name, "invalid transformer subtype"))
            })
            .transpose()?
            .unwrap_or("canonical");
        let regulator = matches!(
            subtype,
            "single_phase_autotransformer" | "open_delta_regulator"
        );
        let nw = subtype == "n_winding" || subtype == "canonical";
        if !matches!(
            subtype,
            "canonical"
                | "n_winding"
                | "single_phase"
                | "center_tap"
                | "wye_delta"
                | "delta_wye"
                | "single_phase_autotransformer"
                | "open_delta_regulator"
        ) {
            return Err(unsupported(&t.name, "unknown transformer subtype"));
        }
        let m = t.windings.len();
        if m < 2 || t.xsc_pct.len() != m * (m - 1) / 2 {
            return Err(invalid(&t.name, "invalid winding/leakage dimensions"));
        }
        let (maps, mut noms, mut resistances) = winding_geometry(ctx, t, subtype, nw)?;
        let phases = maps[0].len();
        if maps.iter().any(|p| p.len() != phases) {
            return Err(invalid(&t.name, "all windings must have equal coil count"));
        }
        if regulator {
            prepare_regulator(
                ctx,
                t,
                &maps,
                &mut noms,
                &mut resistances,
                branches,
                terminals,
            )?;
        }
        let profile = TransformerProfile {
            t,
            subtype,
            nw,
            regulator,
            phases,
            zb,
            ib,
            sb,
        };
        let mut tx = leakage_descriptor(&profile, &maps, &noms, &resistances)?;
        prepare_ports(&profile, ctx, &maps, &mut tx, shunts)?;
        for row in &tx.equations {
            for (_, c) in row.voltage.iter().chain(&row.current) {
                finite(&t.name, c)?;
            }
        }
        explicit_core(&profile, &tx, shunts)?;
        prepare_tap(&profile, &noms, &mut tx)?;
        out.push(tx);
    }
    Ok(out)
}
/// Solve the no-load voltage equations for starts. Free common modes are zero;
/// these starts never impose a physical voltage reference on the OPF.
pub(super) fn initialize(
    branches: &[McOpfBranch],
    transformers: &[McOpfTransformer],
    terminals: &mut [McOpfTerminal],
) -> Result<()> {
    let n = terminals.len();
    let mut rows = Vec::new();
    let mut connected: Vec<_> = (0..n).collect();
    for (k, t) in terminals.iter().enumerate() {
        if let Some(v) = t.fixed {
            let mut row = vec![0.0; n + 2];
            row[k] = 1.0;
            row[n] = v[0];
            row[n + 1] = v[1];
            rows.push(row);
        }
    }
    for br in branches.iter().filter(|b| !b.open) {
        for (&a, &b) in br.from.iter().zip(&br.to) {
            let mut row = vec![0.0; n + 2];
            row[a] = 1.0;
            row[b] -= 1.0;
            rows.push(row);
            join(&mut connected, a, b);
        }
    }
    for tx in transformers {
        for coil in &tx.coils {
            if let Some(q) = coil.negative {
                join(&mut connected, coil.positive, q);
            }
            join(&mut connected, tx.coils[0].positive, coil.positive);
        }
        for (index, eq) in tx.equations.iter().enumerate() {
            if eq.voltage.is_empty() {
                continue;
            }
            let mut row = vec![0.0; n + 2];
            for &(k, c) in eq.voltage.iter().chain(
                tx.tap_equations
                    .get(index)
                    .into_iter()
                    .flat_map(|r| &r.voltage),
            ) {
                row[k] += c[0];
            }
            rows.push(row);
        }
    }
    let anchors: BTreeSet<_> = terminals
        .iter()
        .enumerate()
        .filter(|(_, t)| t.fixed.is_some())
        .map(|(k, _)| root(&mut connected, k))
        .collect();
    for (k, t) in terminals.iter().enumerate() {
        if !anchors.contains(&root(&mut connected, k)) {
            return Err(unsupported(&t.bus, "unreferenced transformer island"));
        }
    }
    let mut pivot = 0;
    let mut pivots = Vec::new();
    for col in 0..n {
        let best =
            (pivot..rows.len()).max_by(|&a, &b| rows[a][col].abs().total_cmp(&rows[b][col].abs()));
        let Some(best) = best.filter(|&r| rows[r][col].abs() > 1e-12) else {
            continue;
        };
        rows.swap(pivot, best);
        let d = rows[pivot][col];
        for v in &mut rows[pivot][col..] {
            *v /= d;
        }
        let row = rows[pivot].clone();
        for (j, r) in rows.iter_mut().enumerate() {
            if j != pivot {
                let f = r[col];
                for k in col..n + 2 {
                    r[k] -= f * row[k];
                }
            }
        }
        pivots.push((pivot, col));
        pivot += 1;
    }
    for t in terminals.iter_mut().filter(|t| t.fixed.is_none()) {
        t.start = [0.0, 0.0];
    }
    for (r, c) in pivots {
        if terminals[c].fixed.is_none() {
            terminals[c].start = [rows[r][n], rows[r][n + 1]];
        }
    }
    Ok(())
}

fn explicit_core(
    profile: &TransformerProfile<'_>,
    tx: &McOpfTransformer,
    shunts: &mut Vec<McOpfShunt>,
) -> Result<()> {
    let TransformerProfile { t, phases, zb, .. } = *profile;
    let m = t.windings.len();
    if let Some(core) = t.extras.get("no_load_shunt") {
        if t.extras.contains_key("g_no_load") || t.extras.contains_key("b_no_load") {
            return Err(invalid(&t.name, "competing no-load representations"));
        }
        let object = core
            .as_object()
            .ok_or_else(|| invalid(&t.name, "no_load_shunt must be an object"))?;
        if object
            .keys()
            .any(|k| !["winding", "g", "b"].contains(&k.as_str()))
        {
            return Err(unsupported(&t.name, "unknown no-load field"));
        }
        let winding = core["winding"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v > 0 && *v <= m)
            .ok_or_else(|| invalid(&t.name, "invalid no-load winding"))?
            - 1;
        let g = core["g"]
            .as_f64()
            .ok_or_else(|| invalid(&t.name, "missing no-load conductance"))?;
        let b = core["b"]
            .as_f64()
            .ok_or_else(|| invalid(&t.name, "missing no-load susceptance"))?;
        finite(&t.name, &[g, b])?;
        if g < 0.0 {
            return Err(invalid(&t.name, "negative no-load conductance"));
        }
        for k in 0..phases {
            stamp_shunt(
                shunts,
                format!("transformer:{}:explicit_core:{k}", t.name),
                &tx.coils[winding * phases + k],
                g * zb,
                b * zb,
            );
        }
    }
    Ok(())
}

fn prepare_tap(
    profile: &TransformerProfile<'_>,
    noms: &[f64],
    tx: &mut McOpfTransformer,
) -> Result<()> {
    let TransformerProfile {
        t, nw, regulator, ..
    } = *profile;
    if regulator && (t.extras.contains_key("tap_min") || t.extras.contains_key("tap_max")) {
        return Err(unsupported(
            &t.name,
            "regulators require tap_ratio_min/max, not ordinary transformer tap_min/max",
        ));
    }
    let keys = if regulator {
        ["tap_ratio_min", "tap_ratio_max"]
    } else {
        ["tap_min", "tap_max"]
    };
    if t.extras.contains_key(keys[0]) || t.extras.contains_key(keys[1]) {
        if nw {
            return Err(unsupported(
                &t.name,
                "n-winding tap optimization is not supported by the reference engine",
            ));
        }
        let read = |key: &str| -> Result<f64> {
            let v = t
                .extras
                .get(key)
                .ok_or_else(|| invalid(&t.name, "tap requires both bounds"))?;
            let v = if let Some(meta) = t.extras.get("bmopf_open_delta") {
                v.get(
                    meta["leg"]
                        .as_u64()
                        .ok_or_else(|| invalid(&t.name, "missing tap leg"))?
                        as usize,
                )
                .unwrap_or(v)
            } else {
                v
            };
            let value = v
                .as_f64()
                .ok_or_else(|| invalid(&t.name, "invalid tap bound"))?;
            finite(&t.name, &[value])?;
            Ok(value)
        };
        let lo = read(keys[0])?;
        let hi = read(keys[1])?;
        if lo <= 0.0 || lo >= hi {
            return Err(invalid(
                &t.name,
                "tap bounds must be positive and strictly ordered",
            ));
        }
        tx.tap_nominal = if regulator {
            t.windings[1].tap
        } else {
            t.windings[0].tap
        };
        tx.tap_inverse = regulator
            && t.extras
                .get("regulator_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("B")
                == "B";
        tx.tap_bounds = Some(if tx.tap_inverse {
            [tx.tap_nominal / hi, tx.tap_nominal / lo]
        } else {
            [lo / tx.tap_nominal, hi / tx.tap_nominal]
        });
        tap_equations(profile, noms, tx)?;
    }
    Ok(())
}

fn prepare_ports(
    profile: &TransformerProfile<'_>,
    ctx: &Context<'_>,
    maps: &[super::CoilPairs],
    tx: &mut McOpfTransformer,
    shunts: &mut Vec<McOpfShunt>,
) -> Result<()> {
    let TransformerProfile {
        t,
        subtype,
        nw,
        regulator,
        phases,
        zb,
        ib,
        sb,
    } = *profile;
    let shunt_side = usize::from(!regulator);
    let shunt_divisor = if nw || subtype == "center_tap" || regulator {
        1.0
    } else {
        phases as f64
    };
    let g = extra_number(t, "g_no_load", 0.0)? * zb / shunt_divisor;
    let b = extra_number(t, "b_no_load", 0.0)? * zb / shunt_divisor;
    for k in 0..phases {
        stamp_shunt(
            shunts,
            format!("transformer:{}:core:{k}", t.name),
            &tx.coils[shunt_side * phases + k],
            g,
            b,
        );
    }
    for (j, w) in t.windings.iter().enumerate() {
        let meta = t
            .extras
            .get("bmopf_winding_metadata")
            .and_then(|v| v.get(j.to_string()));
        if let Some(meta) = meta {
            let obj = meta
                .as_object()
                .ok_or_else(|| invalid(&t.name, "winding metadata must be an object"))?;
            for key in obj.keys() {
                if !["i_max", "s_max"].contains(&key.as_str()) {
                    return Err(unsupported(&t.name, format!("winding field {key}")));
                }
            }
        }
        let imax = if nw {
            meta.and_then(|v| v.get("i_max"))
                .map(|v| {
                    v.as_f64()
                        .ok_or_else(|| invalid(&t.name, "invalid winding current cap"))
                })
                .transpose()?
                .map(|v| vec![v; phases])
        } else {
            extra_vector(t, if j == 0 { "i_max_from" } else { "i_max_to" })?
        };
        // Native Yd/Dy current ratings apply to terminal line currents; n-winding ratings to coils.
        let (imax, neutral_cap) = terminal_current_ratings(profile, ctx, &maps[j], j, imax)?;
        if let Some(cap) = neutral_cap {
            neutral_port(profile, tx, &maps[j], j, [g, b], cap)?;
        }
        let imax = caps(&t.name, imax.as_ref(), phases, ib)?;
        let rating = if nw {
            meta.and_then(|v| v.get("s_max"))
                .map(|v| {
                    v.as_f64()
                        .ok_or_else(|| invalid(&t.name, "invalid winding apparent-power rating"))
                })
                .transpose()?
        } else if j == usize::from(subtype == "delta_wye") {
            Some(w.s_rating)
        } else {
            None
        };
        let smax = caps(
            &t.name,
            rating.map(|v| vec![v / (phases as f64); phases]).as_ref(),
            phases,
            sb,
        )?;
        phase_ports(profile, tx, j, &imax, &smax, [g, b]);
        grounding(profile, w, &maps[j], j, shunts)?;
    }
    Ok(())
}

#[allow(clippy::float_cmp)] // Tap=1 is an exact schema sentinel, not a numerical tolerance.
fn winding_geometry(
    ctx: &Context<'_>,
    t: &powerio_dist::DistTransformer,
    subtype: &str,
    nw: bool,
) -> Result<WindingGeometry> {
    let mut maps = Vec::new();
    let mut noms = Vec::new();
    let mut resistances = Vec::new();
    for (j, w) in t.windings.iter().enumerate() {
        finite(&t.name, &[w.v_ref, w.s_rating, w.r_pct, w.tap])?;
        if w.v_ref <= 0.0 || w.s_rating <= 0.0 || w.tap <= 0.0 || w.r_pct < 0.0 {
            return Err(invalid(&t.name, "invalid winding nameplate/tap"));
        }
        let config = if w.conn == DistWindingConn::Delta {
            Configuration::Delta
        } else if w.terminal_map.len() == 2 && (!nw || t.phases == 1) {
            Configuration::SinglePhase
        } else {
            Configuration::Wye
        };
        let (_, mut pairs) = ctx.coils(&t.name, &w.bus, &w.terminal_map, config)?;
        if subtype == "n_winding" && w.conn == DistWindingConn::Delta && w.terminal_map.len() == 2 {
            let ids = ctx.resolve(&t.name, &w.bus, &w.terminal_map)?;
            pairs = vec![(ids[0], Some(ids[1])), (ids[1], Some(ids[0]))];
        }
        if subtype == "n_winding" && w.tap != 1.0 {
            return Err(unsupported(
                &t.name,
                "n-winding tap fields are not supported by BMOPFTools",
            ));
        }
        let n = pairs.len();
        if w.conn == DistWindingConn::Delta {
            let roll = t
                .extras
                .get("bmopf_delta_rolls")
                .and_then(|v| v.get((j + 1).to_string()))
                .map(|v| {
                    v.as_i64()
                        .ok_or_else(|| invalid(&t.name, "invalid delta roll"))
                })
                .transpose()?
                .unwrap_or(if subtype == "delta_wye" { -1 } else { 1 });
            if roll != 1 && roll != -1 {
                return Err(invalid(&t.name, "delta roll must be +1 or -1"));
            }
            if roll == -1 && n > 2 {
                let positives: Vec<_> = pairs.iter().map(|p| p.0).collect();
                for k in 0..n {
                    pairs[k].1 = Some(positives[(k + n - 1) % n]);
                }
            }
        }
        let nominal = if nw {
            if w.conn == DistWindingConn::Wye && n > 1 {
                w.v_ref / 3f64.sqrt()
            } else {
                w.v_ref
            }
        } else if w.conn == DistWindingConn::Delta {
            w.v_ref * 3f64.sqrt()
        } else {
            w.v_ref
        };
        let base = if nw {
            nominal.powi(2) * winding_base_phases(w, n) / w.s_rating
        } else {
            nominal.powi(2) / w.s_rating
        };
        noms.push(nominal * w.tap);
        resistances.push(w.r_pct / 100.0 * base * w.tap.powi(2));
        maps.push(pairs);
    }
    Ok((maps, noms, resistances))
}

fn prepare_regulator(
    ctx: &Context<'_>,
    t: &powerio_dist::DistTransformer,
    maps: &[super::CoilPairs],
    noms: &mut Vec<f64>,
    resistances: &mut Vec<f64>,
    branches: &mut Vec<McOpfBranch>,
    terminals: &[McOpfTerminal],
) -> Result<()> {
    let m = t.windings.len();
    let phases = maps[0].len();
    if m != 2 || phases != 1 {
        return Err(invalid(&t.name, "regulator must have two single coils"));
    }
    let tap = t.windings[1].tap;
    let ratio = match t
        .extras
        .get("regulator_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("B")
    {
        "A" => tap,
        "B" => 1.0 / tap,
        _ => return Err(invalid(&t.name, "unknown regulator type")),
    };
    if ratio <= 0.0 {
        return Err(invalid(&t.name, "nonpositive effective regulator ratio"));
    }
    *noms = vec![ratio, 1.0];
    *resistances = vec![
        extra_number(t, "r_series_from", 0.0)?,
        extra_number(t, "r_series_to", 0.0)?,
    ];
    let bond = if let Some(meta) = t.extras.get("bmopf_open_delta") {
        let k = meta["shared"]
            .as_u64()
            .ok_or_else(|| invalid(&t.name, "missing regulator shared phase"))?
            .checked_sub(1)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| invalid(&t.name, "invalid shared phase index"))?;
        let get = |side: &str, j: usize| -> Result<usize> {
            let label = meta[side]
                .get(k)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid(&t.name, "invalid regulator terminal map"))?;
            Ok(ctx.resolve(&t.name, &t.windings[j].bus, &[label.to_owned()])?[0])
        };
        (Some(get("from", 0)?), Some(get("to", 1)?))
    } else {
        (maps[0][0].1, maps[1][0].1)
    };
    if let (Some(a), Some(b)) = bond
        && !(terminals[a].grounded && terminals[b].grounded)
        && !branches.iter().any(|br| {
            br.from == [a] && br.to == [b] && br.identity.starts_with("transformer-bond:")
        })
    {
        let z = vec![vec![0.0]];
        branches.push(McOpfBranch {
            identity: format!("transformer-bond:{}", t.name),
            from: vec![a],
            to: vec![b],
            r: z.clone(),
            x: z.clone(),
            g_from: z.clone(),
            b_from: z.clone(),
            g_to: z.clone(),
            b_to: z,
            current_max: vec![None],
            apparent_max: vec![None],
            open: false,
        });
    }
    Ok(())
}

fn leakage_descriptor(
    profile: &TransformerProfile<'_>,
    maps: &[super::CoilPairs],
    noms: &[f64],
    resistances: &[f64],
) -> Result<McOpfTransformer> {
    let TransformerProfile {
        t,
        nw,
        regulator,
        phases,
        zb,
        ..
    } = *profile;
    let m = t.windings.len();
    let ratios: Vec<_> = noms.iter().map(|n| n / noms[0]).collect();
    let mut zpair = vec![vec![[0.0, 0.0]; m]; m];
    let mut index = 0;
    let ref_base = if nw {
        (noms[0] / t.windings[0].tap).powi(2) * winding_base_phases(&t.windings[0], phases)
            / extra_number(t, "bmopf_power_base", t.windings[0].s_rating)?
    } else {
        (noms[0] / t.windings[0].tap).powi(2) / t.windings[0].s_rating
    };
    if ref_base <= 0.0 {
        return Err(invalid(&t.name, "nonpositive transformer power base"));
    }
    for a in 0..m {
        for b in a + 1..m {
            let x = if regulator {
                extra_number(t, "x_series_from", 0.0)?
                    + extra_number(t, "x_series_to", 0.0)? / ratios[1].powi(2)
            } else {
                t.xsc_pct[index] / 100.0 * ref_base * t.windings[0].tap.powi(2)
            };
            zpair[a][b] = [
                (resistances[a] / ratios[a].powi(2) + resistances[b] / ratios[b].powi(2)) / zb,
                x / zb,
            ];
            zpair[b][a] = zpair[a][b];
            index += 1;
        }
    }
    let mut tx = McOpfTransformer {
        identity: t.name.clone(),
        coils: Vec::new(),
        equations: Vec::new(),
        ports: Vec::new(),
        tap_bounds: None,
        tap_nominal: 1.0,
        tap_inverse: false,
        tap_equations: Vec::new(),
    };
    for map in maps {
        for &(positive, negative) in map {
            tx.coils.push(McOpfTransformerCoil { positive, negative });
        }
    }
    for k in 0..phases {
        let mut balance = McOpfComplexRow::default();
        for (j, &ratio) in ratios.iter().enumerate() {
            balance.current.push((j * phases + k, [ratio, 0.0]));
        }
        tx.equations.push(balance);
        for a in 1..m {
            let mut row = McOpfComplexRow::default();
            add_voltage(&mut row, &tx.coils[k], 1.0);
            add_voltage(&mut row, &tx.coils[a * phases + k], -1.0 / ratios[a]);
            for b in 1..m {
                let z = if a == b {
                    zpair[0][a]
                } else {
                    [
                        (zpair[0][a][0] + zpair[0][b][0] - zpair[a][b][0]) * 0.5,
                        (zpair[0][a][1] + zpair[0][b][1] - zpair[a][b][1]) * 0.5,
                    ]
                };
                row.current
                    .push((b * phases + k, [z[0] * ratios[b], z[1] * ratios[b]]));
            }
            tx.equations.push(row);
        }
    }
    Ok(tx)
}

fn grounding(
    profile: &TransformerProfile<'_>,
    w: &powerio_dist::DistWinding,
    map: &super::CoilPairs,
    j: usize,
    shunts: &mut Vec<McOpfShunt>,
) -> Result<()> {
    let TransformerProfile { t, zb, .. } = *profile;
    let rn = w.r_neutral.unwrap_or(0.0);
    let xn = w.x_neutral.unwrap_or(0.0);
    finite(&t.name, &[rn, xn])?;
    if rn < 0.0 {
        return Err(unsupported(
            &t.name,
            "negative neutral resistance must be normalized to an explicit open grounding branch",
        ));
    }
    if rn != 0.0 || xn != 0.0 {
        if w.conn == DistWindingConn::Delta {
            return Err(unsupported(&t.name, "delta neutral impedance"));
        }
        let neutral = map[0]
            .1
            .ok_or_else(|| invalid(&t.name, "neutral impedance without neutral"))?;
        let d = rn * rn + xn * xn;
        stamp_shunt(
            shunts,
            format!("transformer:{}:neutral:{j}", t.name),
            &McOpfTransformerCoil {
                positive: neutral,
                negative: None,
            },
            rn / d * zb,
            -xn / d * zb,
        );
    }
    Ok(())
}

fn phase_ports(
    profile: &TransformerProfile<'_>,
    tx: &mut McOpfTransformer,
    j: usize,
    imax: &[Option<f64>],
    smax: &[Option<f64>],
    core: [f64; 2],
) {
    let TransformerProfile {
        t,
        subtype,
        nw,
        phases,
        ..
    } = *profile;
    let [g, b] = core;
    let delta_terminal = !nw && t.windings[j].conn == DistWindingConn::Delta;
    for k in 0..phases {
        let coil = &tx.coils[j * phases + k];
        let mut current = McOpfComplexRow::default();
        current.current.push((j * phases + k, [1.0, 0.0]));
        if delta_terminal {
            current.current.clear();
            for q in 0..phases {
                let c = &tx.coils[j * phases + q];
                if c.positive == coil.positive {
                    current.current.push((j * phases + q, [1.0, 0.0]));
                }
                if c.negative == Some(coil.positive) {
                    current.current.push((j * phases + q, [-1.0, 0.0]));
                }
            }
        }
        // Through-power uses bare winding current. YY output and single-phase
        // regulator input current ratings include the exciting current.
        let apparent_current = current.clone();
        let magnetized = (matches!(subtype, "single_phase" | "center_tap") && j == 1)
            || (subtype == "single_phase_autotransformer" && j == 0);
        if magnetized {
            current.voltage.push((coil.positive, [g, b]));
            if let Some(n) = coil.negative {
                current.voltage.push((n, [-g, -b]));
            }
        }
        tx.ports.push(McOpfTransformerPort {
            name: format!("w{j}:{k}"),
            positive: coil.positive,
            negative: coil.negative,
            current,
            current_max: imax[k],
            apparent_max: if magnetized { None } else { smax[k] },
        });
        if magnetized && smax[k].is_some() {
            tx.ports.push(McOpfTransformerPort {
                name: format!("w{j}:{k}:through"),
                positive: coil.positive,
                negative: coil.negative,
                current: apparent_current,
                current_max: None,
                apparent_max: smax[k],
            });
        }
    }
}

fn neutral_port(
    profile: &TransformerProfile<'_>,
    tx: &mut McOpfTransformer,
    map: &super::CoilPairs,
    j: usize,
    core: [f64; 2],
    cap: f64,
) -> Result<()> {
    let TransformerProfile {
        t, subtype, phases, ..
    } = *profile;
    let [g, b] = core;
    let neutral = map[0]
        .1
        .ok_or_else(|| invalid(&t.name, "neutral cap without neutral"))?;
    let mut current = McOpfComplexRow::default();
    if subtype == "center_tap" {
        current.current = vec![(1, [-1.0, 0.0]), (2, [1.0, 0.0])];
        current.voltage.push((tx.coils[1].positive, [-g, -b]));
        current.voltage.push((neutral, [g, b]));
    } else {
        current.current = (0..phases).map(|k| (j * phases + k, [-1.0, 0.0])).collect();
    }
    tx.ports.push(McOpfTransformerPort {
        name: format!("w{j}:neutral"),
        positive: neutral,
        negative: None,
        current,
        current_max: Some(cap),
        apparent_max: None,
    });
    Ok(())
}

fn terminal_current_ratings(
    profile: &TransformerProfile<'_>,
    ctx: &Context<'_>,
    map: &super::CoilPairs,
    j: usize,
    imax: Option<Vec<f64>>,
) -> Result<TerminalRatings> {
    let TransformerProfile {
        t,
        subtype,
        phases,
        ib,
        ..
    } = *profile;
    let w = &t.windings[j];
    let imax = if let Some(meta) = t.extras.get("bmopf_open_delta") {
        let leg = meta["leg"]
            .as_u64()
            .ok_or_else(|| invalid(&t.name, "missing regulator leg"))? as usize;
        imax.map(|v| {
            v.get(leg)
                .copied()
                .map(|x| vec![x])
                .ok_or_else(|| invalid(&t.name, "regulator current count"))
        })
        .transpose()?
    } else {
        imax
    };
    let mut neutral_cap = None;
    let imax = if subtype == "center_tap" {
        imax.map(|v| {
            if v.iter().any(|x| x.is_nan() || *x < 0.0) {
                return Err(invalid(&t.name, "invalid center-tap current rating"));
            }
            if j == 0 {
                if v.len() != 1 && v.len() != 2 {
                    return Err(invalid(
                        &t.name,
                        "center-tap primary rating needs one or two currents",
                    ));
                }
                Ok(vec![v.iter().copied().fold(f64::INFINITY, f64::min)])
            } else {
                if v.len() != 3 {
                    return Err(invalid(
                        &t.name,
                        "center-tap secondary requires three terminal ratings",
                    ));
                }
                if j == 1 {
                    neutral_cap = caps(&t.name, Some(&vec![v[1]]), 1, ib)?[0];
                }
                Ok(vec![v[if j == 1 { 0 } else { 2 }]])
            }
        })
        .transpose()?
    } else if matches!(subtype, "wye_delta" | "delta_wye") && w.conn == DistWindingConn::Wye {
        imax.map(|v| {
            if v.len() == phases {
                return Ok(v);
            }
            let ids = ctx.resolve(&t.name, &w.bus, &w.terminal_map)?;
            if v.len() != ids.len() || ids.len() != phases + 1 {
                return Err(invalid(&t.name, "wye terminal rating shape"));
            }
            let neutral = map[0]
                .1
                .ok_or_else(|| invalid(&t.name, "missing rated neutral"))?;
            let n = ids
                .iter()
                .position(|id| *id == neutral)
                .ok_or_else(|| invalid(&t.name, "missing rated neutral"))?;
            neutral_cap = caps(&t.name, Some(&vec![v[n]]), 1, ib)?[0];
            map.iter()
                .map(|(pos, _)| {
                    ids.iter()
                        .position(|id| id == pos)
                        .map(|k| v[k])
                        .ok_or_else(|| invalid(&t.name, "missing rated phase"))
                })
                .collect()
        })
        .transpose()?
    } else {
        imax
    };
    Ok((imax, neutral_cap))
}

fn tap_equations(
    profile: &TransformerProfile<'_>,
    noms: &[f64],
    tx: &mut McOpfTransformer,
) -> Result<()> {
    let TransformerProfile {
        t, regulator, zb, ..
    } = *profile;
    if regulator {
        let ratio = noms[0] / noms[1];
        let mut balance = McOpfComplexRow::default();
        balance.current.push((1, [1.0, 0.0]));
        let mut dynamic_balance = McOpfComplexRow::default();
        dynamic_balance.current.push((0, [ratio, 0.0]));
        let mut drop = McOpfComplexRow::default();
        add_voltage(&mut drop, &tx.coils[0], 1.0);
        drop.current.push((
            0,
            [
                -extra_number(t, "r_series_from", 0.0)? / zb,
                -extra_number(t, "x_series_from", 0.0)? / zb,
            ],
        ));
        let mut dynamic_drop = McOpfComplexRow::default();
        add_voltage(&mut dynamic_drop, &tx.coils[1], -ratio);
        dynamic_drop.current.push((
            1,
            [
                ratio * extra_number(t, "r_series_to", 0.0)? / zb,
                ratio * extra_number(t, "x_series_to", 0.0)? / zb,
            ],
        ));
        tx.equations = vec![balance, drop];
        tx.tap_equations = vec![dynamic_balance, dynamic_drop];
    } else {
        for row in &mut tx.equations {
            let mut dynamic = McOpfComplexRow::default();
            if row.voltage.is_empty() {
                dynamic.current.push(row.current.remove(0));
            } else {
                let count = 1 + usize::from(tx.coils[0].negative.is_some());
                dynamic.voltage = row.voltage.split_off(count);
                dynamic.current = std::mem::take(&mut row.current);
            }
            tx.tap_equations.push(dynamic);
        }
    }
    Ok(())
}

fn validate_extras(t: &powerio_dist::DistTransformer) -> Result<()> {
    extras(
        &t.name,
        &t.extras,
        &[
            "tap_min",
            "tap_max",
            "tap_ratio_min",
            "tap_ratio_max",
            "bmopf_open_delta",
            "bmopf_subtype",
            "bmopf_power_base",
            "bmopf_winding_metadata",
            "bmopf_delta_rolls",
            "no_load_shunt",
            "g_no_load",
            "b_no_load",
            "i_max_from",
            "i_max_to",
            "regulator_type",
            "r_series_from",
            "r_series_to",
            "x_series_from",
            "x_series_to",
        ],
    )?;
    Ok(())
}

// Canonical two-terminal delta nameplates use a single-coil base even though
// BMOPFTools n-winding incidence retains both directed delta currents.
fn winding_base_phases(w: &powerio_dist::DistWinding, coils: usize) -> f64 {
    if w.conn == DistWindingConn::Delta && w.terminal_map.len() == 2 {
        1.0
    } else {
        coils as f64
    }
}
