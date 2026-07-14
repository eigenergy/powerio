use std::path::{Path, PathBuf};

use powerio::{GenCostPolicyReport, MissingGenCostPolicy, Result};
use powerio_matrix::SparseMatrix;
use powerio_matrix::io::{write_mtx, write_vector_mtx};
use serde::Serialize;

use crate::{DcOpfInstance, Units};

use super::build_dc_opf_matrices;

const DCOPF_SCHEMA: &str = "powerio.dcopf";
const DCOPF_SCHEMA_VERSION: &str = "0.3.0";

/// Cost policy information recorded in a bundle manifest.
#[derive(Debug, Clone)]
pub struct DcOpfBundleMetadata {
    pub cost_policy: MissingGenCostPolicy,
    pub cost_report: GenCostPolicyReport,
}

impl Default for DcOpfBundleMetadata {
    fn default() -> Self {
        Self {
            cost_policy: MissingGenCostPolicy::Require,
            cost_report: GenCostPolicyReport::default(),
        }
    }
}

/// Options that affect bundle output without changing the instance.
#[derive(Debug, Clone, Default)]
pub struct DcOpfBundleOptions {
    pub metadata: DcOpfBundleMetadata,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DcOpfOutputs {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

#[derive(Serialize)]
struct DcOpfMeta<'a> {
    schema: &'static str,
    schema_version: &'static str,
    case_name: &'a str,
    base_mva: f64,
    dimensions: DcOpfDimensions,
    index_base: IndexBaseMeta,
    dc_convention: powerio::DcConvention,
    build_options: BuildOptionsMeta,
    zero_impedance: ZeroImpedanceMeta<'a>,
    grounding: GroundingMeta<'a>,
    operators: Vec<OperatorMeta>,
    n: usize,
    m: usize,
    n_gen: usize,
    reference_buses: &'a [usize],
    convention: powerio::DcConvention,
    units: Units,
    cost_policy: MissingGenCostPolicy,
    synthesized_gen_costs: usize,
    patched_gen_costs: usize,
    files: Vec<String>,
    powerio_version: &'static str,
}

/// One path component derived from a case name. Case names come from source
/// files, so separators and anything else outside `[A-Za-z0-9._-]` map to
/// `_`, and a name that would resolve to the current or parent directory
/// falls back to `case`. The bundle always lands under the caller's
/// output directory.
fn directory_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        "case".to_owned()
    } else {
        cleaned
    }
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
struct BuildOptionsMeta {
    skip_zero_impedance: bool,
}

#[derive(Serialize)]
struct ZeroImpedanceMeta<'a> {
    skip: bool,
    rule: &'static str,
    skipped: ZeroImpedanceSkips<'a>,
}

#[derive(Serialize)]
struct ZeroImpedanceSkips<'a> {
    count: usize,
    branch_indices: &'a [usize],
}

#[derive(Serialize)]
struct GroundingMeta<'a> {
    reference_buses: &'a [usize],
    removed_rows_and_columns: &'a [usize],
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

