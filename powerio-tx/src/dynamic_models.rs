//! Typed views over PSS/E DYR records, and model-aware linking of DYR records
//! to the generators of a [`crate::BalancedNetwork`].
//!
//! [`crate::dynamic`] keeps every DYR record loss-preserving and untyped. This
//! module adds a schema-backed *view* on top of it for the models whose
//! parameter layout is documented in the PSS/E Model Library: it never changes
//! the retained records, so `DynamicCase::to_dyr_string` still writes the
//! source back byte for byte.
//!
//! Parameters keep PSS/E names, order, units and bases (machine parameters
//! are on the machine base `MBASE`). Converting them into the parameters of a
//! particular simulator's equations is the simulator's job.
//!
//! Supported models: machines `GENCLS`, `GENROU`, `GENROE`, `GENSAL`,
//! `GENSAE`; exciters `SEXS`, `ESAC1A`, `EXST1`; governors `TGOV1`, `GAST`, `HYGOV`;
//! stabilizer `IEEEST`. Any other model name is reported as
//! [`ModelError::Unsupported`] and stays available as a raw record.

use std::collections::BTreeMap;
use std::fmt;

use crate::BalancedNetwork;
use crate::dynamic::{DynamicCase, DynamicRecord};
use crate::format::psse::detailed_source_property;
use crate::network::BusId;

/// Round-rotor machine data shared by `GENROU` (quadratic saturation) and
/// `GENROE` (exponential saturation), in DYR order.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct RoundRotor {
    pub td0_p: f64,
    pub td0_pp: f64,
    pub tq0_p: f64,
    pub tq0_pp: f64,
    pub h: f64,
    pub d: f64,
    pub xd: f64,
    pub xq: f64,
    pub xd_p: f64,
    pub xq_p: f64,
    /// `X''d = X''q`.
    pub xd_pp: f64,
    pub xl: f64,
    /// Saturation at 1.0 pu flux, `S(1.0)`.
    pub s1: f64,
    /// Saturation at 1.2 pu flux, `S(1.2)`.
    pub s12: f64,
}

/// Salient-pole machine data shared by `GENSAL` and `GENSAE`, in DYR order.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SalientPole {
    pub td0_p: f64,
    pub td0_pp: f64,
    pub tq0_pp: f64,
    pub h: f64,
    pub d: f64,
    pub xd: f64,
    pub xq: f64,
    pub xd_p: f64,
    /// `X''d = X''q`.
    pub xd_pp: f64,
    pub xl: f64,
    pub s1: f64,
    pub s12: f64,
}

/// Classical machine `GENCLS`: inertia and damping. The transient reactance
/// is the generator's `ZSORCE` in the static case.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Gencls {
    pub h: f64,
    pub d: f64,
}

/// Simplified excitation system `SEXS`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Sexs {
    /// `TA/TB`.
    pub ta_tb: f64,
    pub tb: f64,
    pub k: f64,
    pub te: f64,
    pub emin: f64,
    pub emax: f64,
}

/// IEEE 421.5 AC1A excitation system `ESAC1A`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Esac1a {
    pub tr: f64,
    pub tb: f64,
    pub tc: f64,
    pub ka: f64,
    pub ta: f64,
    pub vamax: f64,
    pub vamin: f64,
    pub te: f64,
    pub kf: f64,
    pub tf: f64,
    pub kc: f64,
    pub kd: f64,
    pub ke: f64,
    pub e1: f64,
    pub se_e1: f64,
    pub e2: f64,
    pub se_e2: f64,
    pub vrmax: f64,
    pub vrmin: f64,
}

/// IEEE type ST1 static excitation system `EXST1`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Exst1 {
    pub tr: f64,
    pub vimax: f64,
    pub vimin: f64,
    pub tc: f64,
    pub tb: f64,
    pub ka: f64,
    pub ta: f64,
    pub vrmax: f64,
    pub vrmin: f64,
    pub kc: f64,
    pub kf: f64,
    pub tf: f64,
}

/// Steam turbine-governor `TGOV1`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Tgov1 {
    pub r: f64,
    pub t1: f64,
    pub vmax: f64,
    pub vmin: f64,
    pub t2: f64,
    pub t3: f64,
    pub dt: f64,
}

/// Gas turbine-governor `GAST`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Gast {
    pub r: f64,
    pub t1: f64,
    pub t2: f64,
    pub t3: f64,
    pub at: f64,
    pub kt: f64,
    pub vmax: f64,
    pub vmin: f64,
    pub dturb: f64,
}

/// Hydro turbine-governor `HYGOV`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Hygov {
    pub r: f64,
    pub r_temp: f64,
    pub tr: f64,
    pub tf: f64,
    pub tg: f64,
    pub velm: f64,
    pub gmax: f64,
    pub gmin: f64,
    pub tw: f64,
    pub at: f64,
    pub dturb: f64,
    pub qnl: f64,
}

/// IEEE stabilizer `IEEEST`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Ieeest {
    /// `ICS`, in `1..=6`: 1 rotor speed deviation, 2 bus frequency
    /// deviation, 3 generator electrical power, 4 accelerating power, 5 bus
    /// voltage, 6 bus voltage derivative.
    pub input_code: u8,
    /// `IB`: remote bus for the input signal; `None` when the record states 0,
    /// the generator's own bus.
    pub remote_bus: Option<BusId>,
    pub a1: f64,
    pub a2: f64,
    pub a3: f64,
    pub a4: f64,
    pub a5: f64,
    pub a6: f64,
    pub t1: f64,
    pub t2: f64,
    pub t3: f64,
    pub t4: f64,
    pub t5: f64,
    pub t6: f64,
    pub ks: f64,
    pub lsmax: f64,
    pub lsmin: f64,
    pub vcu: f64,
    pub vcl: f64,
}

