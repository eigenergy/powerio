//! Writes the static DC-OPF bundle for a case: one directory of named
//! Matrix Market files plus a JSON manifest.
//!
//! Everything here is a pure function of the case — the incidence `A`, the
//! DC Laplacian `L` and its slack-grounded form, the flow map `B Aᵀ`, the
//! generator cost and limit data, the generator→bus map, and nodal load.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sprs::CsMat;

use crate::case::MpcCase;
use crate::io::mtx::{write_mtx, write_vector_mtx};
use crate::matrix::incidence::{DcConvention, build_flow_map, build_incidence};
use crate::matrix::laplacian::{build_weighted_laplacian, ground_at, unit_vector};
use crate::matrix::opf::{Units, build_opf_instance};
use crate::Result;

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
    reference_bus: usize,
    convention: DcConvention,
    units: Units,
    files: Vec<String>,
    casemat_version: String,
}

/// Build and write the DC-OPF bundle into `out_dir/<case>_dcopf/`.
pub fn write_dcopf_bundle(
    case: &MpcCase,
    out_dir: impl AsRef<Path>,
    opts: &DcOpfOptions,
) -> Result<DcOpfOutputs> {
    let dir = out_dir.as_ref().join(format!("{}_dcopf", case.name));
    std::fs::create_dir_all(&dir)?;

    let r = case.reference_bus_index()?;
    let inc = build_incidence(case, opts.convention)?;
    let l = build_weighted_laplacian(&inc.a, &inc.b);
    let l_grounded = ground_at(&l, r);
    let flow = build_flow_map(&inc.a, &inc.b);
    let opf = build_opf_instance(case, &inc, opts.units)?;
    let e_r = unit_vector(case.n(), r);

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
    put_vec(&dir, "e_r.mtx", &e_r, &mut files)?;
    put_vec(&dir, "q.mtx", &opf.q_bus, &mut files)?;
    put_vec(&dir, "c.mtx", &opf.c_bus, &mut files)?;
    put_vec(&dir, "pmax.mtx", &opf.pmax_bus, &mut files)?;
    put_vec(&dir, "pmin.mtx", &opf.pmin_bus, &mut files)?;
    put_vec(&dir, "fmax.mtx", &opf.f_max, &mut files)?;
    put_vec(&dir, "pd.mtx", &opf.p_d, &mut files)?;

    // Generator-space provenance.
    put_vec(&dir, "q_gen.mtx", &opf.q_gen, &mut files)?;
    put_vec(&dir, "c_gen.mtx", &opf.c_gen, &mut files)?;
    put_vec(&dir, "pmax_gen.mtx", &opf.pmax_gen, &mut files)?;
    put_vec(&dir, "pmin_gen.mtx", &opf.pmin_gen, &mut files)?;

    let meta = DcOpfMeta {
        case_name: case.name.clone(),
        base_mva: case.base_mva,
        n: case.n(),
        m: inc.m(),
        n_gen: opf.n_gen,
        reference_bus: r,
        convention: opts.convention,
        units: opts.units,
        files: files
            .iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_string))
            .collect(),
        casemat_version: env!("CARGO_PKG_VERSION").to_string(),
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