/// Write matrix projections for an assembled DC OPF instance.
///
/// The writer reads all costs, bounds, mappings, units, and conventions from
/// `instance`. It does not retain or read a source network.
#[allow(clippy::too_many_lines)]
pub fn write_dcopf_bundle(
    instance: &DcOpfInstance,
    out_dir: impl AsRef<Path>,
    options: &DcOpfBundleOptions,
) -> Result<DcOpfOutputs> {
    let matrices = build_dc_opf_matrices(instance);
    let nodal = instance.nodal_generator_data()?;
    let dir = out_dir
        .as_ref()
        .join(format!("{}_dcopf", directory_component(&instance.name)));
    std::fs::create_dir_all(&dir)?;

    let mut files = Vec::new();
    put_mat(&dir, "A.mtx", &matrices.incidence, &mut files)?;
    put_mat(&dir, "L.mtx", &matrices.laplacian, &mut files)?;
    put_mat(
        &dir,
        "L_grounded.mtx",
        &matrices.grounded_laplacian,
        &mut files,
    )?;
    put_mat(&dir, "BAt.mtx", &matrices.flow_map, &mut files)?;
    put_mat(&dir, "Cg.mtx", &matrices.generator_bus, &mut files)?;

    put_vec(&dir, "b.mtx", &instance.branches.b, &mut files)?;
    put_vec(&dir, "p_shift.mtx", &instance.p_shift, &mut files)?;
    put_vec(&dir, "e_r.mtx", &matrices.reference_selector, &mut files)?;
    put_vec(&dir, "q.mtx", &nodal.q, &mut files)?;
    put_vec(&dir, "c.mtx", &nodal.c, &mut files)?;
    put_vec(&dir, "c0.mtx", &nodal.c0, &mut files)?;
    put_vec(&dir, "pmax.mtx", &nodal.pmax, &mut files)?;
    put_vec(&dir, "pmin.mtx", &nodal.pmin, &mut files)?;
    put_vec(&dir, "fmax.mtx", &instance.branches.f_max, &mut files)?;
    put_vec(&dir, "pd.mtx", &instance.p_d, &mut files)?;
    put_vec(
        &dir,
        "angle_min.mtx",
        &instance.branches.angle_min,
        &mut files,
    )?;
    put_vec(
        &dir,
        "angle_max.mtx",
        &instance.branches.angle_max,
        &mut files,
    )?;

    put_vec(&dir, "q_gen.mtx", &instance.generators.q, &mut files)?;
    put_vec(&dir, "c_gen.mtx", &instance.generators.c, &mut files)?;
    put_vec(&dir, "c0_gen.mtx", &instance.generators.c0, &mut files)?;
    put_vec(&dir, "pmax_gen.mtx", &instance.generators.pmax, &mut files)?;
    put_vec(&dir, "pmin_gen.mtx", &instance.generators.pmin, &mut files)?;

    let power_units = match instance.units {
        Units::PerUnit => "per_unit_power",
        Units::Native => "native_power",
    };
    let meta = DcOpfMeta {
        schema: DCOPF_SCHEMA,
        schema_version: DCOPF_SCHEMA_VERSION,
        case_name: &instance.name,
        base_mva: instance.base_mva,
        dimensions: DcOpfDimensions {
            n_buses: instance.n_buses,
            n_source_branches: instance.n_source_branches,
            n_branch_columns: instance.n_branches(),
            n_generators: instance.n_generators(),
            n_reference_buses: instance.reference_buses.len(),
            n_grounded_buses: instance.n_buses - instance.reference_buses.len(),
        },
        index_base: IndexBaseMeta {
            dense: 0,
            matrix_market: 1,
        },
        dc_convention: instance.convention,
        build_options: BuildOptionsMeta {
            skip_zero_impedance: instance.skip_zero_impedance,
        },
        zero_impedance: ZeroImpedanceMeta {
            skip: instance.skip_zero_impedance,
            rule: "Reactance",
            skipped: ZeroImpedanceSkips {
                count: instance.branches.skipped_zero_impedance.len(),
                branch_indices: &instance.branches.skipped_zero_impedance,
            },
        },
        grounding: GroundingMeta {
            reference_buses: &instance.reference_buses,
            removed_rows_and_columns: &instance.reference_buses,
            grounded_operator: "L_grounded",
            reference_selector: "e_r",
        },
        operators: operator_meta(
            instance.n_buses,
            instance.n_branches(),
            instance.reference_buses.len(),
            instance.n_generators(),
            power_units,
        ),
        n: instance.n_buses,
        m: instance.n_branches(),
        n_gen: instance.n_generators(),
        reference_buses: &instance.reference_buses,
        convention: instance.convention,
        units: instance.units,
        cost_policy: options.metadata.cost_policy,
        synthesized_gen_costs: options.metadata.cost_report.synthesized,
        patched_gen_costs: options.metadata.cost_report.patched,
        files: files
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect(),
        powerio_version: powerio::VERSION,
    };
    let meta_path = dir.join("dcopf_meta.json");
    let json = serde_json::to_string_pretty(&meta)
        .map_err(|error| powerio::Error::Mtx(error.to_string()))?;
    std::fs::write(&meta_path, json)?;
    files.push(meta_path);

    Ok(DcOpfOutputs { dir, files })
}

#[allow(clippy::too_many_lines)]
fn operator_meta(
    n: usize,
    m: usize,
    n_ref: usize,
    n_gen: usize,
    power_units: &'static str,
) -> Vec<OperatorMeta> {
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
            power_units,
        ),
        op(
            "weighted_laplacian",
            "L.mtx",
            "matrix",
            n,
            n,
            "bus_by_bus",
            power_units,
        ),
        op(
            "grounded_laplacian",
            "L_grounded.mtx",
            "matrix",
            n_grounded,
            n_grounded,
            "grounded_bus_by_grounded_bus",
            power_units,
        ),
        op(
            "flow_map",
            "BAt.mtx",
            "matrix",
            m,
            n,
            "branch_by_bus",
            power_units,
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
            power_units,
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
            power_units,
        ),
        op(
            "bus_generation_lower",
            "pmin.mtx",
            "vector",
            n,
            1,
            "bus",
            power_units,
        ),
        op(
            "branch_flow_limit",
            "fmax.mtx",
            "vector",
            m,
            1,
            "branch",
            power_units,
        ),
        op("bus_load", "pd.mtx", "vector", n, 1, "bus", power_units),
        op(
            "branch_angle_minimum",
            "angle_min.mtx",
            "vector",
            m,
            1,
            "branch",
            "radian",
        ),
        op(
            "branch_angle_maximum",
            "angle_max.mtx",
            "vector",
            m,
            1,
            "branch",
            "radian",
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
            power_units,
        ),
        op(
            "generator_lower",
            "pmin_gen.mtx",
            "vector",
            n_gen,
            1,
            "generator",
            power_units,
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

fn put_mat(dir: &Path, name: &str, matrix: &SparseMatrix, files: &mut Vec<PathBuf>) -> Result<()> {
    let path = dir.join(name);
    write_mtx(matrix, &path)?;
    files.push(path);
    Ok(())
}

fn put_vec(dir: &Path, name: &str, values: &[f64], files: &mut Vec<PathBuf>) -> Result<()> {
    let path = dir.join(name);
    write_vector_mtx(values, &path)?;
    files.push(path);
    Ok(())
}
