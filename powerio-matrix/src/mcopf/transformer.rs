//! Winding-current descriptor; no inverse of a leakage impedance is formed.
use super::{
    Context, Deserialize, McAcOpfAssemblyOptions, McOpfBranch, McOpfShunt, McOpfTerminal, Result,
    Serialize, caps, extras, finite, invalid, unsupported,
};
use num_complex::Complex64;
use powerio_dist::DistWindingConn;
use powerio_dist::transformer::{
    TransformerLeakageBase, TransformerMagnetizing, WindingNeutralGrounding,
    calc_coil_shunt_primitive, calc_coil_voltage, calc_reduced_leakage, calc_regulator_series_ohms,
    calc_transformer_leakage_impedances, calc_transformer_magnetizing, calc_winding_connection,
    calc_winding_neutral_grounding, transformer_extra_number,
};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[non_exhaustive]
/// Complex affine expression with no constant term. Coefficients are `[real, imaginary]`.
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
    /// True for regulators, of either ANSI type, whose tap is the regulated
    /// over source voltage ratio.
    pub tap_inverse: bool,
    /// Rows multiplied by the tap multiplier, aligned with equations; empty for fixed taps.
    pub tap_equations: Vec<McOpfComplexRow>,
}
impl McOpfTransformer {
    /// The physical tap at a tap multiplier `t`: `tap_nominal * t`, or
    /// `tap_nominal / t` for a regulator (`tap_inverse`). The one tap
    /// convention, with its inverse [`Self::calc_tap_multiplier`] and the
    /// derivatives below.
    #[must_use]
    pub fn calc_physical_tap(&self, multiplier: f64) -> f64 {
        if self.tap_inverse {
            self.tap_nominal / multiplier
        } else {
            self.tap_nominal * multiplier
        }
    }

    /// The tap multiplier of a physical tap: `physical / tap_nominal`, or
    /// `tap_nominal / physical` for a regulator. The preparation maps its
    /// physical tap bounds through it.
    #[must_use]
    pub fn calc_tap_multiplier(&self, physical: f64) -> f64 {
        if self.tap_inverse {
            self.tap_nominal / physical
        } else {
            physical / self.tap_nominal
        }
    }

    /// `d physical / d t` at the multiplier `t`: `tap_nominal`, or
    /// `-tap_nominal / t²` for a regulator.
    #[must_use]
    pub fn calc_physical_tap_derivative(&self, multiplier: f64) -> f64 {
        if self.tap_inverse {
            -self.tap_nominal / multiplier.powi(2)
        } else {
            self.tap_nominal
        }
    }

    /// `d t / d physical` at a physical tap: `1 / tap_nominal`, or
    /// `-tap_nominal / physical²` for a regulator.
    #[must_use]
    pub fn calc_tap_multiplier_derivative(&self, physical: f64) -> f64 {
        if self.tap_inverse {
            -self.tap_nominal / physical.powi(2)
        } else {
            self.tap_nominal.recip()
        }
    }
}
fn add_voltage(row: &mut McOpfComplexRow, coil: &McOpfTransformerCoil, value: f64) {
    row.voltage.push((coil.positive, [value, 0.0]));
    if let Some(n) = coil.negative {
        row.voltage.push((n, [-value, 0.0]));
    }
}
fn extra_number(t: &powerio_dist::DistTransformer, key: &str, default: f64) -> Result<f64> {
    let v = transformer_extra_number(&t.extras, key).unwrap_or(default);
    if !v.is_finite() {
        return Err(invalid(&t.name, format!("invalid {key}")));
    }
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
/// A per unit admittance `g + j b` across one coil, as a local shunt
/// ([`calc_coil_shunt_primitive`]).
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
    let matrix = calc_coil_shunt_primitive(Complex64::new(g, b), coil.negative.is_none());
    let terminals = std::iter::once(coil.positive)
        .chain(coil.negative)
        .collect();
    shunts.push(McOpfShunt {
        identity: name,
        terminals,
        g: matrix
            .iter()
            .map(|row| row.iter().map(|y| y.re).collect())
            .collect(),
        b: matrix
            .iter()
            .map(|row| row.iter().map(|y| y.im).collect())
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
    /// The magnetizing branch the unit states, siemens per coil.
    core: Option<TransformerMagnetizing>,
}
type TerminalRatings = (Option<Vec<f64>>, Option<f64>);
type WindingGeometry = (Vec<super::CoilPairs>, Vec<f64>);

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
        let (maps, mut noms) = winding_geometry(ctx, t, subtype)?;
        let phases = maps[0].len();
        if maps.iter().any(|p| p.len() != phases) {
            return Err(invalid(&t.name, "all windings must have equal coil count"));
        }
        if regulator {
            prepare_regulator(ctx, t, &maps, &mut noms, branches, terminals)?;
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
            core: calc_transformer_magnetizing(t).map_err(|error| invalid(&t.name, error))?,
        };
        let mut tx = leakage_descriptor(&profile, &maps, &noms)?;
        prepare_ports(&profile, ctx, &maps, &mut tx, shunts, terminals)?;
        for row in &tx.equations {
            for (_, c) in row.voltage.iter().chain(&row.current) {
                finite(&t.name, c)?;
            }
        }
        core_shunts(&profile, &tx, shunts);
        prepare_tap(&profile, &noms, &mut tx)?;
        out.push(tx);
    }
    Ok(out)
}
/// The magnetizing branch ([`calc_transformer_magnetizing`]) as one per
/// unit shunt across each coil of its winding.
fn core_shunts(
    profile: &TransformerProfile<'_>,
    tx: &McOpfTransformer,
    shunts: &mut Vec<McOpfShunt>,
) {
    let TransformerProfile {
        t,
        phases,
        zb,
        core,
        ..
    } = *profile;
    if let Some(core) = core {
        for k in 0..phases {
            stamp_shunt(
                shunts,
                format!("transformer:{}:core:{k}", t.name),
                &tx.coils[core.winding * phases + k],
                core.admittance.re * zb,
                core.admittance.im * zb,
            );
        }
    }
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
        // A regulator's `tap_ratio` is the regulated over source voltage ratio
        // for both ANSI types; `regulator_type` records the hardware only.
        tx.tap_inverse = regulator;
        let [a, b] = [tx.calc_tap_multiplier(lo), tx.calc_tap_multiplier(hi)];
        tx.tap_bounds = Some([a.min(b), a.max(b)]);
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
    terminals: &[McOpfTerminal],
) -> Result<()> {
    let TransformerProfile {
        t,
        subtype,
        nw,
        phases,
        zb,
        ib,
        sb,
        core,
        ..
    } = *profile;
    // A rated port on the magnetized winding carries the per unit core
    // admittance of its coil.
    let core_on = |winding: usize| -> [f64; 2] {
        core.filter(|core| core.winding == winding)
            .map_or([0.0, 0.0], |core| {
                [core.admittance.re * zb, core.admittance.im * zb]
            })
    };
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
            neutral_port(profile, tx, &maps[j], j, core_on(1), cap)?;
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
        let magnetized = usize::from(subtype != "single_phase_autotransformer");
        phase_ports(profile, tx, j, &imax, &smax, core_on(magnetized));
        grounding(profile, &maps[j], j, shunts, terminals)?;
    }
    Ok(())
}

