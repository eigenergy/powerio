//! Per-screen drawing.
//!
//! Each screen is a free function `draw_*(app, frame, area)`. Key handling
//! lives in `mod.rs::handle_key` because input dispatch is small and
//! benefits from being in one place.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap};

use super::app::{App, BatchJob, BatchProgress, ParseStatus, Screen, SynthField};
use super::sparsity::{Sparsity, legend_lines};
use super::theme::{T, bad, border, dim, good, highlight, title, warn};
use crate::pipeline::MatrixKind;

pub fn draw(frame: &mut Frame, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(7),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_header(frame, app, layout[0]);
    match app.screen {
        Screen::Browse => draw_browse(frame, app, layout[1]),
        Screen::Inspect => draw_inspect(frame, app, layout[1]),
        Screen::Batch => draw_batch(frame, app, layout[1]),
        Screen::Synth => draw_synth(frame, app, layout[1]),
        Screen::Help => draw_help(frame, app, layout[1]),
    }
    draw_log(frame, app, layout[2]);
    draw_footer(frame, app, layout[3]);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let title_text = format!(" mpower-bmat · {} ", app.screen.label());
    let line = Line::from(vec![
        Span::styled(title_text, title()),
        Span::raw("  "),
        Span::styled(
            format!("data={}  out={}", app.data_dir.display(), app.out_dir.display()),
            dim(),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = match app.screen {
        Screen::Browse => vec![
            kc("↑↓"), Span::raw(" select  "),
            kc("Space"), Span::raw(" toggle  "),
            kc("Enter"), Span::raw(" inspect  "),
            kc("b"), Span::raw(" batch  "),
            kc("g"), Span::raw(" synth  "),
            kc("?"), Span::raw(" help  "),
            kc("q"), Span::raw(" quit"),
        ],
        Screen::Inspect => vec![
            kc("Tab"), Span::raw(" matrix  "),
            kc("e"), Span::raw(" export  "),
            kc("s"), Span::raw(" toggle scheme  "),
            kc("Esc"), Span::raw(" back  "),
            kc("q"), Span::raw(" quit"),
        ],
        Screen::Batch => vec![
            kc("e"), Span::raw(" run  "),
            kc("m"), Span::raw(" cycle matrices  "),
            kc("r"), Span::raw(" cycle rhs  "),
            kc("Esc"), Span::raw(" back  "),
            kc("q"), Span::raw(" quit"),
        ],
        Screen::Synth => vec![
            kc("↑↓"), Span::raw(" field  "),
            kc("←→"), Span::raw(" tweak  "),
            kc("g"), Span::raw(" generate  "),
            kc("e"), Span::raw(" export  "),
            kc("Esc"), Span::raw(" back  "),
            kc("q"), Span::raw(" quit"),
        ],
        Screen::Help => vec![kc("Esc"), Span::raw(" back  "), kc("q"), Span::raw(" quit")],
    };
    if let Some((msg, _)) = &app.status {
        spans.push(Span::raw("    "));
        spans.push(Span::styled(format!("» {msg}"), Style::default().fg(T.accent)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn kc(label: &str) -> Span<'_> {
    Span::styled(format!("[{label}]"), Style::default().fg(T.accent_alt).add_modifier(Modifier::BOLD))
}

fn draw_browse(frame: &mut Frame, app: &App, area: Rect) {
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let items: Vec<ListItem> = app
        .cases
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let prefix = if app.multi_selected.contains(&i) { "[x] " } else { "[ ] " };
            let parsed_marker = match &c.parsed {
                ParseStatus::Loaded { n_buses, n_branches, .. } => {
                    format!("  · {n_buses} buses / {n_branches} branches")
                }
                ParseStatus::Failed(_) => "  · parse failed".to_string(),
                ParseStatus::NotLoaded => String::new(),
            };
            let line = Line::from(vec![
                Span::raw(prefix),
                Span::styled(c.display_name.clone(), Style::default().fg(T.fg)),
                Span::styled(parsed_marker, dim()),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Cases ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border()),
        )
        .highlight_style(highlight());

    let mut state = ListState::default();
    state.select(if app.cases.is_empty() { None } else { Some(app.selected) });
    frame.render_stateful_widget(list, split[0], &mut state);

    draw_browse_detail(frame, app, split[1]);
}

fn draw_browse_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines: Vec<Line> = if let Some(entry) = app.cases.get(app.selected) {
        let mut v = vec![
            Line::from(vec![
                Span::styled("path:    ", dim()),
                Span::raw(entry.path.display().to_string()),
            ]),
            Line::from(vec![
                Span::styled("name:    ", dim()),
                Span::raw(entry.display_name.clone()),
            ]),
        ];
        match &entry.parsed {
            ParseStatus::Loaded { n_buses, n_branches, base_mva } => {
                v.push(Line::from(vec![
                    Span::styled("buses:   ", dim()),
                    Span::raw(n_buses.to_string()),
                ]));
                v.push(Line::from(vec![
                    Span::styled("branches:", dim()),
                    Span::raw(format!(" {n_branches}")),
                ]));
                v.push(Line::from(vec![
                    Span::styled("baseMVA: ", dim()),
                    Span::raw(format!("{base_mva}")),
                ]));
                v.push(Line::raw(""));
                v.push(Line::from(Span::styled(
                    "press Enter to inspect, Space to add to batch",
                    dim(),
                )));
            }
            ParseStatus::Failed(msg) => {
                v.push(Line::from(Span::styled(
                    format!("parse failed: {msg}"),
                    bad(),
                )));
            }
            ParseStatus::NotLoaded => {
                v.push(Line::from(Span::styled(
                    "not yet parsed (press Enter to inspect)",
                    dim(),
                )));
            }
        }
        v
    } else {
        vec![Line::from(Span::styled(
            "No `.m` cases found in data directory.",
            warn(),
        ))]
    };

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_inspect(frame: &mut Frame, app: &App, area: Rect) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)])
        .split(area);

    // Header: case info + matrix kind selector
    let kind_strip = MatrixKind::ALL
        .iter()
        .map(|k| {
            let active = app
                .inspect
                .as_ref()
                .map(|s| s.kind == *k)
                .unwrap_or(false);
            let style = if active { highlight() } else { dim() };
            Span::styled(format!(" {} ", k.label()), style)
        })
        .collect::<Vec<_>>();

    let mut header_lines = Vec::new();
    if let Some(state) = &app.inspect {
        header_lines.push(Line::from(vec![
            Span::styled("case: ", dim()),
            Span::styled(state.case.name.clone(), title()),
            Span::raw("    "),
            Span::styled("buses: ", dim()),
            Span::raw(state.case.n().to_string()),
            Span::raw("   "),
            Span::styled("branches: ", dim()),
            Span::raw(state.case.branches.len().to_string()),
            Span::raw("   "),
            Span::styled("baseMVA: ", dim()),
            Span::raw(format!("{:.0}", state.case.base_mva)),
            Span::raw("   "),
            Span::styled("scheme: ", dim()),
            Span::raw(format!("{:?}", app.scheme)),
        ]));
    }
    header_lines.push(Line::raw(""));
    header_lines.push(Line::from(kind_strip));

    let header = Paragraph::new(header_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border())
            .title(" Inspect "),
    );
    frame.render_widget(header, split[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(split[1]);

    draw_stats_panel(frame, app, body[0]);
    draw_sparsity_panel(frame, app, body[1]);
}

fn draw_stats_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border())
        .title(" Stats ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let cell = match app.current_matrix() {
        Some(c) => c,
        None => return,
    };
    let s = &cell.stats;
    let sddm_span = if cell.sddm {
        Span::styled("yes", good())
    } else {
        Span::styled("no", bad())
    };
    let m_sign_span = if s.m_matrix_sign {
        Span::styled("yes", good())
    } else {
        Span::styled("no", warn())
    };
    let dd_span = if s.min_dd_margin >= -1e-12 {
        Span::styled(format!("{:+.3e}", s.min_dd_margin), good())
    } else {
        Span::styled(format!("{:+.3e}", s.min_dd_margin), warn())
    };
    let lines = vec![
        kv("n", s.n.to_string()),
        kv("nnz", s.nnz.to_string()),
        kv_styled("min diag", format!("{:+.3e}", s.min_diag), good()),
        kv("max diag", format!("{:+.3e}", s.max_diag)),
        kv_inline("DD margin", dd_span),
        kv_inline("M-matrix sign", m_sign_span),
        kv_inline("SDDM", sddm_span),
        kv("‖A‖_F", format!("{:.3e}", s.frobenius_norm)),
        Line::raw(""),
        Line::from(Span::styled(
            "Tab cycles matrices · e exports · s toggles scheme",
            dim(),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn kv(k: &str, v: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{k:<14}"), dim()),
        Span::raw(v),
    ])
}

fn kv_styled(k: &str, v: String, st: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{k:<14}"), dim()),
        Span::styled(v, st),
    ])
}

fn kv_inline<'a>(k: &str, span: Span<'a>) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{k:<14}"), dim()),
        span,
    ])
}

