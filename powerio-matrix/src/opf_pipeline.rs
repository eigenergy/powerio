//! Writes the static DC OPF bundle for a case: one directory of named
//! Matrix Market files plus a JSON manifest.
//!
//! Everything here is a pure function of the case: the signed incidence matrix
//! `A`, the DC bus susceptance matrix `L = A diag(b) Aᵀ` and its
//! reference-grounded form, the flow map `B Aᵀ`, the generator cost and limit
//! data, the generator→bus map, and nodal load.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sprs::CsMat;

use crate::Result;
use crate::indexed::IndexedNetwork;
use crate::io::mtx::{write_mtx, write_vector_mtx};
use crate::matrix::incidence::{DcConvention, build_flow_map, build_incidence};
use crate::matrix::laplacian::{build_weighted_laplacian, ground_at_each, reference_indicator};
use crate::matrix::opf::{Units, build_opf_instance};
use crate::matrix::{BuildOptions, ZeroImpedanceRule, ZeroImpedanceSkips};
use crate::network::Network;
use crate::{GenCostPatch, MissingGenCostPolicy};

const DCOPF_SCHEMA: &str = "powerio.dcopf";
const DCOPF_SCHEMA_VERSION: &str = "0.1.0";

#[derive(Debug, Clone)]
pub struct DcOpfOptions {
    pub convention: DcConvention,
    pub units: Units,
    pub missing_gen_cost: MissingGenCostPolicy,
    pub gen_cost_patches: Vec<GenCostPatch>,
}

