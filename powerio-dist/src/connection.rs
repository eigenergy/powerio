//! The channels of a typed load, injection, capacitor or winding connection.

use crate::{Configuration, MulticonductorNetwork};

/// One channel of a connection: the terminal its power enters and the
/// terminal it returns through, as positions in the element's terminal map.
/// `to` is `None` for a channel that returns through ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConnectionChannel {
    pub from: usize,
    pub to: Option<usize>,
}

impl ConnectionChannel {
    const fn new(from: usize, to: Option<usize>) -> Self {
        Self { from, to }
    }
}

/// The channels of a connection of `terminal_count` terminals carrying
/// `channel_count` stated powers, one channel per power in power order:
///
/// - single phase or delta, two terminals and one power: the first terminal
///   to the second;
/// - wye or single phase, one terminal per power: each terminal to ground;
/// - wye with one terminal more than its powers: each phase terminal to the
///   neutral, the terminal at `neutral` or else the last;
/// - delta with two terminals per power: each consecutive pair, the first of
///   the pair to the second;
/// - delta with one terminal per power: a ring, terminal `k` to terminal
///   `k + 1`, the last back to the first.
///
/// `neutral` is read only for a wye with an explicit neutral. The one
/// connection rule: the LinDist3Flow incidence, the GridLAB-D writer,
/// [`calc_element_connection`] (every load, capacitor, inverter, source and
/// transformer winding the multiconductor calculations connect) read it.
///
/// # Errors
/// A configuration and arity that describe none of these connections, or a
/// `neutral` position outside the terminal map.
pub fn calc_connection_channels(
    configuration: Configuration,
    terminal_count: usize,
    channel_count: usize,
    neutral: Option<usize>,
) -> Result<Vec<ConnectionChannel>, String> {
    let channels = match configuration {
        Configuration::SinglePhase | Configuration::Delta
            if terminal_count == 2 && channel_count == 1 =>
        {
            vec![ConnectionChannel::new(0, Some(1))]
        }
        Configuration::Wye | Configuration::SinglePhase if terminal_count == channel_count => (0
            ..terminal_count)
            .map(|terminal| ConnectionChannel::new(terminal, None))
            .collect(),
        Configuration::Wye if terminal_count == channel_count + 1 => {
            let neutral = neutral.unwrap_or(channel_count);
            if neutral >= terminal_count {
                return Err(format!(
                    "wye neutral position {neutral} is outside its {terminal_count} terminals"
                ));
            }
            (0..terminal_count)
                .filter(|&terminal| terminal != neutral)
                .map(|phase| ConnectionChannel::new(phase, Some(neutral)))
                .collect()
        }
        Configuration::Delta if channel_count > 0 && terminal_count == 2 * channel_count => (0
            ..channel_count)
            .map(|pair| ConnectionChannel::new(2 * pair, Some(2 * pair + 1)))
            .collect(),
        Configuration::Delta if channel_count > 0 && terminal_count == channel_count => (0
            ..terminal_count)
            .map(|terminal| ConnectionChannel::new(terminal, Some((terminal + 1) % terminal_count)))
            .collect(),
        _ => {
            return Err(format!(
                "{configuration:?} connection with {terminal_count} terminals and {channel_count} channels is unsupported"
            ));
        }
    };
    Ok(channels)
}

/// Whether the LinDist3Flow model carries a connection: its
/// [`calc_connection_channels`] exist and need no explicit neutral and no
/// split delta, so every WYE channel returns through ground and a DELTA
/// connection is one terminal pair or the three phase ring. The one
/// LinDist3Flow connection rule: the instance shape check and the incidence
/// builder read it.
#[must_use]
pub fn lindist3flow_carries_connection(
    configuration: Configuration,
    terminal_count: usize,
    channel_count: usize,
) -> bool {
    calc_connection_channels(configuration, terminal_count, channel_count, None).is_ok_and(
        |channels| match configuration {
            Configuration::Wye => channels.iter().all(|channel| channel.to.is_none()),
            Configuration::Delta => {
                channels.len() == 1 || (channels.len() == 3 && terminal_count == 3)
            }
            _ => true,
        },
    )
}

