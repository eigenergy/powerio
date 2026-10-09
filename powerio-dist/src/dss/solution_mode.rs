//! Ordered solution mode selection, without running any solver or time loop.

// Pinned DSSClass.SolveModeEnum order, including its compatibility aliases.
const MODES: &[(&str, &str)] = &[
    ("snap", "snapshot"),
    ("daily", "daily"),
    ("yearly", "yearly"),
    ("m1", "m1"),
    ("ld1", "ld1"),
    ("peakday", "peakday"),
    ("dutycycle", "dutycycle"),
    ("direct", "direct"),
    ("mf", "mf"),
    ("faultstudy", "faultstudy"),
    ("m2", "m2"),
    ("m3", "m3"),
    ("ld2", "ld2"),
    ("autoadd", "autoadd"),
    ("dynamic", "dynamic"),
    ("harmonic", "harmonic"),
    ("time", "time"),
    ("harmonict", "harmonict"),
    ("snapshot", "snapshot"),
    ("dynamics", "dynamic"),
    ("harmonics", "harmonic"),
    ("s", "snapshot"),
    ("y", "yearly"),
    ("h", "harmonic"),
    ("t", "time"),
    ("f", "faultstudy"),
];

#[derive(Debug)]
pub(super) struct SolutionMode {
    selected: &'static str,
    requires_solved_state: Option<&'static str>,
}

impl Default for SolutionMode {
    fn default() -> Self {
        Self {
            selected: "snapshot",
            requires_solved_state: None,
        }
    }
}

impl SolutionMode {
    pub fn assign(&mut self, value: &str) {
        self.selected = resolve(value);
        // These transitions require a converged native solution and can
        // initialize dynamic/harmonic state. Selecting Snapshot afterwards
        // does not prove that the intervening state was materialized.
        if matches!(
            self.selected,
            "dynamic" | "harmonic" | "harmonict" | "faultstudy" | "mf"
        ) {
            self.requires_solved_state.get_or_insert(self.selected);
        }
    }

    #[cfg(test)]
    pub fn name(&self) -> &'static str {
        self.selected
    }

    #[cfg(test)]
    pub fn unresolved_transition(&self) -> Option<&'static str> {
        self.requires_solved_state
    }
}

fn resolve(value: &str) -> &'static str {
    let value = value.to_ascii_lowercase();
    // Native MinChars=2, MaxChars=9, TryExactFirst for short aliases,
    // UseFirstFound for ambiguous prefixes; unmatched text defaults to Snap.
    if value.len() < 2 {
        return MODES
            .iter()
            .find(|(name, _)| *name == value)
            .map_or("snapshot", |(_, mode)| *mode);
    }
    for count in 2..=value.len().min(9) {
        let mut matches = MODES.iter().filter(|(name, _)| {
            name.len() >= value.len() && name.as_bytes()[..count] == value.as_bytes()[..count]
        });
        if let Some((name, mode)) = matches.next()
            && ((count == 2 && *name == value) || count == value.len() || matches.next().is_none())
        {
            return mode;
        }
    }
    "snapshot"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_alias_order_and_default_are_preserved() {
        for (value, expected) in [
            ("s", "snapshot"),
            ("har", "harmonic"),
            ("harmonict", "harmonict"),
            ("harmonics", "harmonic"),
            ("d", "snapshot"),
            ("di", "direct"),
            ("dailyx", "snapshot"),
            ("7", "snapshot"),
            ("du", "dutycycle"),
            ("SNAPSHOT", "snapshot"),
        ] {
            assert_eq!(resolve(value), expected, "{value}");
        }
    }
}
