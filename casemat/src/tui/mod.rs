//! ratatui interactive TUI.
//!
//! Public entry point: [`run`]. Reachable as the `tui` subcommand of the
//! binary, and as the default when no subcommand is given.

mod app;
mod log_pane;
mod screens;
mod sparsity;
mod theme;

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use anyhow::Context;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};

use app::{App, BatchJob, BatchProgress, Screen, SynthField, WorkerEvent};
use log_pane::LogBuf;

use crate::pipeline::{MatrixKind, Pipeline, RhsKind};
use crate::synth::Topology;

#[derive(Debug, Default)]
pub struct TuiOptions {
    pub data_dir: Option<PathBuf>,
    pub out_dir: Option<PathBuf>,
}

pub fn run(opts: TuiOptions) -> anyhow::Result<()> {
    let log = LogBuf::default();
    let _ = install_tui_tracing(log.clone());

    let data_dir = opts
        .data_dir
        .or_else(|| std::env::current_dir().ok().map(|p| p.join("tests/data")))
        .unwrap_or_else(|| PathBuf::from("."));
    let out_dir = opts
        .out_dir
        .or_else(|| std::env::current_dir().ok().map(|p| p.join("out")))
        .unwrap_or_else(|| PathBuf::from("./out"));

    let mut app = App::new(data_dir, out_dir, log.clone());
    app.refresh_cases();
    app.parse_selected();
    tracing::info!(
        cases = app.cases.len(),
        data = %app.data_dir.display(),
        out = %app.out_dir.display(),
        "TUI ready",
    );

    ratatui::run(|terminal| -> std::io::Result<()> {
        let tick_rate = Duration::from_millis(120);
        let mut last_tick = Instant::now();
        loop {
            terminal.draw(|f| screens::draw(f, &app))?;

            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_default();
            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        if let Err(e) = handle_key(&mut app, key) {
                            tracing::error!(error = %e, "key handler error");
                            app.set_status(format!("error: {e}"));
                        }
                    }
                }
            }
            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
                app.drain_worker();
                if let Some((_, when)) = app.status {
                    if when.elapsed() > Duration::from_secs(4) {
                        app.status = None;
                    }
                }
            }
            if app.should_quit {
                break Ok(());
            }
        }
    })
    .context("TUI run failed")
}

fn install_tui_tracing(buf: LogBuf) -> Result<(), tracing::dispatcher::SetGlobalDefaultError> {
    use tracing_subscriber::fmt;
    use tracing_subscriber::EnvFilter;
    let subscriber = fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(buf)
        .with_ansi(false)
        .without_time()
        .finish();
    tracing::subscriber::set_global_default(subscriber)
}

fn handle_key(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    // Global keys.
    if matches!(key.code, KeyCode::Char('q'))
        || (matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        app.should_quit = true;
        return Ok(());
    }
    if matches!(key.code, KeyCode::Char('?')) {
        app.previous_screen = app.screen;
        app.screen = Screen::Help;
        return Ok(());
    }
    match app.screen {
        Screen::Browse => handle_browse(app, key),
        Screen::Inspect => handle_inspect(app, key),
        Screen::Batch => handle_batch(app, key),
        Screen::Synth => handle_synth(app, key),
        Screen::Help => handle_help(app, key),
    }
}

fn handle_browse(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    match key.code {
        KeyCode::Up => {
            if app.selected > 0 {
                app.selected -= 1;
                app.parse_selected();
            }
        }
        KeyCode::Down => {
            if app.selected + 1 < app.cases.len() {
                app.selected += 1;
                app.parse_selected();
            }
        }
        KeyCode::Char(' ') => {
            if !app.cases.is_empty() {
                if !app.multi_selected.insert(app.selected) {
                    app.multi_selected.remove(&app.selected);
                }
            }
        }
        KeyCode::Char('R') | KeyCode::F(5) => {
            app.refresh_cases();
            app.parse_selected();
            app.set_status("rescanned");
        }
        KeyCode::Enter => {
            if let Err(e) = app.open_inspect() {
                app.set_status(format!("inspect failed: {e}"));
            }
        }
        KeyCode::Char('b') => {
            app.previous_screen = Screen::Browse;
            app.screen = Screen::Batch;
        }
        KeyCode::Char('g') => {
            app.previous_screen = Screen::Browse;
            app.screen = Screen::Synth;
        }
        _ => {}
    }
    Ok(())
}

fn handle_inspect(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::Browse;
            app.inspect = None;
        }
        KeyCode::Tab => app.cycle_matrix_kind(true),
        KeyCode::BackTab => app.cycle_matrix_kind(false),
        KeyCode::Char('s') => {
            app.scheme = match app.scheme {
                crate::matrix::Scheme::Bx => crate::matrix::Scheme::Xb,
                crate::matrix::Scheme::Xb => crate::matrix::Scheme::Bx,
            };
            if let Some(state) = app.inspect.take() {
                let case = state.case;
                let prev_kind = state.kind;
                match app.build_inspect(case) {
                    Ok(mut new_state) => {
                        new_state.kind = prev_kind;
                        new_state.kind_idx = MatrixKind::ALL
                            .iter()
                            .position(|k| *k == prev_kind)
                            .unwrap_or(0);
                        app.inspect = Some(new_state);
                        app.set_status(format!("scheme = {:?}", app.scheme));
                    }
                    Err(e) => app.set_status(format!("rebuild failed: {e}")),
                }
            }
        }
        KeyCode::Char('e') => {
            export_inspect(app)?;
        }
        _ => {}
    }
    Ok(())
}