fn draw_sparsity_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border())
        .title(" Sparsity ");
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(block.inner(area));
    frame.render_widget(block, area);

    if let Some(cell) = app.current_matrix() {
        frame.render_widget(Sparsity::new(&cell.matrix), split[0]);
    }
    frame.render_widget(Paragraph::new(legend_lines()), split[1]);
}

fn draw_batch(frame: &mut Frame, app: &App, area: Rect) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(0)])
        .split(area);

    let matrices = app
        .matrices_to_export
        .iter()
        .map(|m| m.slug())
        .collect::<Vec<_>>()
        .join(", ");
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("output dir: ", dim()),
            Span::raw(app.out_dir.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("matrices:   ", dim()),
            Span::raw(matrices),
            Span::raw("   "),
            Span::styled("rhs: ", dim()),
            Span::raw(format!("{:?}", app.rhs)),
            Span::raw("   "),
            Span::styled("scheme: ", dim()),
            Span::raw(format!("{:?}", app.scheme)),
        ]),
        Line::from(vec![
            Span::styled("queue:      ", dim()),
            Span::raw(format!(
                "{} cases (selected via Space in Browse, or current row)",
                app.batch_targets().len()
            )),
        ]),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border())
            .title(" Batch export "),
    );
    frame.render_widget(header, split[0]);

    let body = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border())
        .title(" Progress ");
    let inner = body.inner(split[1]);
    frame.render_widget(body, split[1]);

    if app.batch.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "press [e] to begin export",
                dim(),
            ))),
            inner,
        );
        return;
    }

    let row_h = 2u16;
    for (i, job) in app.batch.iter().enumerate() {
        let y = inner.y + i as u16 * row_h;
        if y + row_h > inner.y + inner.height {
            break;
        }
        let row = Rect::new(inner.x, y, inner.width, row_h);
        draw_batch_row(frame, job, row);
    }
}

