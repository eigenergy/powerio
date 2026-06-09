//! Writes the static DC-OPF bundle for a case: one directory of named
//! Matrix Market files plus a JSON manifest.
//!
//! Everything here is a pure function of the case — the incidence `A`, the
//! DC Laplacian `L` and its slack-grounded form, the flow map `B Aᵀ`, the
//! generator cost and limit data, the generator→bus map, and nodal load.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sprs::CsMat;

use crate::Result;
use crate::indexed::IndexedNetwork;
use crate::io::mtx::{write_mtx, write_vector_mtx};
use crate::matrix::incidence::{DcConvention, build_flow_map, build_incidence};
use crate::matrix::laplacian::{build_weighted_laplacian, ground_at_each, reference_indicator};
use crate::matrix::opf::{Units, build_opf_instance};
use crate::network::Network;

#[derive(Debug, Clone, Default)]
pub struct DcOpfOptions {
    pub convention: DcConvention,
    pub units: Units,
}

#[derive(Debug, Clone)]
pub struct DcOpfOutputs {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

#[derive(Serialize)]
struct DcOpfMeta {
    case_name: String,
    base_mva: f64,
    n: usize,
    m: usize,
    n_gen: usize,
    /// Dense indices of every grounded reference (slack) bus. Several entries
    /// mean a slack per island or a distributed slack; the solver grounds the
    /// Laplacian at all of them (matching `L_grounded` and `e_r`).
    reference_buses: Vec<usize>,
    convention: DcConvention,
    units: Units,
    files: Vec<String>,
    powerio_version: String,
}

/// Build and write the DC-OPF bundle into `out_dir/<case>_dcopf/`.
pub fn write_dcopf_bundle(
    net: &Network,
    out_dir: impl AsRef<Path>,
    opts: &DcOpfOptions,
) -> Result<DcOpfOutputs> {
    let view = IndexedNetwork::new(net);

    let dir = out_dir.as_ref().join(format!("{}_dcopf", view.name()));
    std::fs::create_dir_all(&dir)?;

    view.check_groundable()?;
    let refs = view.reference_bus_indices();
    let inc = build_incidence(&view, opts.convention)?;
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
        case_name: view.name().to_string(),
        base_mva: view.base_mva(),
        n: view.n(),
        m: inc.m(),
        n_gen: opf.n_gen(),
        reference_buses: refs.clone(),
        convention: opts.convention,
        units: opts.units,
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