fn export_inspect(app: &mut App) -> anyhow::Result<()> {
    let state = match &app.inspect {
        Some(s) => s,
        None => return Ok(()),
    };
    let pipeline = Pipeline {
        matrices: vec![state.kind],
        options: crate::matrix::BuildOptions {
            scheme: app.scheme,
            ..Default::default()
        },
        rhs: app.rhs,
        rng_seed: 0xC0FFEE,
        source_file: None,
    };
    let outputs = pipeline.run(&state.case, &app.out_dir)?;
    tracing::info!(
        files = outputs.files.len(),
        out = %app.out_dir.display(),
        case = %outputs.case_name,
        "inspect export"
    );
    app.set_status(format!(
        "wrote {} files for {} → {}",
        outputs.files.len(),
        outputs.case_name,
        app.out_dir.display()
    ));
    Ok(())
}

fn handle_batch(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.screen = app.previous_screen;
        }
        KeyCode::Char('m') => {
            cycle_matrices(app);
        }
        KeyCode::Char('r') => {
            app.rhs = match app.rhs {
                RhsKind::None => RhsKind::Random,
                RhsKind::Random => RhsKind::Injection,
                RhsKind::Injection => RhsKind::None,
            };
            app.set_status(format!("rhs = {:?}", app.rhs));
        }
        KeyCode::Char('e') => {
            spawn_batch(app);
        }
        _ => {}
    }
    Ok(())
}

fn cycle_matrices(app: &mut App) {
    let presets: &[&[MatrixKind]] = &[
        &[MatrixKind::BPrime],
        &[MatrixKind::BPrime, MatrixKind::BDoublePrime],
        &[
            MatrixKind::BPrime,
            MatrixKind::BDoublePrime,
            MatrixKind::YbusB,
        ],
        &[
            MatrixKind::BPrime,
            MatrixKind::BDoublePrime,
            MatrixKind::YbusG,
            MatrixKind::YbusB,
        ],
        MatrixKind::ALL,
    ];
    let cur_idx = presets
        .iter()
        .position(|p| p.len() == app.matrices_to_export.len() && p.iter().zip(&app.matrices_to_export).all(|(a, b)| a == b))
        .unwrap_or(0);
    let next = presets[(cur_idx + 1) % presets.len()];
    app.matrices_to_export = next.to_vec();
    app.set_status(format!(
        "matrices = {}",
        next.iter().map(|k| k.slug()).collect::<Vec<_>>().join(",")
    ));
}

fn spawn_batch(app: &mut App) {
    let targets = app.batch_targets();
    if targets.is_empty() {
        app.set_status("nothing to export");
        return;
    }
    let paths: Vec<PathBuf> = targets
        .iter()
        .filter_map(|i| app.cases.get(*i).map(|c| c.path.clone()))
        .collect();
    app.batch = paths
        .iter()
        .map(|p| BatchJob {
            case_name: p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string(),
            progress: BatchProgress::Pending,
        })
        .collect();
    let pipeline = Pipeline {
        matrices: app.matrices_to_export.clone(),
        options: crate::matrix::BuildOptions {
            scheme: app.scheme,
            ..Default::default()
        },
        rhs: app.rhs,
        rng_seed: 0xC0FFEE,
        source_file: None,
    };
    let out_dir = app.out_dir.clone();
    let (tx, rx) = channel();
    app.worker_rx = Some(rx);
    let log = app.log.clone();

    std::thread::spawn(move || {
        for (i, path) in paths.iter().enumerate() {
            let _ = tx.send(WorkerEvent::Progress {
                case_idx: i,
                progress: BatchProgress::Running(0.05),
            });
            let parsed = match crate::parse_matpower_file(path) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(WorkerEvent::Progress {
                        case_idx: i,
                        progress: BatchProgress::Failed(format!("parse: {e}")),
                    });
                    log.push(format!("ERROR parse {}: {e}", path.display()));
                    continue;
                }
            };
            let _ = tx.send(WorkerEvent::Progress {
                case_idx: i,
                progress: BatchProgress::Running(0.4),
            });
            let mut p = pipeline.clone();
            p.source_file = Some(path.clone());
            match p.run(&parsed, &out_dir) {
                Ok(out) => {
                    log.push(format!(
                        "INFO  exported {} ({} files)",
                        out.case_name,
                        out.files.len()
                    ));
                    let _ = tx.send(WorkerEvent::Progress {
                        case_idx: i,
                        progress: BatchProgress::Done {
                            files: out.files.len(),
                        },
                    });
                }
                Err(e) => {
                    log.push(format!("ERROR export {}: {e}", path.display()));
                    let _ = tx.send(WorkerEvent::Progress {
                        case_idx: i,
                        progress: BatchProgress::Failed(e.to_string()),
                    });
                }
            }
        }
        let _ = tx.send(WorkerEvent::AllDone);
    });
    app.set_status(format!("running {} jobs", app.batch.len()));
}