/// A DYR record read through its PSS/E parameter schema.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DynamicModel {
    Gencls(Gencls),
    Genrou(RoundRotor),
    Genroe(RoundRotor),
    Gensal(SalientPole),
    Gensae(SalientPole),
    Sexs(Sexs),
    Esac1a(Esac1a),
    Exst1(Exst1),
    Tgov1(Tgov1),
    Gast(Gast),
    Hygov(Hygov),
    Ieeest(Ieeest),
}

/// The role a model plays in a generating unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ModelRole {
    Machine,
    Exciter,
    Governor,
    Stabilizer,
}

impl DynamicModel {
    /// The PSS/E model name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Gencls(_) => "GENCLS",
            Self::Genrou(_) => "GENROU",
            Self::Genroe(_) => "GENROE",
            Self::Gensal(_) => "GENSAL",
            Self::Gensae(_) => "GENSAE",
            Self::Sexs(_) => "SEXS",
            Self::Esac1a(_) => "ESAC1A",
            Self::Exst1(_) => "EXST1",
            Self::Tgov1(_) => "TGOV1",
            Self::Gast(_) => "GAST",
            Self::Hygov(_) => "HYGOV",
            Self::Ieeest(_) => "IEEEST",
        }
    }

    /// Machine, exciter, governor or stabilizer.
    #[must_use]
    pub fn role(&self) -> ModelRole {
        match self {
            Self::Gencls(_)
            | Self::Genrou(_)
            | Self::Genroe(_)
            | Self::Gensal(_)
            | Self::Gensae(_) => ModelRole::Machine,
            Self::Sexs(_) | Self::Esac1a(_) | Self::Exst1(_) => ModelRole::Exciter,
            Self::Tgov1(_) | Self::Gast(_) | Self::Hygov(_) => ModelRole::Governor,
            Self::Ieeest(_) => ModelRole::Stabilizer,
        }
    }
}

/// The model names this module reads, with the parameter count each record
/// states after its device id.
pub const SUPPORTED_MODELS: &[(&str, usize)] = &[
    ("GENCLS", 2),
    ("GENROU", 14),
    ("GENROE", 14),
    ("GENSAL", 12),
    ("GENSAE", 12),
    ("SEXS", 6),
    ("ESAC1A", 19),
    ("EXST1", 12),
    ("TGOV1", 7),
    ("GAST", 9),
    ("HYGOV", 12),
    ("IEEEST", 19),
];

/// Why a record has no typed view.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelError {
    /// The model is not in [`SUPPORTED_MODELS`]; the record stays raw.
    Unsupported { model_name: String },
    /// The record states another number of parameters than the schema.
    Arity {
        model_name: String,
        expected: usize,
        found: usize,
    },
    /// A parameter token is not a finite number.
    NotANumber {
        model_name: String,
        index: usize,
        token: String,
    },
    /// An integer parameter (`ICON`) is not an integer, or is outside the
    /// range the model accepts: `IEEEST` `ICS` must be in `1..=6`, and `IB`
    /// a bus number from 0 to `u32::MAX`.
    InvalidIcon {
        model_name: String,
        index: usize,
        token: String,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { model_name } => write!(f, "model {model_name} has no typed view"),
            Self::Arity {
                model_name,
                expected,
                found,
            } => write!(
                f,
                "{model_name} states {found} parameters, the PSS/E schema has {expected}"
            ),
            Self::NotANumber {
                model_name,
                index,
                token,
            } => write!(
                f,
                "{model_name} parameter {} ({token:?}) is not a finite number",
                index + 1
            ),
            Self::InvalidIcon {
                model_name,
                index,
                token,
            } => write!(
                f,
                "{model_name} parameter {} ({token:?}) is not a valid integer code",
                index + 1
            ),
        }
    }
}

impl std::error::Error for ModelError {}

fn numbers(record: &DynamicRecord, expected: usize) -> Result<Vec<f64>, ModelError> {
    let name = record.model_name().to_ascii_uppercase();
    let params = record.parameters();
    if params.len() != expected {
        return Err(ModelError::Arity {
            model_name: name,
            expected,
            found: params.len(),
        });
    }
    params
        .iter()
        .enumerate()
        .map(|(index, token)| {
            // Fortran list-directed output may write `1.0D0` for `1.0E0`.
            token
                .trim()
                .replacen(['D', 'd'], "E", 1)
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| ModelError::NotANumber {
                    model_name: name.clone(),
                    index,
                    token: token.clone(),
                })
        })
        .collect()
}

/// An integer parameter of an already finite value: the value must be exactly
/// integral (`1` or `1.0`) and inside `range`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn icon(
    record: &DynamicRecord,
    p: &[f64],
    index: usize,
    range: std::ops::RangeInclusive<u32>,
) -> Result<u32, ModelError> {
    let value = p[index];
    let in_range = value.fract() == 0.0
        && value >= f64::from(*range.start())
        && value <= f64::from(*range.end());
    if in_range {
        // Exact: an integral value inside a `u32` range.
        Ok(value as u32)
    } else {
        Err(ModelError::InvalidIcon {
            model_name: record.model_name().trim().to_ascii_uppercase(),
            index,
            token: record.parameters()[index].clone(),
        })
    }
}

fn round_rotor(p: &[f64]) -> RoundRotor {
    RoundRotor {
        td0_p: p[0],
        td0_pp: p[1],
        tq0_p: p[2],
        tq0_pp: p[3],
        h: p[4],
        d: p[5],
        xd: p[6],
        xq: p[7],
        xd_p: p[8],
        xq_p: p[9],
        xd_pp: p[10],
        xl: p[11],
        s1: p[12],
        s12: p[13],
    }
}

