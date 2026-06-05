//! Sparsity preview using Unicode block characters.
//!
//! Downsamples an `n × n` sparse matrix into a `H × W` cell grid where
//! each cell renders as a half/quarter block proportional to the local
//! nonzero density. Cell intensity uses a 4-step ramp:
//!
//! ```text
//!     ' '   .   ░   ▒   ▓   █
//! ```
//!
//! Negative values render with one color (the M-matrix off-diagonal pattern
//! we expect for FDPF B'), positive with another, zero is blank.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use sprs::CsMat;

use super::theme::T;

const RAMP: [char; 6] = [' ', '·', '░', '▒', '▓', '█'];

pub struct Sparsity<'a> {
    matrix: &'a CsMat<f64>,
}

impl<'a> Sparsity<'a> {
    pub fn new(matrix: &'a CsMat<f64>) -> Self {
        Self { matrix }
    }
}

impl Widget for Sparsity<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let cells_h = area.height as usize;
        let cells_w = area.width as usize;
        let inner = area;
        if cells_h == 0 || cells_w == 0 {
            return;
        }

        let n = self.matrix.rows();
        let m = self.matrix.cols();
        if n == 0 || m == 0 {
            return;
        }

        // Bin counts, separated by sign.
        let mut pos = vec![0u32; cells_h * cells_w];
        let mut neg = vec![0u32; cells_h * cells_w];

        let row_scale = cells_h as f64 / n as f64;
        let col_scale = cells_w as f64 / m as f64;

        for (&v, (i, j)) in self.matrix.iter() {
            if v == 0.0 {
                continue;
            }
            let r = ((i as f64) * row_scale).floor() as usize;
            let c = ((j as f64) * col_scale).floor() as usize;
            let idx = r.min(cells_h - 1) * cells_w + c.min(cells_w - 1);
            if v < 0.0 {
                neg[idx] += 1;
            } else {
                pos[idx] += 1;
            }
        }

        // Cell capacity: how many entries fall in one cell at full density.
        let cell_cap_rows = (n as f64 / cells_h as f64).ceil().max(1.0);
        let cell_cap_cols = (m as f64 / cells_w as f64).ceil().max(1.0);
        let cap = (cell_cap_rows * cell_cap_cols).max(1.0);

        for r in 0..cells_h {
            for c in 0..cells_w {
                let idx = r * cells_w + c;
                let p = pos[idx];
                let n_ = neg[idx];
                let total = p + n_;
                if total == 0 {
                    continue;
                }
                let density = (total as f64 / cap).clamp(0.0, 1.0);
                let level = (density * (RAMP.len() - 1) as f64).ceil() as usize;
                let glyph = RAMP[level.min(RAMP.len() - 1)];
                let color = if r == c {
                    // Diagonal — accent
                    T.accent
                } else if n_ > p {
                    Color::LightBlue
                } else {
                    Color::LightRed
                };
                buf.set_string(
                    inner.x + c as u16,
                    inner.y + r as u16,
                    glyph.to_string(),
                    Style::default().fg(color),
                );
            }
        }
    }
}

/// Two line legend strip for the bottom of the sparsity preview pane.
pub fn legend_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("·░▒▓█", Style::default().fg(Color::LightBlue)),
            Span::raw(" negative   "),
            Span::styled("·░▒▓█", Style::default().fg(Color::LightRed)),
            Span::raw(" positive   "),
            Span::styled("·░▒▓█", Style::default().fg(T.accent)),
            Span::raw(" diagonal"),
        ]),
    ]
}