fn handle_synth(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.screen = app.previous_screen;
        }
        KeyCode::Up => app.synth.field = app.synth.field.prev(),
        KeyCode::Down => app.synth.field = app.synth.field.next(),
        KeyCode::Left => synth_tweak(app, false),
        KeyCode::Right => synth_tweak(app, true),
        KeyCode::Char('g') => {
            let case = crate::synth::generate(&app.synth.spec);
            app.synth.generated = Some(case);
            app.set_status("regenerated synthetic case");
        }
        KeyCode::Char('e') => {
            if let Some(case) = &app.synth.generated {
                let pipeline = Pipeline {
                    matrices: app.matrices_to_export.clone(),
                    options: crate::matrix::BuildOptions {
                        scheme: app.scheme,
                        ..Default::default()
                    },
                    rhs: app.rhs,
                    rng_seed: app.synth.spec.seed,
                    source_file: None,
                };
                match pipeline.run(case, &app.out_dir) {
                    Ok(out) => {
                        app.set_status(format!(
                            "wrote {} files for {} → {}",
                            out.files.len(),
                            out.case_name,
                            app.out_dir.display()
                        ));
                    }
                    Err(e) => app.set_status(format!("export failed: {e}")),
                }
            } else {
                app.set_status("press [g] to generate first");
            }
        }
        _ => {}
    }
    Ok(())
}

fn synth_tweak(app: &mut App, increase: bool) {
    let s = &mut app.synth.spec;
    let direction = if increase { 1.0 } else { -1.0 };
    match app.synth.field {
        SynthField::Topology => {
            s.topology = match (s.topology, increase) {
                (Topology::Tree, true) => Topology::Lattice2D,
                (Topology::Lattice2D, true) => Topology::PegaseLike,
                (Topology::PegaseLike, true) => Topology::Tree,
                (Topology::Tree, false) => Topology::PegaseLike,
                (Topology::Lattice2D, false) => Topology::Tree,
                (Topology::PegaseLike, false) => Topology::Lattice2D,
            };
        }
        SynthField::N => {
            let step = (s.n as f64 * 0.25).round() as usize;
            s.n = if increase {
                s.n.saturating_add(step.max(8))
            } else {
                s.n.saturating_sub(step.max(8)).max(2)
            };
        }
        SynthField::ROverX => {
            s.r_over_x = (s.r_over_x + direction * 0.05).clamp(0.0, 5.0);
        }
        SynthField::MeanX => {
            let factor: f64 = if increase { 1.2 } else { 1.0 / 1.2 };
            s.mean_x = (s.mean_x * factor).clamp(1e-4, 1.0);
        }
        SynthField::Seed => {
            s.seed = s.seed.wrapping_add(if increase { 1 } else { u64::MAX });
        }
    }
}

fn handle_help(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
        app.screen = app.previous_screen;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn make_app() -> App {
        let log = LogBuf::default();
        App::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data"),
            std::env::temp_dir().join("casemat-tui-test"),
            log,
        )
    }

    fn buffer_text(buf: &Buffer) -> String {
        let area = buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(area.x + x, area.y + y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn smoke_browse_renders() {
        let mut app = make_app();
        app.refresh_cases();
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| screens::draw(f, &app)).unwrap();
        let s = buffer_text(terminal.backend().buffer());
        assert!(s.contains("Browse"), "expected screen header to contain Browse:\n{s}");
        assert!(s.contains("Cases"), "expected case panel title:\n{s}");
    }

    #[test]
    fn inspect_render_after_open() {
        let mut app = make_app();
        app.refresh_cases();
        if !app.cases.is_empty() {
            app.selected = 0;
            app.open_inspect().unwrap();
            let backend = TestBackend::new(160, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| screens::draw(f, &app)).unwrap();
            let s = buffer_text(terminal.backend().buffer());
            assert!(s.contains("Inspect"), "missing Inspect header:\n{s}");
            assert!(s.contains("Stats"), "missing Stats panel:\n{s}");
            assert!(s.contains("Sparsity"), "missing Sparsity panel:\n{s}");
        }
    }

    #[test]
    fn matrix_kind_cycles() {
        let mut app = make_app();
        app.refresh_cases();
        if !app.cases.is_empty() {
            app.selected = 0;
            app.open_inspect().unwrap();
            let initial = app.inspect.as_ref().unwrap().kind;
            app.cycle_matrix_kind(true);
            let next = app.inspect.as_ref().unwrap().kind;
            assert_ne!(initial, next);
        }
    }
}