/// Each winding's coils over global terminals, from the one winding
/// connection ([`calc_winding_connection`], which orients DELTA rings), and
/// its coil turns ([`calc_coil_voltage`] times tap).
#[allow(clippy::float_cmp)] // Tap=1 is an exact schema sentinel, not a numerical tolerance.
fn winding_geometry(
    ctx: &Context<'_>,
    t: &powerio_dist::DistTransformer,
    subtype: &str,
) -> Result<WindingGeometry> {
    let mut maps = Vec::new();
    let mut noms = Vec::new();
    for (j, w) in t.windings.iter().enumerate() {
        finite(&t.name, &[w.v_ref, w.s_rating, w.r_pct, w.tap])?;
        if w.v_ref <= 0.0 || w.s_rating <= 0.0 || w.tap <= 0.0 || w.r_pct < 0.0 {
            return Err(invalid(&t.name, "invalid winding nameplate/tap"));
        }
        if subtype == "n_winding" && w.tap != 1.0 {
            return Err(unsupported(
                &t.name,
                "n-winding tap fields are not supported by BMOPFTools",
            ));
        }
        let ids = ctx.resolve(&t.name, &w.bus, &w.terminal_map)?;
        if w.conn == DistWindingConn::Delta {
            ctx.require_phase_terminals(&t.name, &w.bus, &w.terminal_map)?;
        }
        let pairs: super::CoilPairs = calc_winding_connection(ctx.net, t, j)
            .map_err(|error| invalid(&t.name, error))?
            .iter()
            .map(|coil| (ids[coil.from], coil.to.map(|to| ids[to])))
            .collect();
        if pairs.is_empty() {
            return Err(invalid(&t.name, "no coils"));
        }
        noms.push(calc_coil_voltage(t, j) * w.tap);
        maps.push(pairs);
    }
    Ok((maps, noms))
}

