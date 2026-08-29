//! Streamed Matrix Market output for DC sensitivity matrices.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Result;
use crate::indexed::IndexedNetwork;
use crate::matrix::sensitivity::for_each_ptdf_lodf_entry;
use crate::matrix::{SensitivityMetadata, SensitivityOptions};

/// Write PTDF and LODF Matrix Market files from the option based sensitivity
/// path and return the metadata for the same entries. The sparse solver path
/// streams retained coordinates through temp files, so it does not keep the
/// full sparse output in memory.
pub fn write_sensitivity_mtx_with_options(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
    ptdf_path: impl AsRef<Path>,
    lodf_path: impl AsRef<Path>,
) -> Result<SensitivityMetadata> {
    let mut ptdf = CoordinateMtxWriter::new(ptdf_path.as_ref())?;
    let mut lodf = CoordinateMtxWriter::new(lodf_path.as_ref())?;

    let metadata = match for_each_ptdf_lodf_entry(
        case,
        options,
        |row, col, value| ptdf.write_entry(row, col, value),
        |row, col, value| lodf.write_entry(row, col, value),
    ) {
        Ok(metadata) => metadata,
        Err(err) => {
            ptdf.cleanup();
            lodf.cleanup();
            return Err(err);
        }
    };

    let ptdf_target = ptdf.target_path.clone();
    if let Err(error) = ptdf.finish(metadata.ptdf.rows, metadata.ptdf.cols) {
        // The refused first target must not strand the second writer's
        // staging files.
        lodf.cleanup();
        return Err(error);
    }
    if let Err(error) = lodf.finish(metadata.lodf.rows, metadata.lodf.cols) {
        // The two targets are produced together or neither is: remove the
        // file this call itself committed at the first target. The commit
        // refused any pre-existing entry, so this removal can only take back
        // this call's own output.
        let _ = std::fs::remove_file(&ptdf_target);
        return Err(error);
    }
    Ok(metadata)
}

struct CoordinateMtxWriter {
    target_path: PathBuf,
    body_path: PathBuf,
    final_tmp_path: PathBuf,
    body: Option<BufWriter<File>>,
    nnz: usize,
}

impl CoordinateMtxWriter {
    fn new(target_path: &Path) -> Result<Self> {
        let (body_path, body) = create_exclusive_staging(target_path, "body")?;
        // The final staging path is claimed at finish time with the same
        // exclusive open; here only the name is drawn.
        let final_tmp_path = temp_path(target_path, "final");
        Ok(Self {
            target_path: target_path.to_path_buf(),
            body_path,
            final_tmp_path,
            body: Some(BufWriter::new(body)),
            nnz: 0,
        })
    }

    fn write_entry(&mut self, row: usize, col: usize, value: f64) -> Result<()> {
        if value == 0.0 {
            return Ok(());
        }
        let body = self
            .body
            .as_mut()
            .expect("coordinate writer body is open before finish");
        writeln!(body, "{} {} {:.16e}", row + 1, col + 1, value)?;
        self.nnz += 1;
        Ok(())
    }

    fn finish(mut self, rows: usize, cols: usize) -> Result<()> {
        let result = self.finish_inner(rows, cols);
        if result.is_err() {
            // A refused or failed commit leaves no staging file beside the
            // target; the commit itself already removed the staged output.
            self.cleanup();
        }
        result
    }

    fn finish_inner(&mut self, rows: usize, cols: usize) -> Result<()> {
        if let Some(mut body) = self.body.take() {
            body.flush()?;
        }

        let (final_tmp_path, staged) = create_exclusive_staging(&self.target_path, "final")?;
        self.final_tmp_path = final_tmp_path;
        let mut out = BufWriter::new(staged);
        writeln!(out, "%%MatrixMarket matrix coordinate real general")?;
        writeln!(out, "% written by powerio")?;
        writeln!(out, "{rows} {cols} {}", self.nnz)?;
        let mut body = BufReader::new(File::open(&self.body_path)?);
        std::io::copy(&mut body, &mut out)?;
        out.flush()?;
        drop(out);

        // The complete staged matrix moves onto the target through the
        // no-replace commit: an entry at the target refuses the write and the
        // staged file is removed, leaving the caller's filesystem as it was.
        powerio_core::__implementation::__commit_staged_file(
            &self.final_tmp_path,
            &self.target_path,
        )?;
        let _ = std::fs::remove_file(&self.body_path);
        Ok(())
    }

    fn cleanup(&mut self) {
        if let Some(mut body) = self.body.take() {
            let _ = body.flush();
        }
        let _ = std::fs::remove_file(&self.body_path);
        let _ = std::fs::remove_file(&self.final_tmp_path);
    }
}

/// Create one staging file exclusively: an entry already at the drawn name,
/// of any kind, refuses that name instead of being opened, followed, or
/// truncated, and a fresh name is drawn for a bounded number of attempts —
/// the discipline the destination staging applies.
fn create_exclusive_staging(target_path: &Path, suffix: &str) -> Result<(PathBuf, File)> {
    for _ in 0..32 {
        let candidate = temp_path(target_path, suffix);
        match open_exclusive(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(crate::Error::Mtx(format!(
        "could not choose an unused staging path beside `{}`",
        target_path.display()
    )))
}

/// One exclusive create: an entry of any kind already at `path` — a file, a
/// directory, a live or dangling symbolic link — is `AlreadyExists`, never
/// opened, followed, or truncated.
fn open_exclusive(path: &Path) -> std::io::Result<File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

fn temp_path(target_path: &Path, suffix: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let name = target_path
        .file_name()
        .map_or_else(|| "matrix".into(), |name| name.to_string_lossy());
    target_path.with_file_name(format!(".{name}.{pid}.{nanos}.{suffix}.tmp"))
}

#[cfg(test)]
mod staging_tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "powerio-sensitivity-staging-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_exclusive_open_refuses_every_kind_of_existing_entry() {
        let dir = scratch("open-exclusive");

        let file = dir.join("file");
        std::fs::write(&file, b"kept").unwrap();
        assert_eq!(
            open_exclusive(&file).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"kept");

        let directory = dir.join("dir");
        std::fs::create_dir(&directory).unwrap();
        assert!(open_exclusive(&directory).is_err());

        #[cfg(unix)]
        {
            let designated = dir.join("designated");
            std::fs::write(&designated, b"designated").unwrap();
            let live = dir.join("live-link");
            std::os::unix::fs::symlink(&designated, &live).unwrap();
            assert_eq!(
                open_exclusive(&live).unwrap_err().kind(),
                std::io::ErrorKind::AlreadyExists
            );
            assert_eq!(std::fs::read(&designated).unwrap(), b"designated");

            let dangling = dir.join("dangling-link");
            std::os::unix::fs::symlink(dir.join("missing"), &dangling).unwrap();
            assert!(open_exclusive(&dangling).is_err());
        }

        // A fresh name opens.
        assert!(open_exclusive(&dir.join("fresh")).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