fn salient_pole(p: &[f64]) -> SalientPole {
    SalientPole {
        td0_p: p[0],
        td0_pp: p[1],
        tq0_pp: p[2],
        h: p[3],
        d: p[4],
        xd: p[5],
        xq: p[6],
        xd_p: p[7],
        xd_pp: p[8],
        xl: p[9],
        s1: p[10],
        s12: p[11],
    }
}

/// Read a DYR record through its PSS/E parameter schema.
///
/// # Errors
/// [`ModelError::Unsupported`] for a model outside [`SUPPORTED_MODELS`],
/// [`ModelError::Arity`] or [`ModelError::NotANumber`] when the record does
/// not match the schema, [`ModelError::InvalidIcon`] for an integer parameter
/// that is not an integer in range. Other parameter values pass through
/// unchecked: several zeros carry PSS/E meaning (a zero time constant
/// bypasses its block, `GENCLS` `H = 0` is an infinite bus).
#[allow(clippy::cast_possible_truncation, clippy::too_many_lines)] // one arm per schema
pub fn typed_model(record: &DynamicRecord) -> Result<DynamicModel, ModelError> {
    let name = record.model_name().trim().to_ascii_uppercase();
    let Some(&(_, arity)) = SUPPORTED_MODELS.iter().find(|(n, _)| *n == name) else {
        return Err(ModelError::Unsupported { model_name: name });
    };
    let p = numbers(record, arity)?;
    Ok(match name.as_str() {
        "GENCLS" => DynamicModel::Gencls(Gencls { h: p[0], d: p[1] }),
        "GENROU" => DynamicModel::Genrou(round_rotor(&p)),
        "GENROE" => DynamicModel::Genroe(round_rotor(&p)),
        "GENSAL" => DynamicModel::Gensal(salient_pole(&p)),
        "GENSAE" => DynamicModel::Gensae(salient_pole(&p)),
        "SEXS" => DynamicModel::Sexs(Sexs {
            ta_tb: p[0],
            tb: p[1],
            k: p[2],
            te: p[3],
            emin: p[4],
            emax: p[5],
        }),
        "ESAC1A" => DynamicModel::Esac1a(Esac1a {
            tr: p[0],
            tb: p[1],
            tc: p[2],
            ka: p[3],
            ta: p[4],
            vamax: p[5],
            vamin: p[6],
            te: p[7],
            kf: p[8],
            tf: p[9],
            kc: p[10],
            kd: p[11],
            ke: p[12],
            e1: p[13],
            se_e1: p[14],
            e2: p[15],
            se_e2: p[16],
            vrmax: p[17],
            vrmin: p[18],
        }),
        "EXST1" => DynamicModel::Exst1(Exst1 {
            tr: p[0],
            vimax: p[1],
            vimin: p[2],
            tc: p[3],
            tb: p[4],
            ka: p[5],
            ta: p[6],
            vrmax: p[7],
            vrmin: p[8],
            kc: p[9],
            kf: p[10],
            tf: p[11],
        }),
        "TGOV1" => DynamicModel::Tgov1(Tgov1 {
            r: p[0],
            t1: p[1],
            vmax: p[2],
            vmin: p[3],
            t2: p[4],
            t3: p[5],
            dt: p[6],
        }),
        "GAST" => DynamicModel::Gast(Gast {
            r: p[0],
            t1: p[1],
            t2: p[2],
            t3: p[3],
            at: p[4],
            kt: p[5],
            vmax: p[6],
            vmin: p[7],
            dturb: p[8],
        }),
        "HYGOV" => DynamicModel::Hygov(Hygov {
            r: p[0],
            r_temp: p[1],
            tr: p[2],
            tf: p[3],
            tg: p[4],
            velm: p[5],
            gmax: p[6],
            gmin: p[7],
            tw: p[8],
            at: p[9],
            dturb: p[10],
            qnl: p[11],
        }),
        "IEEEST" => DynamicModel::Ieeest(Ieeest {
            // Exact: ICS is in 1..=6.
            input_code: icon(record, &p, 0, 1..=6)? as u8,
            remote_bus: Some(icon(record, &p, 1, 0..=u32::MAX)?)
                .filter(|&bus| bus != 0)
                .map(|bus| BusId(bus as usize)),
            a1: p[2],
            a2: p[3],
            a3: p[4],
            a4: p[5],
            a5: p[6],
            a6: p[7],
            t1: p[8],
            t2: p[9],
            t3: p[10],
            t4: p[11],
            t5: p[12],
            t6: p[13],
            ks: p[14],
            lsmax: p[15],
            lsmin: p[16],
            vcu: p[17],
            vcl: p[18],
        }),
        _ => unreachable!("every supported model has a schema arm"),
    })
}

/// The PSS/E machine id of every generator, aligned with
/// [`BalancedNetwork::generators`], trimmed: the id the PSS/E writer states for
/// it and the `.con` resolver matches (the retained id, else the lowest
/// positional id still free on the bus). DYR records name generators by
/// `(bus, machine id)`.
#[must_use]
pub fn psse_machine_ids(net: &BalancedNetwork) -> Vec<String> {
    let (ids, _) = crate::contingency::machine_index(net, &mut 0);
    ids.into_iter().map(|id| id.trim().to_owned()).collect()
}

/// The generator's source impedance `ZSORCE = ZR + jZX` on its machine base,
/// as the static case states it (PSS/E defaults `0 + j1` when absent); `None`
/// when the network has no such generator.
#[must_use]
pub fn generator_source_impedance(net: &BalancedNetwork, generator: usize) -> Option<(f64, f64)> {
    let g = net.generators().get(generator)?;
    let read = |key: &str, default: f64| {
        detailed_source_property(net, "generator", g.uid.as_deref(), key)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default)
    };
    Some((read("psse_zr", 0.0), read("psse_zx", 1.0)))
}