impl Default for DcOpfOptions {
    fn default() -> Self {
        Self {
            convention: DcConvention::default(),
            units: Units::default(),
            missing_gen_cost: MissingGenCostPolicy::Require,
            gen_cost_patches: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DcOpfOutputs {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

#[derive(Serialize)]
struct DcOpfMeta {
    schema: &'static str,
    schema_version: &'static str,
    case_name: String,
    base_mva: f64,
    dimensions: DcOpfDimensions,
    index_base: IndexBaseMeta,
    dc_convention: DcConvention,
    build_options: BuildOptions,
    zero_impedance: ZeroImpedanceMeta,
    grounding: GroundingMeta,
    operators: Vec<OperatorMeta>,
    /// Backward compatible aliases retained for current readers.
    n: usize,
    /// Backward compatible alias for the number of incidence columns.
    m: usize,
    /// Backward compatible alias for in-service generators.
    n_gen: usize,
    /// Dense indices of every grounded reference (slack) bus. Several entries
    /// mean one reference per island, or several reference buses fixed in one
    /// island. The solver grounds the DC bus susceptance matrix at all of them, matching
    /// `L_grounded` and `e_r`.
    reference_buses: Vec<usize>,
    /// Backward compatible alias for `dc_convention`.
    convention: DcConvention,
    units: Units,
    cost_policy: MissingGenCostPolicy,
    synthesized_gen_costs: usize,
    patched_gen_costs: usize,
    files: Vec<String>,
    powerio_version: String,
}

#[derive(Serialize)]
#[allow(clippy::struct_field_names)]
struct DcOpfDimensions {
    n_buses: usize,
    n_source_branches: usize,
    n_branch_columns: usize,
    n_generators: usize,
    n_reference_buses: usize,
    n_grounded_buses: usize,
}

#[derive(Serialize)]
struct IndexBaseMeta {
    dense: usize,
    matrix_market: usize,
}

#[derive(Serialize)]
struct ZeroImpedanceMeta {
    skip: bool,
    rule: ZeroImpedanceRule,
    skipped: ZeroImpedanceSkips,
}

#[derive(Serialize)]
struct GroundingMeta {
    reference_buses: Vec<usize>,
    removed_rows_and_columns: Vec<usize>,
    grounded_operator: &'static str,
    reference_selector: &'static str,
}

#[derive(Serialize)]
struct OperatorMeta {
    name: &'static str,
    file: &'static str,
    kind: &'static str,
    rows: usize,
    cols: usize,
    index_space: &'static str,
    units: &'static str,
}

/// Build and write the DC OPF bundle into `out_dir/<case>_dcopf/`.
pub fn write_dcopf_bundle(
    net: &Network,
    out_dir: impl AsRef<Path>,
    opts: &DcOpfOptions,
) -> Result<DcOpfOutputs> {
    let mut policy_net = net.clone();
    let cost_report =
        policy_net.apply_gen_cost_policy(&opts.gen_cost_patches, opts.missing_gen_cost)?;
    let view = IndexedNetwork::new(&policy_net);

    let dir = out_dir.as_ref().join(format!("{}_dcopf", view.name()));
    std::fs::create_dir_all(&dir)?;

    view.check_reference_coverage()?;
    let refs = view.reference_bus_indices();
    let build_options = BuildOptions::default();
    let inc = build_incidence(&view, opts.convention, &build_options)?;
    let l = build_weighted_laplacian(&inc.a, &inc.b);
    let l_grounded = ground_at_each(&l, &refs);
    let flow = build_flow_map(&inc.a, &inc.b);
    let opf = build_opf_instance(&view, &inc, opts.units)?;
    let e_r = reference_indicator(view.n(), &refs);

    let mut files = Vec::new();

    // Network operators.
    put_mat(&dir, "A.mtx", &inc.a, &mut files)?;
    put_mat(&dir, "L.mtx", &l, &mut files)?;
    put_mat(&dir, "L_grounded.mtx", &l_grounded, &mut files)?;
    put_mat(&dir, "BAt.mtx", &flow, &mut files)?;
    put_mat(&dir, "Cg.mtx", &opf.c_g, &mut files)?;

    // Network / OPF vectors (bus or branch indexed).
    put_vec(&dir, "b.mtx", &inc.b, &mut files)?;
    put_vec(&dir, "p_shift.mtx", &inc.p_shift, &mut files)?;
    // e_r is 1 at every reference bus, not a single-slack one-hot: read it
    // alongside `reference_buses` in the manifest (one entry ⇒ the old one-hot).
    put_vec(&dir, "e_r.mtx", &e_r, &mut files)?;
    put_vec(&dir, "q.mtx", &opf.bus.q, &mut files)?;
    put_vec(&dir, "c.mtx", &opf.bus.c, &mut files)?;
    put_vec(&dir, "pmax.mtx", &opf.bus.pmax, &mut files)?;
    put_vec(&dir, "pmin.mtx", &opf.bus.pmin, &mut files)?;
    put_vec(&dir, "fmax.mtx", &opf.f_max, &mut files)?;
    put_vec(&dir, "pd.mtx", &opf.bus.p_d, &mut files)?;

    // Generator-space provenance.
    put_vec(&dir, "q_gen.mtx", &opf.gen_costs.q, &mut files)?;
    put_vec(&dir, "c_gen.mtx", &opf.gen_costs.c, &mut files)?;
    put_vec(&dir, "pmax_gen.mtx", &opf.gen_costs.pmax, &mut files)?;
    put_vec(&dir, "pmin_gen.mtx", &opf.gen_costs.pmin, &mut files)?;

    let meta = DcOpfMeta {
        schema: DCOPF_SCHEMA,
        schema_version: DCOPF_SCHEMA_VERSION,
        case_name: view.name().to_string(),
        base_mva: view.base_mva(),
        dimensions: DcOpfDimensions {
            n_buses: view.n(),
            n_source_branches: view.branches().len(),
            n_branch_columns: inc.m(),
            n_generators: opf.n_gen(),
            n_reference_buses: refs.len(),
            n_grounded_buses: view.n() - refs.len(),
        },
        index_base: IndexBaseMeta {
            dense: 0,
            matrix_market: 1,
        },
        dc_convention: opts.convention,
        build_options: build_options.clone(),
        zero_impedance: ZeroImpedanceMeta {
            skip: build_options.skip_zero_impedance,
            rule: ZeroImpedanceRule::Reactance,
            skipped: inc.skipped_zero_impedance.clone(),
        },
        grounding: GroundingMeta {
            reference_buses: refs.clone(),
            removed_rows_and_columns: refs.clone(),
            grounded_operator: "L_grounded",
            reference_selector: "e_r",
        },
        operators: operator_meta(view.n(), inc.m(), refs.len(), opf.n_gen()),
        n: view.n(),
        m: inc.m(),
        n_gen: opf.n_gen(),
        reference_buses: refs.clone(),
        convention: opts.convention,
        units: opts.units,
        cost_policy: opts.missing_gen_cost,
        synthesized_gen_costs: cost_report.synthesized,
        patched_gen_costs: cost_report.patched,
        files: files
            .iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_string))
            .collect(),
        powerio_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let meta_path = dir.join("dcopf_meta.json");
    let json = serde_json::to_string_pretty(&meta).map_err(|e| crate::Error::Mtx(e.to_string()))?;
    std::fs::write(&meta_path, json)?;
    files.push(meta_path);

    Ok(DcOpfOutputs { dir, files })
}

#[allow(clippy::too_many_lines)]
fn operator_meta(n: usize, m: usize, n_ref: usize, n_gen: usize) -> Vec<OperatorMeta> {
    let n_grounded = n - n_ref;
    vec![
        op(
            "signed_incidence",
            "A.mtx",
            "matrix",
            n,
            m,
            "bus_by_branch",
            "unitless",
        ),
        op(
            "branch_susceptance",
            "b.mtx",
            "vector",
            m,
            1,
            "branch",
            "per_unit_susceptance",
        ),
        op(
            "weighted_laplacian",
            "L.mtx",
            "matrix",
            n,
            n,
            "bus_by_bus",
            "per_unit_susceptance",
        ),
        op(
            "grounded_laplacian",
            "L_grounded.mtx",
            "matrix",
            n_grounded,
            n_grounded,
            "grounded_bus_by_grounded_bus",
            "per_unit_susceptance",
        ),
        op(
            "flow_map",
            "BAt.mtx",
            "matrix",
            m,
            n,
            "branch_by_bus",
            "per_unit_susceptance",
        ),
        op(
            "generator_to_bus",
            "Cg.mtx",
            "matrix",
            n,
            n_gen,
            "bus_by_generator",
            "unitless",
        ),
        op(
            "phase_shift_injection",
            "p_shift.mtx",
            "vector",
            n,
            1,
            "bus",
            "per_unit_power",
        ),
        op(
            "reference_selector",
            "e_r.mtx",
            "vector",
            n,
            1,
            "bus",
            "indicator",
        ),
        op(
            "bus_cost_quadratic",
            "q.mtx",
            "vector",
            n,
            1,
            "bus",
            "selected_cost_units",
        ),
        op(
            "bus_cost_linear",
            "c.mtx",
            "vector",
            n,
            1,
            "bus",
            "selected_cost_units",
        ),
        op(
            "bus_generation_upper",
            "pmax.mtx",
            "vector",
            n,
            1,
            "bus",
            "selected_power_units",
        ),
        op(
            "bus_generation_lower",
            "pmin.mtx",
            "vector",
            n,
            1,
            "bus",
            "selected_power_units",
        ),
        op(
            "branch_flow_limit",
            "fmax.mtx",
            "vector",
            m,
            1,
            "branch",
            "selected_power_units",
        ),
        op(
            "bus_load",
            "pd.mtx",
            "vector",
            n,
            1,
            "bus",
            "selected_power_units",
        ),
        op(
            "generator_cost_quadratic",
            "q_gen.mtx",
            "vector",
            n_gen,
            1,
            "generator",
            "selected_cost_units",
        ),
        op(
            "generator_cost_linear",
            "c_gen.mtx",
            "vector",
            n_gen,
            1,
            "generator",
            "selected_cost_units",
        ),
        op(
            "generator_upper",
            "pmax_gen.mtx",
            "vector",
            n_gen,
            1,
            "generator",
            "selected_power_units",
        ),
        op(
            "generator_lower",
            "pmin_gen.mtx",
            "vector",
            n_gen,
            1,
            "generator",
            "selected_power_units",
        ),
    ]
}

fn op(
    name: &'static str,
    file: &'static str,
    kind: &'static str,
    rows: usize,
    cols: usize,
    index_space: &'static str,
    units: &'static str,
) -> OperatorMeta {
    OperatorMeta {
        name,
        file,
        kind,
        rows,
        cols,
        index_space,
        units,
    }
}

fn put_mat(dir: &Path, name: &str, m: &CsMat<f64>, files: &mut Vec<PathBuf>) -> Result<()> {
    let p = dir.join(name);
    write_mtx(m, &p)?;
    files.push(p);
    Ok(())
}

fn put_vec(dir: &Path, name: &str, v: &[f64], files: &mut Vec<PathBuf>) -> Result<()> {
    let p = dir.join(name);
    write_vector_mtx(v, &p)?;
    files.push(p);
    Ok(())
}