fn draw_batch_row(frame: &mut Frame, job: &BatchJob, row: Rect) {
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(0)])
        .split(row);
    let label = Paragraph::new(Line::from(vec![
        Span::styled(format!(" {} ", job.case_name), Style::default().fg(T.fg)),
    ]));
    frame.render_widget(label, split[0]);
    let (ratio, label_text, color) = match &job.progress {
        BatchProgress::Pending => (0.0, "queued".to_string(), T.fg_dim),
        &BatchProgress::Running(f) => (f, format!("{:>3}%", (f * 100.0) as i32), T.accent),
        BatchProgress::Done { files } => (1.0, format!("{files} files"), T.good),
        BatchProgress::Failed(msg) => (1.0, format!("FAIL — {msg}"), T.bad),
    };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(color))
        .ratio(ratio)
        .label(label_text);
    frame.render_widget(gauge, split[1]);
}

fn draw_synth(frame: &mut Frame, app: &App, area: Rect) {
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let mut lines = Vec::new();
    let s = &app.synth.spec;
    lines.push(synth_field_line(
        "topology",
        App::topology_label(s.topology).to_string(),
        app.synth.field == SynthField::Topology,
    ));
    lines.push(synth_field_line(
        "n",
        s.n.to_string(),
        app.synth.field == SynthField::N,
    ));
    lines.push(synth_field_line(
        "r/x",
        format!("{:.3}", s.r_over_x),
        app.synth.field == SynthField::ROverX,
    ));
    lines.push(synth_field_line(
        "mean x",
        format!("{:.3}", s.mean_x),
        app.synth.field == SynthField::MeanX,
    ));
    lines.push(synth_field_line(
        "seed",
        format!("0x{:X}", s.seed),
        app.synth.field == SynthField::Seed,
    ));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "g generates · e exports to out/",
        dim(),
    )));

    let form = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border())
            .title(" Synth "),
    );
    frame.render_widget(form, split[0]);

    let preview_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border())
        .title(" Preview ");
    if let Some(case) = &app.synth.generated {
        if let Ok(b) = crate::build_bprime(case, &Default::default()) {
            let inner = preview_block.inner(split[1]);
            frame.render_widget(preview_block, split[1]);
            frame.render_widget(Sparsity::new(&b), inner);
        } else {
            frame.render_widget(
                Paragraph::new("(failed to build B')").block(preview_block),
                split[1],
            );
        }
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "press [g] to generate",
                dim(),
            )))
            .block(preview_block),
            split[1],
        );
    }
}

