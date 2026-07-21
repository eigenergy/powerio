//! Top level App state machine for the TUI.
//!
//! State is intentionally a single struct with explicit screens (not an
//! enum based state machine) because nearly every screen needs access to
//! the case list, the log buffer, and the output directory.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use sprs::CsMat;

use powerio_matrix::matrix::{MatrixStats, sddm_check};
use powerio_matrix::network::Network;
use powerio_matrix::pipeline::{MatrixKind, RhsKind, matrix_stats_for_kind};
use powerio_matrix::synth::{SynthSpec, Topology};

use super::log_pane::LogBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Browse,
    Inspect,
    Batch,
    Synth,
    Help,
}

impl Screen {
    pub fn label(self) -> &'static str {
        match self {
            Self::Browse => "Browse",
            Self::Inspect => "Inspect",
            Self::Batch => "Batch",
            Self::Synth => "Synth",
            Self::Help => "Help",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaseEntry {
    pub path: PathBuf,
    pub display_name: String,
    pub parsed: ParseStatus,
}

#[derive(Debug, Clone)]
pub enum ParseStatus {
    NotLoaded,
    Loaded {
        n_buses: usize,
        n_branches: usize,
        base_mva: f64,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct MatrixCell {
    pub matrix: CsMat<f64>,
    pub stats: MatrixStats,
    pub sddm: bool,
}

#[derive(Debug, Clone)]
pub struct InspectState {
    pub case: Network,
    pub kind: MatrixKind,
    pub kind_idx: usize,
    pub matrices: BTreeMap<MatrixKindOrd, MatrixCell>,
}

/// `MatrixKind` does not implement `Ord`; this thin wrapper does, so we
/// can use it as a `BTreeMap` key without changing the public type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatrixKindOrd(u8);

impl MatrixKindOrd {
    pub const fn from_kind(k: MatrixKind) -> Self {
        Self(match k {
            MatrixKind::BPrime => 0,
            MatrixKind::BDoublePrime => 1,
            MatrixKind::YbusG => 2,
            MatrixKind::YbusB => 3,
            MatrixKind::Lacpf => 4,
            MatrixKind::Adjacency => 5,
            // A future MatrixKind sorts after the current set until given an
            // explicit ordinal here.
            _ => 6,
        })
    }
}

#[derive(Debug, Clone)]
pub enum BatchProgress {
    Pending,
    Running(f64),
    Done { files: usize },
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct BatchJob {
    pub case_name: String,
    pub progress: BatchProgress,
}

#[derive(Debug, Clone)]
pub enum WorkerEvent {
    Progress {
        case_idx: usize,
        progress: BatchProgress,
    },
    AllDone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthField {
    Topology,
    N,
    ROverX,
    MeanX,
    Seed,
}

impl SynthField {
    pub fn next(self) -> Self {
        match self {
            Self::Topology => Self::N,
            Self::N => Self::ROverX,
            Self::ROverX => Self::MeanX,
            Self::MeanX => Self::Seed,
            Self::Seed => Self::Topology,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Self::Topology => Self::Seed,
            Self::N => Self::Topology,
            Self::ROverX => Self::N,
            Self::MeanX => Self::ROverX,
            Self::Seed => Self::MeanX,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SynthState {
    pub spec: SynthSpec,
    pub field: SynthField,
    pub generated: Option<Network>,
}

impl Default for SynthState {
    fn default() -> Self {
        Self {
            spec: SynthSpec::default(),
            field: SynthField::Topology,
            generated: None,
        }
    }
}

pub struct App {
    pub data_dir: PathBuf,
    pub out_dir: PathBuf,
    pub screen: Screen,
    pub previous_screen: Screen,
    pub cases: Vec<CaseEntry>,
    pub selected: usize,
    pub multi_selected: HashSet<usize>,
    pub inspect: Option<InspectState>,
    pub synth: SynthState,
    pub batch: Vec<BatchJob>,
    pub log: LogBuf,
    pub status: Option<(String, Instant)>,
    pub should_quit: bool,
    pub worker_rx: Option<Receiver<WorkerEvent>>,
    pub matrices_to_export: Vec<MatrixKind>,
    pub scheme: powerio_matrix::matrix::Scheme,
    pub rhs: RhsKind,
}

impl App {
    pub fn new(data_dir: PathBuf, out_dir: PathBuf, log: LogBuf) -> Self {
        Self {
            data_dir,
            out_dir,
            screen: Screen::Browse,
            previous_screen: Screen::Browse,
            cases: Vec::new(),
            selected: 0,
            multi_selected: HashSet::new(),
            inspect: None,
            synth: SynthState::default(),
            batch: Vec::new(),
            log,
            status: None,
            should_quit: false,
            worker_rx: None,
            matrices_to_export: vec![MatrixKind::BPrime, MatrixKind::BDoublePrime],
            scheme: powerio_matrix::matrix::Scheme::default(),
            rhs: RhsKind::Random,
        }
    }

    pub fn refresh_cases(&mut self) {
        // The discovered order is already sorted by path, which groups
        // subfolders; display names keep the relative path and extension so
        // same-stem cases in different formats stay distinguishable.
        self.cases = crate::cases::discover_cases(&self.data_dir, Some(&self.out_dir))
            .into_iter()
            .map(|path| {
                let display_name = path
                    .strip_prefix(&self.data_dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                CaseEntry {
                    path,
                    display_name,
                    parsed: ParseStatus::NotLoaded,
                }
            })
            .collect();
        if self.selected >= self.cases.len() {
            self.selected = self.cases.len().saturating_sub(1);
        }
    }

    pub fn parse_selected(&mut self) {
        if let Some(entry) = self.cases.get_mut(self.selected) {
            if matches!(entry.parsed, ParseStatus::NotLoaded) {
                entry.parsed = match crate::cases::load_network(&entry.path) {
                    Ok(loaded) => {
                        self.log.push_parse_warnings(&entry.path, &loaded.warnings);
                        ParseStatus::Loaded {
                            n_buses: loaded.network.buses.len(),
                            n_branches: loaded.network.branches.len(),
                            base_mva: loaded.network.base_mva,
                        }
                    }
                    Err(e) => ParseStatus::Failed(format!("{e:#}")),
                };
            }
        }
    }

    pub fn open_inspect(&mut self) -> anyhow::Result<()> {
        let Some(entry) = self.cases.get(self.selected) else {
            return Ok(());
        };
        let loaded = crate::cases::load_network(&entry.path)?;
        self.log.push_parse_warnings(&entry.path, &loaded.warnings);
        self.inspect = Some(self.build_inspect(loaded.network)?);
        self.previous_screen = self.screen;
        self.screen = Screen::Inspect;
        Ok(())
    }

    pub fn build_inspect(&self, case: Network) -> powerio_matrix::Result<InspectState> {
        let opts = powerio_matrix::matrix::BuildOptions {
            scheme: self.scheme,
            ..Default::default()
        };
        let view = powerio_matrix::IndexedNetwork::new(&case);
        let mut matrices = BTreeMap::new();
        for &kind in MatrixKind::ALL {
            let mat = powerio_matrix::build_kind(&view, kind, &opts)?;
            let stats = matrix_stats_for_kind(&mat, &view, kind, &opts);
            let sddm = sddm_check(&mat);
            matrices.insert(
                MatrixKindOrd::from_kind(kind),
                MatrixCell {
                    matrix: mat,
                    stats,
                    sddm,
                },
            );
        }
        Ok(InspectState {
            case,
            kind: MatrixKind::BPrime,
            kind_idx: 0,
            matrices,
        })
    }

    pub fn current_matrix(&self) -> Option<&MatrixCell> {
        self.inspect
            .as_ref()
            .and_then(|s| s.matrices.get(&MatrixKindOrd::from_kind(s.kind)))
    }

    pub fn cycle_matrix_kind(&mut self, forward: bool) {
        if let Some(state) = &mut self.inspect {
            let len = MatrixKind::ALL.len();
            state.kind_idx = if forward {
                (state.kind_idx + 1) % len
            } else {
                (state.kind_idx + len - 1) % len
            };
            state.kind = MatrixKind::ALL[state.kind_idx];
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    pub fn batch_targets(&self) -> Vec<usize> {
        if self.multi_selected.is_empty() {
            vec![self.selected]
        } else {
            let mut v: Vec<_> = self.multi_selected.iter().copied().collect();
            v.sort_unstable();
            v
        }
    }

    pub fn drain_worker(&mut self) {
        let mut events = Vec::new();
        if let Some(rx) = &self.worker_rx {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            match ev {
                WorkerEvent::Progress { case_idx, progress } => {
                    if let Some(job) = self.batch.get_mut(case_idx) {
                        job.progress = progress;
                    }
                }
                WorkerEvent::AllDone => {
                    self.set_status("batch complete");
                }
            }
        }
    }

    pub fn topology_label(t: Topology) -> &'static str {
        match t {
            Topology::Tree => "tree",
            Topology::Lattice2D => "lattice 2D",
            Topology::PegaseLike => "pegase",
        }
    }
}
