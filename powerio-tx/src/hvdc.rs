//! HVDC lines as fixed bus injections.
//!
//! A balanced calculation has no DC network: an [`Hvdc`] line couples its two
//! AC terminal buses only through the power its converters exchange with
//! them. The calculations therefore carry each in service line as two fixed
//! injections at its stated operating point.
//!
//! The calculation interface is the bus-to-device terminal view,
//! [`Hvdc::calc_bus_to_device_powers`]: at each end, active and reactive
//! power are positive from that end's AC bus into the HVDC system (the two
//! converters and the DC line taken as one device), the direction AC branch
//! terminal flows use. The two active terminal powers sum to the power the
//! line and its converters lose, and the injection into each bus is the
//! negative of that end's terminal power, negated once in
//! [`Hvdc::calc_terminal_injections`].
//!
//! The stored fields [`pf`](Hvdc::pf), [`pt`](Hvdc::pt), [`qf`](Hvdc::qf), and
//! [`qt`](Hvdc::qt) keep MATPOWER's `dcline` signs throughout 0.11.x, whatever
//! the source format: `pf` is positive from the from bus into the line, while
//! `pt`, `qf`, and `qt` are positive from the line into their bus. The
//! terminal view converts them once: `p_from = pf`, `q_from = -qf`,
//! `p_to = -pt`, `q_to = -qt`. The injections match MATPOWER's
//! `toggle_dcline` dummy generators.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Diagnostic;
use crate::diagnostics::codes;
use crate::network::{BalancedNetwork, BusId, BusType, Hvdc};

/// How a calculation treats the network's HVDC lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HvdcTreatment {
    /// Each in service line enters the bus balance as a fixed injection: the
    /// negative of its bus-to-device terminal powers, which in the stored
    /// MATPOWER signs is `-pf` and `+qf` at the from bus and `+pt` and `+qt`
    /// at the to bus.
    #[default]
    FixedInjection,
    /// Leave every HVDC line out of the bus balance. The power the lines
    /// transfer is absent from the calculation, and a `BUILD.HVDC.IGNORED`
    /// warning states how much.
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

/// The bus-to-device terminal powers of one HVDC line: at each end, active
/// and reactive power positive from that end's AC bus into the HVDC system
/// (both converters and the DC line taken as one device). Build it with
/// [`Hvdc::calc_bus_to_device_powers`].
///
/// The from and to ends name the line's orientation, not which end sends: a
/// line transferring power from its to end has a negative `p_from` and a
/// positive `p_to`. Powers are in the network's unit, MW and MVAr, or per
/// unit for a normalized network.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct HvdcTerminalPowers {
    /// Active power from the from bus into the HVDC system.
    pub p_from: f64,
    /// Reactive power from the from bus into the HVDC system; positive when
    /// the from converter absorbs reactive power.
    pub q_from: f64,
    /// Active power from the to bus into the HVDC system; negative when the
    /// line delivers power to the to bus.
    pub p_to: f64,
    /// Reactive power from the to bus into the HVDC system; positive when the
    /// to converter absorbs reactive power.
    pub q_to: f64,
}

impl HvdcTerminalPowers {
    /// The active power the line and its converters lose, `p_from + p_to`:
    /// what enters the HVDC system at both ends together.
    #[must_use]
    pub fn calc_active_loss(&self) -> f64 {
        self.p_from + self.p_to
    }
}

impl Hvdc {
    /// The line's stated operating point as bus-to-device terminal powers,
    /// the convention the [module](crate::hvdc) states. The stored fields
    /// keep MATPOWER's `dcline` signs in 0.11.x, so this is the one place
    /// they convert: `p_from = pf`, `q_from = -qf`, `p_to = -pt`,
    /// `q_to = -qt`. The in service flag is not consulted.
    ///
    /// A line sending 100 MW from its from end and delivering 97 MW at its
    /// to end, with converters absorbing 30 and 25 MVAr, stores `pf = 100`,
    /// `pt = 97`, `qf = -30`, `qt = -25` and has terminal powers
    /// `(p_from, q_from) = (100, 30)` and `(p_to, q_to) = (-97, 25)`, a 3 MW
    /// loss.
    #[must_use]
    pub fn calc_bus_to_device_powers(&self) -> HvdcTerminalPowers {
        HvdcTerminalPowers {
            p_from: self.pf,
            q_from: -self.qf,
            p_to: -self.pt,
            q_to: -self.qt,
        }
    }