fn synth_field_line(label: &str, value: String, focused: bool) -> Line<'static> {
    let marker = if focused { "▶ " } else { "  " };
    let style = if focused { highlight() } else { Style::default() };
    Line::from(vec![
        Span::styled(marker, Style::default().fg(T.accent_alt)),
        Span::styled(format!("{label:<10}"), dim()),
        Span::styled(value, style),
    ])
}

fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let snap = app.log.snapshot();
    let visible = snap
        .iter()
        .rev()
        .take((area.height.saturating_sub(2)) as usize)
        .cloned()
        .collect::<Vec<_>>();
    let lines: Vec<Line> = visible
        .into_iter()
        .rev()
        .map(|s| {
            let style = if s.contains("ERROR") {
                bad()
            } else if s.contains("WARN") {
                warn()
            } else {
                Style::default().fg(T.fg)
            };
            Line::from(Span::styled(s, style))
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border())
        .title(" Log ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_help(frame: &mut Frame, _app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border())
        .title(" Help ")
        .title_alignment(Alignment::Center);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(Span::styled("mpower-bmat — TUI cheatsheet", title())),
        Line::raw(""),
        Line::raw("Browse"),
        Line::raw("  ↑/↓        move selection"),
        Line::raw("  Space      add/remove from batch queue"),
        Line::raw("  Enter      open Inspect for current case"),
        Line::raw("  b          enter Batch screen"),
        Line::raw("  g          enter Synthetic generator"),
        Line::raw("  R          re-scan data directory"),
        Line::raw(""),
        Line::raw("Inspect"),
        Line::raw("  Tab/Shift-Tab cycle matrix kind (B', B'', G, -B, LACPF)"),
        Line::raw("  s          toggle BX ↔ XB scheme (rebuilds matrices)"),
        Line::raw("  e          export current matrix to out_dir"),
        Line::raw("  Esc        back to Browse"),
        Line::raw(""),
        Line::raw("Batch"),
        Line::raw("  m          cycle which matrices to emit"),
        Line::raw("  r          cycle RHS strategy (none/random/injection)"),
        Line::raw("  e          start export"),
        Line::raw(""),
        Line::raw("Synth"),
        Line::raw("  ↑/↓        select field"),
        Line::raw("  ←/→        decrement/increment field"),
        Line::raw("  g          (re)generate the case"),
        Line::raw("  e          export to out_dir"),
        Line::raw(""),
        Line::raw("Global"),
        Line::raw("  ?          help (this screen)"),
        Line::raw("  q          quit"),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}