/// The position of a WYE connection's explicit neutral in an element's
/// terminal map: the one mapped terminal that the bus's terminal
/// conventions (the network's `bmopf_terminal_conventions`, else the
/// `n` and four wire `4` spellings, [`crate::DistBus::phase_indices`]) do
/// not list as a phase, or ground terminal `0`. `None` when every mapped
/// terminal is a phase.
///
/// # Errors
/// A bus the network does not declare, or more than one mapped terminal
/// that is not a phase.
pub fn calc_wye_neutral_position(
    network: &MulticonductorNetwork,
    bus: &str,
    terminal_map: &[String],
) -> Result<Option<usize>, String> {
    let declared = network
        .bus(bus)
        .ok_or_else(|| format!("bus `{bus}` is not declared"))?;
    let phases: Vec<&str> = declared
        .phase_indices(network.extras().get("bmopf_terminal_conventions"))
        .into_iter()
        .map(|index| declared.terminals[index].as_str())
        .filter(|terminal| *terminal != "0")
        .collect();
    let mut nonphase = terminal_map
        .iter()
        .enumerate()
        .filter(|(_, terminal)| !phases.contains(&terminal.as_str()))
        .map(|(position, _)| position);
    let neutral = nonphase.next();
    if nonphase.next().is_some() {
        return Err(format!(
            "bus `{bus}` maps more than one non-phase terminal: the WYE neutral is ambiguous"
        ));
    }
    Ok(neutral)
}

/// The channels of an element's connection at `bus` over its terminal map
/// ([`calc_connection_channels`]), with a WYE neutral found by
/// [`calc_wye_neutral_position`]. `channel_count` is the element's number of
/// stated powers or phases; `None` reads it from the map: a WYE has one
/// channel per phase terminal, a two terminal DELTA or single phase element
/// one channel, and any other DELTA one channel per terminal (a ring).
///
/// The one rule for which terminal pair each coil of a connection spans:
/// the multiconductor OPF, the capacitor primitive, the transformer
/// primitive and its ideal constraints read it.
///
/// # Errors
/// An undeclared bus, an ambiguous WYE neutral, or a connection
/// [`calc_connection_channels`] refuses.
pub fn calc_element_connection(
    network: &MulticonductorNetwork,
    bus: &str,
    terminal_map: &[String],
    configuration: Configuration,
    channel_count: Option<usize>,
) -> Result<Vec<ConnectionChannel>, String> {
    let neutral = if configuration == Configuration::Wye {
        calc_wye_neutral_position(network, bus, terminal_map)?
    } else {
        None
    };
    let count = terminal_map.len();
    let channels = channel_count.unwrap_or(match configuration {
        Configuration::Wye => count - usize::from(neutral.is_some()),
        _ if count == 2 => 1,
        _ => count,
    });
    calc_connection_channels(configuration, count, channels, neutral)
}

/// Orient a DELTA ring of channels: `+1` keeps each channel from terminal
/// `k` to the next terminal of the ring, `-1` returns each to the preceding
/// one. A ring of fewer than three channels has one orientation.
pub fn orient_delta_ring(channels: &mut [ConnectionChannel], direction: i8) {
    let n = channels.len();
    if direction >= 0 || n < 3 {
        return;
    }
    let positives: Vec<usize> = channels.iter().map(|channel| channel.from).collect();
    for (k, channel) in channels.iter_mut().enumerate() {
        channel.to = Some(positives[(k + n - 1) % n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(channels: &[ConnectionChannel]) -> Vec<(usize, Option<usize>)> {
        channels.iter().map(|c| (c.from, c.to)).collect()
    }

    #[test]
    fn every_connection_shape_has_one_channel_per_power() {
        let wye = calc_connection_channels(Configuration::Wye, 3, 3, None).unwrap();
        assert_eq!(pairs(&wye), [(0, None), (1, None), (2, None)]);
        let neutral = calc_connection_channels(Configuration::Wye, 4, 3, None).unwrap();
        assert_eq!(pairs(&neutral), [(0, Some(3)), (1, Some(3)), (2, Some(3))]);
        let first = calc_connection_channels(Configuration::Wye, 4, 3, Some(0)).unwrap();
        assert_eq!(pairs(&first), [(1, Some(0)), (2, Some(0)), (3, Some(0))]);
        let single = calc_connection_channels(Configuration::SinglePhase, 2, 1, None).unwrap();
        assert_eq!(pairs(&single), [(0, Some(1))]);
        let ring = calc_connection_channels(Configuration::Delta, 3, 3, None).unwrap();
        assert_eq!(pairs(&ring), [(0, Some(1)), (1, Some(2)), (2, Some(0))]);
        let stated = calc_connection_channels(Configuration::Delta, 4, 2, None).unwrap();
        assert_eq!(pairs(&stated), [(0, Some(1)), (2, Some(3))]);
        assert!(calc_connection_channels(Configuration::Wye, 5, 3, None).is_err());
        assert!(calc_connection_channels(Configuration::Wye, 4, 3, Some(4)).is_err());
        assert!(calc_connection_channels(Configuration::Delta, 5, 2, None).is_err());
    }
}