/// One typed model and the DYR line it came from.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinkedModel {
    pub model: DynamicModel,
    pub line: usize,
}

/// The dynamic models attached to one generator of the static case.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MachineDynamics {
    /// Index into [`BalancedNetwork::generators`].
    pub generator: usize,
    pub bus: BusId,
    /// The generator's PSS/E machine id, trimmed (see [`psse_machine_ids`]).
    pub machine_id: String,
    /// Machine base in MVA.
    pub mbase: f64,
    /// `ZSORCE` on the machine base.
    pub zsource: (f64, f64),
    pub machine: Option<LinkedModel>,
    pub exciter: Option<LinkedModel>,
    pub governor: Option<LinkedModel>,
    pub stabilizer: Option<LinkedModel>,
}

/// Model names without a typed view that PSS/E attaches to a generator by
/// `(bus, machine id)`: machines, exciters, governors, stabilizers, limiters,
/// compensators and the renewable generator, electrical and plant controls.
/// A record of one of these names an existing unit even though it is not
/// linked; any other untyped record (load, dc line, relay, user model, ...)
/// may key something else.
#[rustfmt::skip]
const GENERATOR_ATTACHED: &[&str] = &[
    // Machines.
    "GENTPJ", "GENTPF", "GENQEC", "GENDCO", "GENTRA", "CIMTR1", "CIMTR3",
    // Exciters.
    "IEEET1", "IEEET2", "IEEET3", "IEEET4", "IEEET5", "IEEEX1", "IEEEX2", "IEEEX3", "IEEEX4",
    "EXAC1", "EXAC1A", "EXAC2", "EXAC3", "EXAC4", "EXDC2", "EXELI", "EXPIC1", "EXST2",
    "EXST2A", "EXST3", "ESAC2A", "ESAC3A", "ESAC4A", "ESAC5A", "ESAC6A", "ESAC7B", "ESAC8B",
    "ESDC1A", "ESDC2A", "ESDC3A", "ESDC4B", "ESST1A", "ESST2A", "ESST3A", "ESST4B", "ESST5B",
    "ESST6B", "ESST7B", "SCRX", "REXS", "URST5T",
    // Governors.
    "IEEEG1", "IEEEG2", "IEEEG3", "IEESGO", "GGOV1", "TGOV2", "TGOV3", "TGOV5", "HYGOV2",
    "HYGOV4", "WEHGOV", "WSIEG1", "PIDGOV", "GAST2A", "GASTWD", "DEGOV", "DEGOV1", "CRCMGV",
    // Stabilizers.
    "STAB1", "STAB2A", "STAB3", "STAB4", "PSS1A", "PSS2A", "PSS2B", "PSS2C", "PSS3B", "PSS4B",
    "IEE2ST", "ST2CUT",
    // Limiters and compensators.
    "MAXEX1", "MAXEX2", "MNLEX1", "MNLEX2", "MNLEX3", "UEL1", "UEL2", "OEL1B", "COMP", "IEEEVC",
];

/// Name prefixes of generator-attached model families without a typed view:
/// the WECC renewable generator/converter, electrical and plant controllers,
/// and the generic wind models.
const GENERATOR_ATTACHED_PREFIXES: &[&str] = &["REGC", "REEC", "REPC", "WT1", "WT2", "WT3", "WT4"];

/// Whether a record of this model names a generator: every supported model
/// does, and so do the known generator-attached models without a typed view.
fn names_a_generator(error: &ModelError) -> bool {
    match error {
        ModelError::Unsupported { model_name } => {
            GENERATOR_ATTACHED.contains(&model_name.as_str())
                || GENERATOR_ATTACHED_PREFIXES
                    .iter()
                    .any(|prefix| model_name.starts_with(prefix))
        }
        _ => true,
    }
}

/// A DYR record that did not attach to a generator as a typed model, or
/// replaced one that had.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinkIssue {
    /// The record has no typed view; it is kept raw and not linked. `bus` and
    /// `machine_id` are the generator it names when the model is known to
    /// attach to one (a supported model that failed its schema, or a
    /// generator-attached model without a typed view, whose role on that
    /// unit stays empty); `None` for any other record, whose keys may name a
    /// load, a dc line or another device.
    Untyped {
        line: usize,
        bus: Option<BusId>,
        machine_id: Option<String>,
        error: ModelError,
    },
    /// No generator of the static case has this `(bus, id)`.
    NoGenerator {
        line: usize,
        model_name: String,
        bus: BusId,
        machine_id: String,
    },
    /// The generator exists but is out of service; its records are kept and
    /// not linked.
    OutOfService {
        line: usize,
        model_name: String,
        bus: BusId,
        machine_id: String,
    },
    /// No generator on the bus has this id exactly, and several match it
    /// ignoring ASCII case; the record is not linked.
    AmbiguousMachineId {
        line: usize,
        model_name: String,
        bus: BusId,
        machine_id: String,
        /// The ids of the generators that match, in network order.
        candidates: Vec<String>,
    },
    /// A later record of the same role for one generator replaced the
    /// earlier one, as PSS/E does when it reads the DYR file. The record at
    /// `line` is linked; the one at `replaced_line` is not.
    Replaced {
        line: usize,
        replaced_line: usize,
        bus: BusId,
        /// The generator's machine id.
        machine_id: String,
        /// The model of the record at `line`.
        model_name: String,
        role: ModelRole,
    },
}

