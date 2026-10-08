//! HVDC lines as fixed bus injections.
//!
//! A balanced calculation has no DC network: an [`Hvdc`] line couples its two
//! AC terminal buses only through the power its converters exchange with
//! them. The calculations therefore carry each in service line as two fixed
//! injections at its stated operating point, in MATPOWER's `dcline`
//! convention (its `toggle_dcline` dummy generators): the from converter
//! withdraws `pf` and injects `qf`, the to converter injects `pt` and `qt`.
//! Every reader stores [`Hvdc`] in that convention, so the rule holds for a
//! MATPOWER `dcline` row and a PSS/E two-terminal DC record alike. `pf - pt`
//! is the power the line and its converters lose.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Diagnostic;
use crate::diagnostics::codes;
use crate::network::{BalancedNetwork, BusId, BusType, Hvdc};

/// How a calculation treats the network's HVDC lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HvdcTreatment {
    /// Each in service line enters the bus balance as its stated terminal
    /// powers: `-pf` and `+qf` at the from bus, `+pt` and `+qt` at the to bus.
    #[default]
    FixedInjection,
    /// Leave every HVDC line out of the bus balance. The power the lines
    /// transfer is absent from the calculation, and a
    /// [`BUILD.HVDC.IGNORED`](codes::BUILD_HVDC_IGNORED) warning states how
    /// much.
    Ignore,
}

/// The injection one HVDC line makes at one of its AC terminal buses.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct HvdcTerminalInjection {
    /// Row of the line in [`BalancedNetwork::hvdc`].
    pub row: usize,
    /// The AC terminal bus.
    pub bus: BusId,
    /// Active power injected into the bus, in the network's power unit (MW,
    /// or per unit for a normalized network). Negative at a from end that
    /// sends power.
    pub p: f64,
    /// Reactive power injected into the bus, same unit.
    pub q: f64,
}

/// Every fixed HVDC injection a network states, with the lines that make
/// none. Build it with [`BalancedNetwork::calc_hvdc_injections`].
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct HvdcInjections {
    /// Two entries per injecting line, its from end then its to end, in
    /// [`BalancedNetwork::hvdc`] row order.
    pub terminals: Vec<HvdcTerminalInjection>,
    /// Rows of the lines whose in service flag is false. They inject nothing.
    pub out_of_service: Vec<usize>,
    /// Rows of in service lines left out because a terminal bus is isolated
    /// or not declared by the network. A converter with no energized AC
    /// terminal cannot exchange power, so neither end injects.
    pub inactive_terminal: Vec<usize>,
    /// Whether the powers are per unit (the network is normalized) rather
    /// than MW and MVAr.
    pub per_unit: bool,
}

impl HvdcInjections {
    /// How many lines inject.
    #[must_use]
    pub fn n_injecting(&self) -> usize {
        self.terminals.len() / 2
    }

    fn sending_ends(&self) -> impl Iterator<Item = &HvdcTerminalInjection> {
        self.terminals.iter().step_by(2)
    }

    fn receiving_ends(&self) -> impl Iterator<Item = &HvdcTerminalInjection> {
        self.terminals.iter().skip(1).step_by(2)
    }

    /// Active power the injecting lines take from their from buses, summed.
    #[must_use]
    pub fn calc_sent_power(&self) -> f64 {
        -self.sending_ends().map(|t| t.p).sum::<f64>()
    }

    /// Active power the injecting lines deliver to their to buses, summed.
    #[must_use]
    pub fn calc_delivered_power(&self) -> f64 {
        self.receiving_ends().map(|t| t.p).sum()
    }

    /// Reactive power the injecting lines put into their terminal buses,
    /// summed over both ends.
    #[must_use]
    pub fn calc_reactive_injection(&self) -> f64 {
        self.terminals.iter().map(|t| t.q).sum()
    }

    /// The diagnostics a calculation reports for these lines under
    /// `treatment`: a remark stating what was injected, or a warning stating
    /// what was left out, plus one warning per in service line whose terminal
    /// bus is inactive. A network with no HVDC line reports nothing.
    #[must_use]
    pub fn to_diagnostics(&self, treatment: HvdcTreatment) -> Vec<Diagnostic> {
        let (p_unit, q_unit) = if self.per_unit {
            ("per unit", "per unit")
        } else {
            ("MW", "MVAr")
        };
        let mut diagnostics = Vec::new();
        let n = self.n_injecting();
        if n > 0 {
            let sent = self.calc_sent_power();
            let delivered = self.calc_delivered_power();
            let lost = amount(sent - delivered);
            let reactive = amount(self.calc_reactive_injection());
            let (sent, delivered) = (amount(sent), amount(delivered));
            diagnostics.push(match treatment {
                HvdcTreatment::FixedInjection => Diagnostic::of(
                    &codes::BUILD_HVDC_FIXED_INJECTION,
                    format!(
                        "{n} in service HVDC line(s) enter the bus balance as fixed injections: \
                         {sent} {p_unit} withdrawn at the from buses, {delivered} {p_unit} \
                         delivered at the to buses ({lost} {p_unit} lost), and {reactive} \
                         {q_unit} reactive injection over both ends"
                    ),
                ),
                HvdcTreatment::Ignore => Diagnostic::of(
                    &codes::BUILD_HVDC_IGNORED,
                    format!(
                        "{n} in service HVDC line(s) were left out of the bus balance by \
                         request; {sent} {p_unit} sent and {delivered} {p_unit} delivered are \
                         absent from the calculation"
                    ),
                ),
            });
        }
        for row in &self.inactive_terminal {
            diagnostics.push(Diagnostic::of(
                &codes::BUILD_HVDC_TERMINAL_INACTIVE,
                format!(
                    "HVDC line row {row} is in service, but a terminal bus is isolated or not \
                     declared; it injects nothing"
                ),
            ));
        }
        diagnostics
    }
}