fn prepare_regulator(
    ctx: &Context<'_>,
    t: &powerio_dist::DistTransformer,
    maps: &[super::CoilPairs],
    noms: &mut Vec<f64>,
    branches: &mut Vec<McOpfBranch>,
    terminals: &[McOpfTerminal],
) -> Result<()> {
    let m = t.windings.len();
    let phases = maps[0].len();
    if m != 2 || phases != 1 {
        return Err(invalid(&t.name, "regulator must have two single coils"));
    }
    let tap = t.windings[1].tap;
    // `tap_ratio` is V_to/V_from for both ANSI types (BMOPF);
    // `regulator_type` records the A/B hardware and does not change it.
    if !matches!(
        t.extras
            .get("regulator_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("B"),
        "A" | "B"
    ) {
        return Err(invalid(&t.name, "unknown regulator type"));
    }
    let ratio = 1.0 / tap;
    if ratio <= 0.0 {
        return Err(invalid(&t.name, "nonpositive effective regulator ratio"));
    }
    *noms = vec![ratio, 1.0];
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
    // The first winding's coil impedance base: its coil voltage squared over
    // its per phase power base.
    let ref_base = calc_coil_voltage(t, 0).powi(2) * winding_base_phases(&t.windings[0], phases)
        / extra_number(t, "bmopf_power_base", t.windings[0].s_rating)?;
    if ref_base <= 0.0 {
        return Err(invalid(&t.name, "nonpositive transformer power base"));
    }
    // The pairwise leakage is the one derivation in
    // `calc_transformer_leakage_impedances`, referred to the first winding on
    // the base the unit states it in: OpenDSS nameplate percentages, an
    // n-winding BMOPF unit's phase bases, or a regulator's stated ohms. A
    // per unit result scales to the first winding's ohmic base over the bus
    // base; stated ohms divide by the bus base.
    let base = if regulator {
        TransformerLeakageBase::StatedOhms
    } else if nw {
        TransformerLeakageBase::WindingPhase {
            power_base: extra_number(t, "bmopf_power_base", t.windings[0].s_rating)?,
            phases: t
                .windings
                .iter()
                .zip(maps)
                .map(|(w, map)| winding_base_phases(w, map.len()))
                .collect(),
        }
    } else {
        TransformerLeakageBase::Nameplate
    };
    let leakage =
        calc_transformer_leakage_impedances(t, &base).map_err(|error| invalid(&t.name, error))?;
    let scale = ref_base * t.windings[0].tap.powi(2) / zb;
    let per_unit: Vec<Vec<Complex64>> = leakage
        .iter()
        .map(|row| {
            row.iter()
                .map(|z| {
                    if regulator {
                        Complex64::new(z.re / zb, z.im / zb)
                    } else {
                        Complex64::new(z.re * scale, z.im * scale)
                    }
                })
                .collect()
        })
        .collect();
    // The winding difference impedances against the first winding, kept in
    // impedance form: row `a - 1` is winding `a` less winding one.
    let reduced = calc_reduced_leakage(&per_unit, 0);
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
                let z = reduced[a - 1][b - 1];
                row.current
                    .push((b * phases + k, [z.re * ratios[b], z.im * ratios[b]]));
            }
            tx.equations.push(row);
        }
    }
    Ok(tx)
}

/// A winding's neutral grounding ([`calc_winding_neutral_grounding`]) at the
/// neutral its coils return through: a finite impedance is a per unit shunt
/// to ground, a solid grounding must land on a terminal the network already
/// grounds (the descriptor states no extra exact zero voltage), and an
/// unstated or floating neutral adds nothing.
fn grounding(
    profile: &TransformerProfile<'_>,
    map: &super::CoilPairs,
    j: usize,
    shunts: &mut Vec<McOpfShunt>,
    terminals: &[McOpfTerminal],
) -> Result<()> {
    let TransformerProfile { t, zb, .. } = *profile;
    let grounding =
        calc_winding_neutral_grounding(t, j).map_err(|error| unsupported(&t.name, error))?;
    let neutral = || {
        map[0]
            .1
            .ok_or_else(|| invalid(&t.name, "neutral impedance without neutral"))
    };
    match grounding {
        WindingNeutralGrounding::Impedance(y) => stamp_shunt(
            shunts,
            format!("transformer:{}:neutral:{j}", t.name),
            &McOpfTransformerCoil {
                positive: neutral()?,
                negative: None,
            },
            y.re * zb,
            y.im * zb,
        ),
        WindingNeutralGrounding::Solid if !terminals[neutral()?].grounded => {
            return Err(unsupported(
                &t.name,
                "a solidly grounded winding neutral on a terminal the network does not ground needs an exact zero voltage",
            ));
        }
        _ => {}
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
        let [r_from, x_from, r_to, x_to] =
            calc_regulator_series_ohms(t).map_err(|error| invalid(&t.name, error))?;
        let ratio = noms[0] / noms[1];
        let mut balance = McOpfComplexRow::default();
        balance.current.push((1, [1.0, 0.0]));
        let mut dynamic_balance = McOpfComplexRow::default();
        dynamic_balance.current.push((0, [ratio, 0.0]));
        let mut drop = McOpfComplexRow::default();
        add_voltage(&mut drop, &tx.coils[0], 1.0);
        drop.current.push((0, [-r_from / zb, -x_from / zb]));
        let mut dynamic_drop = McOpfComplexRow::default();
        add_voltage(&mut dynamic_drop, &tx.coils[1], -ratio);
        dynamic_drop
            .current
            .push((1, [ratio * r_to / zb, ratio * x_to / zb]));
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

/// Transformer extras the preparation interprets.
pub(super) const INTERPRETED_EXTRAS: &[&str] = &[
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
    "%noloadloss",
    "%imag",
    "leadlag",
    "i_max_from",
    "i_max_to",
    "regulator_type",
    "r_series_from",
    "r_series_to",
    "x_series_from",
    "x_series_to",
];

fn validate_extras(t: &powerio_dist::DistTransformer) -> Result<()> {
    extras(&t.name, &t.extras, INTERPRETED_EXTRAS)
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
