//! The channels of a typed load or injection connection.

use crate::Configuration;

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
/// connection rule: the LinDist3Flow incidence, the GridLAB-D writer and
/// every load evaluation over terminal pairs read it.
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