/// A power total for a message: at most six decimals, trailing zeros
/// dropped, so a sum's rounding noise does not reach the text.
fn amount(value: f64) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

impl Hvdc {
    /// The fixed injections this line's stated operating point makes,
    /// `[from end, to end]` as `(bus, p, q)`: the from converter withdraws
    /// [`pf`](Self::pf) and injects [`qf`](Self::qf), the to converter injects
    /// [`pt`](Self::pt) and [`qt`](Self::qt). The in service flag is not
    /// consulted.
    #[must_use]
    pub fn calc_terminal_injections(&self) -> [(BusId, f64, f64); 2] {
        [(self.from, -self.pf, self.qf), (self.to, self.pt, self.qt)]
    }
}

impl BalancedNetwork {
    /// The fixed bus injections of every in service HVDC line whose two
    /// terminal buses are declared and not isolated, in the convention the
    /// [module](crate::hvdc) states, plus the rows of the lines that inject
    /// nothing. Powers are in the network's unit, so a normalized network
    /// yields per unit.
    #[must_use]
    pub fn calc_hvdc_injections(&self) -> HvdcInjections {
        let mut injections = HvdcInjections {
            per_unit: self.is_normalized(),
            ..HvdcInjections::default()
        };
        if self.hvdc().is_empty() {
            return injections;
        }
        let kinds: HashMap<BusId, BusType> = self.buses().iter().map(|b| (b.id, b.kind)).collect();
        let active = |bus: BusId| {
            kinds
                .get(&bus)
                .is_some_and(|&kind| kind != BusType::Isolated)
        };
        for (row, line) in self.hvdc().iter().enumerate() {
            if !line.in_service {
                injections.out_of_service.push(row);
            } else if !(active(line.from) && active(line.to)) {
                injections.inactive_terminal.push(row);
            } else {
                for (bus, p, q) in line.calc_terminal_injections() {
                    injections
                        .terminals
                        .push(HvdcTerminalInjection { row, bus, p, q });
                }
            }
        }
        injections
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::Bus;

    fn bus(id: usize, kind: BusType) -> Bus {
        Bus::new(BusId(id), kind, 230.0)
    }

    fn line(from: usize, to: usize, pf: f64, pt: f64, qf: f64, qt: f64) -> Hvdc {
        let mut line = Hvdc::new(BusId(from), BusId(to));
        line.pf = pf;
        line.pt = pt;
        line.qf = qf;
        line.qt = qt;
        line
    }

    #[test]
    fn terminal_injections_follow_the_dcline_convention() {
        let injections = line(1, 2, 100.0, 97.0, -40.0, -35.0).calc_terminal_injections();
        assert_eq!(injections[0], (BusId(1), -100.0, -40.0));
        assert_eq!(injections[1], (BusId(2), 97.0, -35.0));
    }

    #[test]
    fn out_of_service_and_inactive_terminals_inject_nothing() {
        let mut net = BalancedNetwork::in_memory(
            "hvdc",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Isolated),
            ],
            Vec::new(),
        );
        let mut off = line(1, 2, 50.0, 49.0, 0.0, 0.0);
        off.in_service = false;
        net.hvdc_mut().extend([
            line(1, 2, 100.0, 97.0, 0.0, 0.0),
            off,
            line(1, 3, 10.0, 10.0, 0.0, 0.0),
            line(1, 9, 10.0, 10.0, 0.0, 0.0),
        ]);
        let injections = net.calc_hvdc_injections();
        assert_eq!(injections.n_injecting(), 1);
        assert_eq!(injections.out_of_service, vec![1]);
        assert_eq!(injections.inactive_terminal, vec![2, 3]);
        assert!((injections.calc_sent_power() - 100.0).abs() < 1e-12);
        assert!((injections.calc_delivered_power() - 97.0).abs() < 1e-12);

        let fixed = injections.to_diagnostics(HvdcTreatment::FixedInjection);
        assert_eq!(fixed.len(), 3);
        assert_eq!(fixed[0].code(), "BUILD.HVDC.FIXED_INJECTION");
        assert!(
            fixed[0].message().contains("100 MW withdrawn"),
            "{}",
            fixed[0].message()
        );
        assert!(
            fixed[0].message().contains("97 MW"),
            "{}",
            fixed[0].message()
        );
        assert_eq!(fixed[1].code(), "BUILD.HVDC.TERMINAL_INACTIVE");
        let ignored = injections.to_diagnostics(HvdcTreatment::Ignore);
        assert_eq!(ignored[0].code(), "BUILD.HVDC.IGNORED");
    }

    #[test]
    fn a_network_without_hvdc_reports_nothing() {
        let net =
            BalancedNetwork::in_memory("empty", 100.0, vec![bus(1, BusType::Ref)], Vec::new());
        let injections = net.calc_hvdc_injections();
        assert_eq!(injections, HvdcInjections::default());
        assert_eq!(injections.to_diagnostics(HvdcTreatment::FixedInjection), []);
    }
}