impl fmt::Display for LinkIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Untyped {
                line,
                bus,
                machine_id,
                error,
            } => match (bus, machine_id) {
                (Some(bus), Some(id)) => {
                    write!(f, "line {line}: {error} (generator ({bus}, '{id}'))")
                }
                _ => write!(f, "line {line}: record not linked: {error}"),
            },
            Self::NoGenerator {
                line,
                model_name,
                bus,
                machine_id,
            } => write!(
                f,
                "line {line}: {model_name} names generator ({bus}, '{machine_id}'), which the static case does not hold"
            ),
            Self::OutOfService {
                line,
                model_name,
                bus,
                machine_id,
            } => write!(
                f,
                "line {line}: {model_name} names generator ({bus}, '{machine_id}'), which is out of service"
            ),
            Self::AmbiguousMachineId {
                line,
                model_name,
                bus,
                machine_id,
                candidates,
            } => write!(
                f,
                "line {line}: {model_name} names generator ({bus}, '{machine_id}'), which matches ids {candidates:?} on that bus only ignoring case"
            ),
            Self::Replaced {
                line,
                replaced_line,
                bus,
                machine_id,
                model_name,
                role,
            } => write!(
                f,
                "line {line}: {model_name} replaces the {role:?} model of line {replaced_line} for generator ({bus}, '{machine_id}')"
            ),
        }
    }
}

/// The result of linking a [`DynamicCase`] to a network's generators.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MachineLinking {
    /// One entry per in-service generator, in network order, with or without
    /// dynamic models.
    pub machines: Vec<MachineDynamics>,
    /// In record order.
    pub issues: Vec<LinkIssue>,
}

/// One generator on a bus: its trimmed machine id, and its slot in
/// [`MachineLinking::machines`] (`None` when out of service).
type BusGenerators = Vec<(String, Option<usize>)>;

/// The generator a record names on its bus: the exact id, else the one
/// generator whose id matches ignoring ASCII case. `Err` holds the candidate
/// ids when several do.
fn find_generator<'a>(
    on_bus: &'a [(String, Option<usize>)],
    machine_id: &str,
) -> Result<Option<&'a (String, Option<usize>)>, Vec<String>> {
    if let Some(exact) = on_bus.iter().find(|(id, _)| id == machine_id) {
        return Ok(Some(exact));
    }
    let folded: Vec<_> = on_bus
        .iter()
        .filter(|(id, _)| id.eq_ignore_ascii_case(machine_id))
        .collect();
    match folded.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        many => Err(many.iter().map(|(id, _)| id.clone()).collect()),
    }
}