    /// The fixed injections this line's stated operating point makes,
    /// `[from end, to end]` as `(bus, p, q)`: at each end the negative of
    /// the [bus-to-device terminal powers](Self::calc_bus_to_device_powers).
    /// In the stored MATPOWER signs the from converter withdraws
    /// [`pf`](Self::pf) and injects [`qf`](Self::qf), and the to converter
    /// injects [`pt`](Self::pt) and [`qt`](Self::qt). The in service flag is
    /// not consulted.
    #[must_use]
    pub fn calc_terminal_injections(&self) -> [(BusId, f64, f64); 2] {
        let terminal = self.calc_bus_to_device_powers();
        [
            (self.from, -terminal.p_from, -terminal.q_from),
            (self.to, -terminal.p_to, -terminal.q_to),
        ]
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

    /// Each bus injection is exactly the negative of its terminal power.
    fn assert_injections_negate_terminals(line: &Hvdc) {
        let terminal = line.calc_bus_to_device_powers();
        let [from, to] = line.calc_terminal_injections();
        assert_eq!(from, (line.from, -terminal.p_from, -terminal.q_from));
        assert_eq!(to, (line.to, -terminal.p_to, -terminal.q_to));
    }

    #[test]
    fn forward_transfer_terminal_powers_point_from_each_bus_into_the_line() {
        // 100 MW sent, 97 MW delivered, converters absorbing 30 and 25 MVAr:
        // MATPOWER stores the delivered power and both reactive powers as
        // injections into their buses.
        let forward = line(1, 2, 100.0, 97.0, -30.0, -25.0);
        let terminal = forward.calc_bus_to_device_powers();
        assert_eq!((terminal.p_from, terminal.q_from), (100.0, 30.0));
        assert_eq!((terminal.p_to, terminal.q_to), (-97.0, 25.0));
        assert!((terminal.calc_active_loss() - 3.0).abs() < 1e-12);
        let [from, to] = forward.calc_terminal_injections();
        assert_eq!(from, (BusId(1), -100.0, -30.0));
        assert_eq!(to, (BusId(2), 97.0, -25.0));
        assert_injections_negate_terminals(&forward);
    }

    #[test]
    fn reverse_transfer_keeps_the_ends_and_a_positive_loss() {
        // The to end sends 100 MW and the from end receives 97 MW, with the
        // receiving converter absorbing 25 MVAr and the sending one 30: the
        // ends keep their identities and the terminal values change sign.
        let reverse = line(1, 2, -97.0, -100.0, -25.0, -30.0);
        let terminal = reverse.calc_bus_to_device_powers();
        assert_eq!((terminal.p_from, terminal.q_from), (-97.0, 25.0));
        assert_eq!((terminal.p_to, terminal.q_to), (100.0, 30.0));
        assert!((terminal.calc_active_loss() - 3.0).abs() < 1e-12);
        let [from, to] = reverse.calc_terminal_injections();
        assert_eq!(from, (BusId(1), 97.0, -25.0));
        assert_eq!(to, (BusId(2), -100.0, -30.0));
        assert_injections_negate_terminals(&reverse);
    }

    #[test]
    fn reactive_absorption_and_injection_at_either_end() {
        // The from converter injects 10 MVAr into its bus and the to
        // converter absorbs 5 MVAr from its bus.
        let mixed = line(1, 2, 50.0, 49.0, 10.0, -5.0);
        let terminal = mixed.calc_bus_to_device_powers();
        assert_eq!((terminal.q_from, terminal.q_to), (-10.0, 5.0));
        let [from, to] = mixed.calc_terminal_injections();
        assert_eq!((from.2, to.2), (10.0, -5.0));
        assert_injections_negate_terminals(&mixed);

        // The opposite: the from converter absorbs 10 MVAr and the to
        // converter injects 5 MVAr.
        let opposite = line(1, 2, 50.0, 49.0, -10.0, 5.0);
        let terminal = opposite.calc_bus_to_device_powers();
        assert_eq!((terminal.q_from, terminal.q_to), (10.0, -5.0));
        let [from, to] = opposite.calc_terminal_injections();
        assert_eq!((from.2, to.2), (-10.0, 5.0));
        assert_injections_negate_terminals(&opposite);

        // Both ends injecting, and both absorbing, at once.
        for (qf, qt) in [(12.0, 7.0), (-12.0, -7.0)] {
            let both = line(1, 2, 50.0, 49.0, qf, qt);
            let terminal = both.calc_bus_to_device_powers();
            assert_eq!((terminal.q_from, terminal.q_to), (-qf, -qt));
            assert_injections_negate_terminals(&both);
        }
    }

    #[test]
    fn network_injections_negate_the_terminal_powers() {
        let mut net = BalancedNetwork::in_memory(
            "hvdc",
            100.0,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            Vec::new(),
        );
        let forward = line(1, 2, 100.0, 97.0, -30.0, -25.0);
        net.hvdc_mut().push(forward.clone());
        let injections = net.calc_hvdc_injections();
        let terminal = forward.calc_bus_to_device_powers();
        let [from, to] = &injections.terminals[..] else {
            panic!("one injecting line has two terminals");
        };
        assert_eq!((from.p, from.q), (-terminal.p_from, -terminal.q_from));
        assert_eq!((to.p, to.q), (-terminal.p_to, -terminal.q_to));
        let lost = injections.calc_sent_power() - injections.calc_delivered_power();
        assert!((lost - terminal.calc_active_loss()).abs() < 1e-12);
        assert!((injections.calc_reactive_injection() + 55.0).abs() < 1e-12);
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
