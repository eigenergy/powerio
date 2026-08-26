use std::path::{Path, PathBuf};

use powerio_tx::{GenCostPolicyReport, MissingGenCostPolicy};

use crate::Result;
use powerio_matrix::SparseMatrix;
use serde::Serialize;

use crate::prep::{DcOpfPreparation, Units};

use super::build_dc_opf_matrices;

const DCOPF_SCHEMA: &str = "powerio.dcopf";

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
    case_name: &'a str,
    base_mva: f64,
    dimensions: DcOpfDimensions,
    index_base: IndexBaseMeta,
    dc_convention: powerio_tx::DcConvention,
    build_options: BuildOptionsMeta,
    zero_impedance: ZeroImpedanceMeta<'a>,
    grounding: GroundingMeta<'a>,
    operators: Vec<OperatorMeta>,
    n: usize,
    m: usize,
    n_gen: usize,
    reference_buses: &'a [usize],
    convention: powerio_tx::DcConvention,
    units: Units,
    cost_policy: MissingGenCostPolicy,
    synthesized_gen_costs: usize,
    patched_gen_costs: usize,
    files: Vec<String>,
    powerio_version: &'static str,
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
    synthesize_unrated_limits: bool,
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
    instance: &DcOpfPreparation,
    out_dir: impl AsRef<Path>,
    options: &DcOpfBundleOptions,
) -> Result<DcOpfOutputs> {
    let matrices = build_dc_opf_matrices(instance);
    let nodal = instance.nodal_generator_data();
    let fixed_withdrawal = instance.fixed_nodal_withdrawal();
    let flow_offset = instance.branch_flow_offset();
    // The case name comes from source file content, so it must not steer the
    // output path. `sanitize_stem` reduces it to one safe component and
    // disambiguates names that would otherwise sanitize alike, so a batch
    // export cannot be steered into overwriting an earlier bundle.
    let bundle_root = out_dir.as_ref().join(format!(
        "{}_dcopf",
        powerio_matrix::sanitize_stem(&instance.name)
    ));

    let mut inventory: Vec<(&'static str, Vec<u8>)> = Vec::new();
    put_mat(&mut inventory, "A.mtx", &matrices.incidence)?;
    put_mat(&mut inventory, "L.mtx", &matrices.laplacian)?;
    put_mat(
        &mut inventory,
        "L_grounded.mtx",
        &matrices.grounded_laplacian,
    )?;
    put_mat(&mut inventory, "BAt.mtx", &matrices.flow_map)?;
    put_mat(&mut inventory, "Cg.mtx", &matrices.generator_bus)?;

    put_vec(&mut inventory, "b.mtx", &instance.branches.b)?;
    put_vec(&mut inventory, "shift.mtx", &instance.branches.shift)?;
    put_vec(&mut inventory, "flow_offset.mtx", &flow_offset)?;
    put_vec(&mut inventory, "p_shift.mtx", &instance.p_shift)?;
    put_vec(&mut inventory, "fixed_withdrawal.mtx", &fixed_withdrawal)?;
    put_vec(&mut inventory, "e_r.mtx", &matrices.reference_selector)?;
    put_vec(&mut inventory, "q.mtx", &nodal.q)?;
    put_vec(&mut inventory, "c.mtx", &nodal.c)?;
    put_vec(&mut inventory, "c0.mtx", &nodal.c0)?;
    put_vec(&mut inventory, "pmax.mtx", &nodal.pmax)?;
    put_vec(&mut inventory, "pmin.mtx", &nodal.pmin)?;
    put_vec(&mut inventory, "fmax.mtx", &instance.branches.f_max)?;
    put_vec(&mut inventory, "pd.mtx", &instance.p_d)?;
    put_vec(&mut inventory, "gs.mtx", &instance.g_s)?;
    put_vec(
        &mut inventory,
        "angle_min.mtx",
        &instance.branches.angle_min,
    )?;
    put_vec(
        &mut inventory,
        "angle_max.mtx",
        &instance.branches.angle_max,
    )?;

    put_vec(&mut inventory, "q_gen.mtx", &instance.generators.q)?;
    put_vec(&mut inventory, "c_gen.mtx", &instance.generators.c)?;
    put_vec(&mut inventory, "c0_gen.mtx", &instance.generators.c0)?;
    put_vec(&mut inventory, "pmax_gen.mtx", &instance.generators.pmax)?;
    put_vec(&mut inventory, "pmin_gen.mtx", &instance.generators.pmin)?;

    let power_units = match instance.units {
        Units::PerUnit => "per_unit_power",
        Units::Native => "native_power",
    };
    let meta = DcOpfMeta {
        schema: DCOPF_SCHEMA,
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
            synthesize_unrated_limits: instance.synthesize_unrated_limits,
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
            reference_buses: instance.reference_buses.as_ref(),
            removed_rows_and_columns: instance.reference_buses.as_ref(),
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
        reference_buses: instance.reference_buses.as_ref(),
        convention: instance.convention,
        units: instance.units,
        cost_policy: options.metadata.cost_policy,
        synthesized_gen_costs: options.metadata.cost_report.synthesized,
        patched_gen_costs: options.metadata.cost_report.patched,
        // The manifest lists the operator files it describes; it does not
        // list itself, matching the wire form consumers already read.
        files: inventory
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect(),
        powerio_version: powerio_tx::VERSION,
    };
    let json = serde_json::to_string_pretty(&meta)
        .map_err(|error| powerio_matrix::Error::Mtx(error.to_string()))?;
    inventory.push(("dcopf_meta.json", json.into_bytes()));

    // The complete bundle commits at once through the no-replace destination:
    // an existing bundle directory is refused rather than replaced.
    let artifacts = inventory
        .into_iter()
        .map(|(name, bytes)| {
            Ok(powerio_core::MemoryArtifact::new(
                powerio_core::ArtifactPath::new(name)?,
                bytes,
            ))
        })
        .collect::<std::result::Result<Vec<_>, powerio_core::Error>>()
        .map_err(powerio_matrix::Error::from)?;
    let committed = powerio_core::Destination::path(&bundle_root)
        .__commit_artifacts(true, artifacts, Vec::new())
        .map_err(powerio_matrix::Error::from)?;
    let powerio_core::WrittenOutput::Path { root, artifacts } = committed.into_output() else {
        unreachable!("a path destination returns a path output")
    };

    Ok(DcOpfOutputs {
        dir: root,
        files: artifacts,
    })
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
            "branch_phase_shift",
            "shift.mtx",
            "vector",
            m,
            1,
            "branch",
            "radian",
        ),
        op(
            "branch_flow_offset",
            "flow_offset.mtx",
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
            "fixed_nodal_withdrawal",
            "fixed_withdrawal.mtx",
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
            "bus_cost_constant",
            "c0.mtx",
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
            "bus_shunt_conductance",
            "gs.mtx",
            "vector",
            n,
            1,
            "bus",
            power_units,
        ),
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
            "generator_cost_constant",
            "c0_gen.mtx",
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

fn put_mat(
    inventory: &mut Vec<(&'static str, Vec<u8>)>,
    name: &'static str,
    matrix: &SparseMatrix,
) -> Result<()> {
    inventory.push((name, powerio_matrix::io::mtx_bytes(matrix)?));
    Ok(())
}

fn put_vec(
    inventory: &mut Vec<(&'static str, Vec<u8>)>,
    name: &'static str,
    values: &[f64],
) -> Result<()> {
    inventory.push((name, powerio_matrix::io::vector_mtx_bytes(values)?));
    Ok(())
}