/// Attach the typed DYR models to the generators they name by
/// `(bus, machine id)`.
///
/// The machine id matches exactly; when no generator on the bus has it, the
/// one generator whose id matches ignoring ASCII case is used. When several
/// records of one role name the same generator, the last one wins and each
/// replacement is reported as [`LinkIssue::Replaced`], as PSS/E replaces a
/// model of the same category when it reads a DYR file.
#[must_use]
pub fn link_machines(net: &BalancedNetwork, dynamics: &DynamicCase) -> MachineLinking {
    let ids = psse_machine_ids(net);
    let mut machines: Vec<MachineDynamics> = Vec::new();
    let mut by_bus: BTreeMap<BusId, BusGenerators> = BTreeMap::new();
    for (index, (g, id)) in net.generators().iter().zip(&ids).enumerate() {
        let slot = g.in_service.then_some(machines.len());
        by_bus.entry(g.bus).or_default().push((id.clone(), slot));
        if !g.in_service {
            continue;
        }
        machines.push(MachineDynamics {
            generator: index,
            bus: g.bus,
            machine_id: id.clone(),
            mbase: g.mbase,
            zsource: generator_source_impedance(net, index).unwrap_or((0.0, 1.0)),
            machine: None,
            exciter: None,
            governor: None,
            stabilizer: None,
        });
    }
    let mut issues = Vec::new();
    for record in dynamics.records() {
        let line = record.line();
        let machine_id = record.device_id().unwrap_or("1").trim().to_owned();
        let model = match typed_model(record) {
            Ok(model) => model,
            Err(error) => {
                let named = record
                    .bus_id()
                    .filter(|_| names_a_generator(&error))
                    .map(|bus| BusId(bus as usize));
                issues.push(LinkIssue::Untyped {
                    line,
                    bus: named,
                    machine_id: named.map(|_| machine_id),
                    error,
                });
                continue;
            }
        };
        let model_name = model.name().to_owned();
        let bus = BusId(record.bus_id().unwrap_or(0) as usize);
        let on_bus = by_bus.get(&bus).map_or(&[][..], Vec::as_slice);
        let slot = match find_generator(on_bus, &machine_id) {
            Ok(Some((_, Some(slot)))) => *slot,
            Ok(Some((_, None))) => {
                issues.push(LinkIssue::OutOfService {
                    line,
                    model_name,
                    bus,
                    machine_id,
                });
                continue;
            }
            Ok(None) => {
                issues.push(LinkIssue::NoGenerator {
                    line,
                    model_name,
                    bus,
                    machine_id,
                });
                continue;
            }
            Err(candidates) => {
                issues.push(LinkIssue::AmbiguousMachineId {
                    line,
                    model_name,
                    bus,
                    machine_id,
                    candidates,
                });
                continue;
            }
        };
        let entry = &mut machines[slot];
        let role = model.role();
        let target = match role {
            ModelRole::Machine => &mut entry.machine,
            ModelRole::Exciter => &mut entry.exciter,
            ModelRole::Governor => &mut entry.governor,
            ModelRole::Stabilizer => &mut entry.stabilizer,
        };
        if let Some(replaced) = target.replace(LinkedModel { model, line }) {
            issues.push(LinkIssue::Replaced {
                line,
                replaced_line: replaced.line,
                bus: entry.bus,
                machine_id: entry.machine_id.clone(),
                model_name,
                role,
            });
        }
    }
    MachineLinking { machines, issues }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic::parse_dyr;

    const DYR: &str = "    101 'GENCLS' 1     0.0000       0.0000      /
    102 'GENROU' 1     8.0000      0.30000E-01  0.40000      0.50000E-01
          6.1750      0.50000E-01   1.8000       1.7000      0.30000
         0.55000      0.25000      0.20000      0.10000      0.80000      /
    102 'SEXS' 1    0.4    5.0      20.0     1.0     -50.0       50.0 /
    102 'IEEEST' 1  1  0  0  0  0  0  0  0.0  0  0  0  0.02  1.65  1.65  100  0.1  -0.1  0  0 /
    102 'USRMDL' 1 'X' 1 2 /
";

    #[test]
    fn reads_psse_schemas() {
        let case = parse_dyr(DYR).unwrap();
        let models: Vec<_> = case.records().iter().map(typed_model).collect();
        assert_eq!(
            models[0],
            Ok(DynamicModel::Gencls(Gencls { h: 0.0, d: 0.0 }))
        );
        let Ok(DynamicModel::Genrou(g)) = &models[1] else {
            panic!("GENROU not typed: {:?}", models[1]);
        };
        assert!((g.td0_p - 8.0).abs() < 1e-12 && (g.h - 6.175).abs() < 1e-12);
        assert!((g.xl - 0.2).abs() < 1e-12 && (g.s12 - 0.8).abs() < 1e-12);
        let Ok(DynamicModel::Sexs(s)) = &models[2] else {
            panic!("SEXS not typed");
        };
        assert!((s.k - 20.0).abs() < 1e-12 && (s.emax - 50.0).abs() < 1e-12);
        let Ok(DynamicModel::Ieeest(p)) = &models[3] else {
            panic!("IEEEST not typed");
        };
        assert_eq!(p.input_code, 1);
        assert!((p.ks - 100.0).abs() < 1e-12 && (p.lsmin + 0.1).abs() < 1e-12);
        assert!(matches!(models[4], Err(ModelError::Unsupported { .. })));
    }

    #[test]
    fn arity_is_checked() {
        let case = parse_dyr("1 'SEXS' 1 0.1 10 /").unwrap();
        assert_eq!(
            typed_model(&case.records()[0]),
            Err(ModelError::Arity {
                model_name: "SEXS".into(),
                expected: 6,
                found: 2
            })
        );
    }

    /// Every schema: a record with parameters 1, 2, ... lands field by field
    /// in PSS/E Model Library order.
    #[test]
    fn every_schema_reads_in_record_order() {
        for (name, arity) in SUPPORTED_MODELS {
            let params: Vec<String> = (1..=*arity).map(|i| i.to_string()).collect();
            let text = format!("7 '{name}' 1 {} /", params.join(" "));
            let case = parse_dyr(&text).unwrap();
            let model = typed_model(&case.records()[0]).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(model.name(), *name);
            let fields: Vec<f64> = match model {
                DynamicModel::Gencls(m) => vec![m.h, m.d],
                DynamicModel::Genrou(m) | DynamicModel::Genroe(m) => vec![
                    m.td0_p, m.td0_pp, m.tq0_p, m.tq0_pp, m.h, m.d, m.xd, m.xq, m.xd_p, m.xq_p,
                    m.xd_pp, m.xl, m.s1, m.s12,
                ],
                DynamicModel::Gensal(m) | DynamicModel::Gensae(m) => vec![
                    m.td0_p, m.td0_pp, m.tq0_pp, m.h, m.d, m.xd, m.xq, m.xd_p, m.xd_pp, m.xl, m.s1,
                    m.s12,
                ],
                DynamicModel::Sexs(m) => vec![m.ta_tb, m.tb, m.k, m.te, m.emin, m.emax],
                DynamicModel::Esac1a(m) => vec![
                    m.tr, m.tb, m.tc, m.ka, m.ta, m.vamax, m.vamin, m.te, m.kf, m.tf, m.kc, m.kd,
                    m.ke, m.e1, m.se_e1, m.e2, m.se_e2, m.vrmax, m.vrmin,
                ],
                DynamicModel::Exst1(m) => vec![
                    m.tr, m.vimax, m.vimin, m.tc, m.tb, m.ka, m.ta, m.vrmax, m.vrmin, m.kc, m.kf,
                    m.tf,
                ],
                DynamicModel::Tgov1(m) => vec![m.r, m.t1, m.vmax, m.vmin, m.t2, m.t3, m.dt],
                DynamicModel::Gast(m) => {
                    vec![m.r, m.t1, m.t2, m.t3, m.at, m.kt, m.vmax, m.vmin, m.dturb]
                }
                DynamicModel::Hygov(m) => vec![
                    m.r, m.r_temp, m.tr, m.tf, m.tg, m.velm, m.gmax, m.gmin, m.tw, m.at, m.dturb,
                    m.qnl,
                ],
                DynamicModel::Ieeest(m) => vec![
                    f64::from(m.input_code),
                    m.remote_bus.map_or(0.0, |b| b.0 as f64),
                    m.a1,
                    m.a2,
                    m.a3,
                    m.a4,
                    m.a5,
                    m.a6,
                    m.t1,
                    m.t2,
                    m.t3,
                    m.t4,
                    m.t5,
                    m.t6,
                    m.ks,
                    m.lsmax,
                    m.lsmin,
                    m.vcu,
                    m.vcl,
                ],
            };
            let expected: Vec<f64> = (1..=*arity).map(|i| i as f64).collect();
            assert_eq!(fields, expected, "{name}");
        }
    }

    #[test]
    fn a_token_that_is_not_a_number_is_named() {
        let case = parse_dyr("1 'SEXS' 1 0.1 10 x 0.1 0 3 /").unwrap();
        assert_eq!(
            typed_model(&case.records()[0]),
            Err(ModelError::NotANumber {
                model_name: "SEXS".into(),
                index: 2,
                token: "x".into()
            })
        );
    }

    fn ieeest(ics: &str, ib: &str) -> Result<DynamicModel, ModelError> {
        let text =
            format!("1 'IEEEST' 1 {ics} {ib} 0 0 0 0 0 0 0 0 0 0.02 1.65 1.65 100 0.1 -0.1 0 0 /");
        typed_model(&parse_dyr(&text).unwrap().records()[0])
    }

    #[test]
    fn ieeest_icons_are_integers_in_range() {
        let Ok(DynamicModel::Ieeest(p)) = ieeest("2", "0") else {
            panic!("IEEEST not typed");
        };
        assert_eq!((p.input_code, p.remote_bus), (2, None));
        // An exactly integral float is an integer code.
        let Ok(DynamicModel::Ieeest(p)) = ieeest("6.0", "4294967295") else {
            panic!("IEEEST not typed");
        };
        assert_eq!(
            (p.input_code, p.remote_bus),
            (6, Some(BusId(4_294_967_295)))
        );
        let Ok(DynamicModel::Ieeest(p)) = ieeest("1", "1.0D2") else {
            panic!("IEEEST not typed");
        };
        assert_eq!(p.remote_bus, Some(BusId(100)));
        let invalid = |index: usize, token: &str| {
            Err(ModelError::InvalidIcon {
                model_name: "IEEEST".into(),
                index,
                token: token.into(),
            })
        };
        assert_eq!(ieeest("1.5", "0"), invalid(0, "1.5"));
        assert_eq!(ieeest("0", "0"), invalid(0, "0"));
        assert_eq!(ieeest("7", "0"), invalid(0, "7"));
        assert_eq!(ieeest("1", "-1"), invalid(1, "-1"));
        assert_eq!(ieeest("1", "4294967296"), invalid(1, "4294967296"));
        assert_eq!(ieeest("1", "2.5"), invalid(1, "2.5"));
        // A token that is no number at all stays NotANumber.
        assert!(matches!(
            ieeest("x", "0"),
            Err(ModelError::NotANumber { index: 0, .. })
        ));
        assert_eq!(
            invalid(0, "7").unwrap_err().to_string(),
            "IEEEST parameter 1 (\"7\") is not a valid integer code"
        );
    }

    #[test]
    fn fortran_d_exponents_are_numbers() {
        let case = parse_dyr("1 'SEXS' 1 1.0D-1 1.0d1 2.0D+1 0.1 -5.0D0 5D0 /").unwrap();
        let Ok(DynamicModel::Sexs(s)) = typed_model(&case.records()[0]) else {
            panic!("SEXS not typed");
        };
        let read = [s.ta_tb, s.tb, s.k, s.te, s.emin, s.emax];
        let expected = [0.1, 10.0, 20.0, 0.1, -5.0, 5.0];
        assert!(
            read.iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 1e-12),
            "{read:?}"
        );
        // Errors keep the source token; a second D, or an overflow, is refused.
        for bad in ["1.0D0D0", "1D999", "D"] {
            let text = format!("1 'SEXS' 1 0.1 10 {bad} 0.1 0 3 /");
            let case = parse_dyr(&text).unwrap();
            assert_eq!(
                typed_model(&case.records()[0]),
                Err(ModelError::NotANumber {
                    model_name: "SEXS".into(),
                    index: 2,
                    token: bad.into()
                }),
                "{bad}"
            );
        }
    }

    /// Buses 101 to 104; bus 102 holds 'G2', '1' and the out-of-service
    /// 'OS', bus 103 'ab' and 'AB', bus 104 'C' and 'Gt'.
    fn network() -> BalancedNetwork {
        use std::fmt::Write as _;
        let generator = |bus: u32, id: &str, status: u8| {
            format!(
                "{bus},'{id}', 10.0, 0.0, 100.0, -100.0, 1.0, 0, 200.0, 1.0E-3, 0.3, 0.0, 0.0, 1.0, {status}, 100.0, 100.0, 0.0, 1, 1.0\n"
            )
        };
        let mut raw = String::from("0, 100.00, 33, 0, 0, 60.00 / test\nfirst\nsecond\n");
        for bus in 101..=104 {
            let kind = if bus == 101 { 3 } else { 2 };
            writeln!(raw, "{bus},'B{bus}', 230.0, {kind}, 1, 1, 1, 1.0, 0.0").unwrap();
        }
        raw += "0 / End of Bus data\n0 / End of Load data\n0 / End of Fixed shunt data\n";
        for (bus, id, status) in [
            (101, "1", 1),
            (102, "G2", 1),
            (102, "1", 1),
            (102, "OS", 0),
            (103, "ab", 1),
            (103, "AB", 1),
            (104, "C", 1),
            (104, "Gt", 1),
        ] {
            raw += &generator(bus, id, status);
        }
        raw += "0 / End of Generator data\n";
        for to in 102..=104 {
            writeln!(raw, "101, {to},'1', 0.0, 0.1, 0.0, 100.0, 100.0, 100.0, 0.0, 0.0, 0.0, 0.0, 1, 1, 0.0, 1, 1.0").unwrap();
        }
        raw += "0 / End of Branch data\n0 / End of Transformer data\nQ\n";
        crate::format::psse::parse_psse_source(
            &raw,
            None,
            &mut crate::diagnostics::Diagnostics::default(),
        )
        .unwrap()
    }

    fn link(dyr: &str) -> MachineLinking {
        link_machines(&network(), &parse_dyr(dyr).unwrap())
    }

    fn unit<'a>(linking: &'a MachineLinking, bus: usize, id: &str) -> &'a MachineDynamics {
        linking
            .machines
            .iter()
            .find(|m| m.bus == BusId(bus) && m.machine_id == id)
            .unwrap_or_else(|| panic!("no unit ({bus}, {id})"))
    }

    #[test]
    fn machines_carry_static_bus_ids() {
        let net = network();
        assert_eq!(
            psse_machine_ids(&net),
            ["1", "G2", "1", "OS", "ab", "AB", "C", "Gt"]
        );
        let linking = link("");
        assert_eq!(linking.machines.len(), 7);
        let g2 = unit(&linking, 102, "G2");
        assert_eq!((g2.generator, g2.bus, g2.mbase), (1, BusId(102), 200.0));
        assert_eq!(linking.issues, []);
    }

    /// PSS/E replaces an earlier model of the same category for one machine
    /// with a later one; so does the linking, and it says so.
    #[test]
    fn a_later_record_of_one_role_replaces_the_earlier() {
        let linking = link(
            "102 'GENROU' 1 8 0.03 0.4 0.05 6 0 1.8 1.7 0.3 0.55 0.25 0.2 0.1 0.8 /
102 'SEXS' 1 0.1 10 100 0.1 0 4 /
102 'GENCLS' 1 5.0 1.0 /
102 'SEXS' '1 ' 0.2 10 100 0.1 0 4 /
",
        );
        let g = unit(&linking, 102, "1");
        assert!(matches!(
            g.machine,
            Some(LinkedModel {
                model: DynamicModel::Gencls(_),
                line: 3
            })
        ));
        assert!(matches!(
            &g.exciter,
            Some(LinkedModel { model: DynamicModel::Sexs(s), line: 4 }) if (s.ta_tb - 0.2).abs() < 1e-12
        ));
        assert_eq!(
            linking.issues,
            [
                LinkIssue::Replaced {
                    line: 3,
                    replaced_line: 1,
                    bus: BusId(102),
                    machine_id: "1".into(),
                    model_name: "GENCLS".into(),
                    role: ModelRole::Machine,
                },
                LinkIssue::Replaced {
                    line: 4,
                    replaced_line: 2,
                    bus: BusId(102),
                    machine_id: "1".into(),
                    model_name: "SEXS".into(),
                    role: ModelRole::Exciter,
                },
            ]
        );
        assert_eq!(
            linking.issues[0].to_string(),
            "line 3: GENCLS replaces the Machine model of line 1 for generator (102, '1')"
        );
    }

    /// Unquoted alphabetic ids, as WECC 240 writes them, and ids that differ
    /// from the static case only in ASCII case.
    #[test]
    fn machine_ids_match_exactly_then_ignoring_case() {
        let linking = link(
            "104 'GENCLS' C 3.0 0.0 /
104 'GENCLS' GT 4.0 0.0 /
104 'SEXS' gt 0.1 10 100 0.1 0 4 /
103 'GENCLS' ab 1.0 0.0 /
103 'GENCLS' AB 2.0 0.0 /
103 'GENCLS' Ab 9.0 0.0 /
102 'GENCLS' os 1.0 0.0 /
102 'GENCLS' G 1.0 0.0 /
",
        );
        let h = |bus, id| match &unit(&linking, bus, id).machine {
            Some(LinkedModel {
                model: DynamicModel::Gencls(g),
                ..
            }) => g.h,
            other => panic!("({bus}, {id}): {other:?}"),
        };
        assert!((h(104, "C") - 3.0).abs() < 1e-12);
        // 'GT' is no id on bus 104; the one id equal ignoring case is 'Gt'.
        assert!((h(104, "Gt") - 4.0).abs() < 1e-12);
        assert!(unit(&linking, 104, "Gt").exciter.is_some());
        // Exact matches win over the case-folded ones.
        assert!((h(103, "ab") - 1.0).abs() < 1e-12);
        assert!((h(103, "AB") - 2.0).abs() < 1e-12);
        assert_eq!(
            linking.issues,
            [
                LinkIssue::AmbiguousMachineId {
                    line: 6,
                    model_name: "GENCLS".into(),
                    bus: BusId(103),
                    machine_id: "Ab".into(),
                    candidates: vec!["ab".into(), "AB".into()],
                },
                LinkIssue::OutOfService {
                    line: 7,
                    model_name: "GENCLS".into(),
                    bus: BusId(102),
                    machine_id: "os".into(),
                },
                LinkIssue::NoGenerator {
                    line: 8,
                    model_name: "GENCLS".into(),
                    bus: BusId(102),
                    machine_id: "G".into(),
                },
            ]
        );
        // The stored ids are never folded.
        assert_eq!(unit(&linking, 104, "Gt").machine_id, "Gt");
    }

    /// An untyped record names a generator only when its model is known to
    /// attach to one.
    #[test]
    fn untyped_records_name_a_generator_only_for_generator_models() {
        let linking = link(
            "102 'IEEET1' 1 0 400 0.04 7.3 -7.3 1 0.8 0 0.03 1 0 0 0 0 /
102 'REECA1' 'G2' 1 2 /
102 'SEXS' 1 0.1 10 /
102 'CLODBL' 1 1 2 /
2 'LDS3BL' * 1 2 /
'DC1' 'CDC4T' 1 2 /
102 'USRMDL' 1 'X' 1 2 /
",
        );
        let named: Vec<_> = linking
            .issues
            .iter()
            .map(|issue| match issue {
                LinkIssue::Untyped {
                    bus, machine_id, ..
                } => (*bus, machine_id.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        let generator = |id: &str| (Some(BusId(102)), Some(id.to_owned()));
        assert_eq!(
            named,
            [
                generator("1"),
                generator("G2"),
                generator("1"),
                (None, None),
                (None, None),
                (None, None),
                (None, None),
            ]
        );
        assert_eq!(
            linking.issues[0].to_string(),
            "line 1: model IEEET1 has no typed view (generator (102, '1'))"
        );
        assert_eq!(
            linking.issues[3].to_string(),
            "line 4: record not linked: model CLODBL has no typed view"
        );
    }
}
