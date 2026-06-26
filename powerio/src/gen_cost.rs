use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::network::{BusId, GenCost, Network};
use crate::{Error, Result};

/// Policy for generators whose source format has no active-power cost row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum MissingGenCostPolicy {
    /// Leave missing costs absent.
    #[default]
    Preserve,
    /// Error when an in-service generator has no cost row.
    Require,
    /// Fill missing costs with a MATPOWER polynomial row.
    Fill {
        c2: f64,
        c1: f64,
        c0: f64,
        startup: f64,
        shutdown: f64,
    },
}

impl MissingGenCostPolicy {
    #[must_use]
    pub fn zero() -> Self {
        Self::Fill {
            c2: 0.0,
            c1: 0.0,
            c0: 0.0,
            startup: 0.0,
            shutdown: 0.0,
        }
    }

    #[must_use]
    pub fn quadratic(c2: f64, c1: f64, c0: f64) -> Self {
        Self::Fill {
            c2,
            c1,
            c0,
            startup: 0.0,
            shutdown: 0.0,
        }
    }

    #[must_use]
    pub fn is_preserve(self) -> bool {
        matches!(self, Self::Preserve)
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Require => "require",
            Self::Fill { .. } => "fill",
        }
    }

    fn fill_cost(c2: f64, c1: f64, c0: f64, startup: f64, shutdown: f64) -> Result<GenCost> {
        for (field, value) in [
            ("c2", c2),
            ("c1", c1),
            ("c0", c0),
            ("startup", startup),
            ("shutdown", shutdown),
        ] {
            if !value.is_finite() {
                return Err(Error::NonFiniteGenCost { field, value });
            }
        }
        Ok(GenCost {
            model: 2,
            startup,
            shutdown,
            ncost: 3,
            coeffs: vec![c2, c1, c0],
        })
    }
}

/// One explicit generator cost patch from a user supplied table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenCostPatch {
    /// Zero based index into [`Network::generators`].
    pub gen_index: usize,
    /// Bus id expected on that generator, used to catch stale patch tables.
    pub bus: BusId,
    pub cost: GenCost,
}

/// Counts produced by applying user cost patches and a missing-cost policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenCostPolicyReport {
    pub missing_before: usize,
    pub missing_in_service_before: usize,
    pub patched: usize,
    pub synthesized: usize,
}

impl Network {
    /// Apply explicit cost patches, then a missing-cost policy.
    ///
    /// Patches replace the existing cost for the named generator. The missing-cost
    /// fill policy only touches generators still missing a cost after patching.
    pub fn apply_gen_cost_policy(
        &mut self,
        patches: &[GenCostPatch],
        policy: MissingGenCostPolicy,
    ) -> Result<GenCostPolicyReport> {
        let patched = self.apply_gen_cost_patches(patches)?;
        let missing_before = self.generators.iter().filter(|g| g.cost.is_none()).count();
        let missing_in_service_before = self
            .generators
            .iter()
            .filter(|g| g.in_service && g.cost.is_none())
            .count();

        let mut synthesized = 0usize;
        match policy {
            MissingGenCostPolicy::Preserve => {}
            MissingGenCostPolicy::Require => {
                if let Some((idx, _)) = self
                    .generators
                    .iter()
                    .enumerate()
                    .find(|(_, g)| g.in_service && g.cost.is_none())
                {
                    return Err(Error::MissingGenCost { gen_index: idx });
                }
            }
            MissingGenCostPolicy::Fill {
                c2,
                c1,
                c0,
                startup,
                shutdown,
            } => {
                let cost = MissingGenCostPolicy::fill_cost(c2, c1, c0, startup, shutdown)?;
                for generator in &mut self.generators {
                    if generator.cost.is_none() {
                        generator.cost = Some(cost.clone());
                        synthesized += 1;
                    }
                }
            }
        }

        Ok(GenCostPolicyReport {
            missing_before,
            missing_in_service_before,
            patched,
            synthesized,
        })
    }

    fn apply_gen_cost_patches(&mut self, patches: &[GenCostPatch]) -> Result<usize> {
        let mut seen = BTreeSet::new();
        for (row, patch) in patches.iter().enumerate() {
            let row = row + 1;
            if !seen.insert(patch.gen_index) {
                return Err(Error::InvalidGenCostPatch {
                    row,
                    reason: format!("duplicate gen_index {}", patch.gen_index),
                });
            }
            let Some(generator) = self.generators.get_mut(patch.gen_index) else {
                return Err(Error::InvalidGenCostPatch {
                    row,
                    reason: format!(
                        "gen_index {} out of range for {} generator(s)",
                        patch.gen_index,
                        self.generators.len()
                    ),
                });
            };
            if generator.bus != patch.bus {
                return Err(Error::InvalidGenCostPatch {
                    row,
                    reason: format!(
                        "bus mismatch for gen_index {}: table has {}, network has {}",
                        patch.gen_index, patch.bus, generator.bus
                    ),
                });
            }
            validate_cost(&patch.cost, row)?;
            generator.cost = Some(patch.cost.clone());
        }
        Ok(patches.len())
    }
}

/// Parse a simple generator cost CSV with required columns
/// `gen_index,bus,c2,c1,c0` and optional `startup,shutdown`.
///
/// The parser accepts plain comma separated fields with a header row. Quoted CSV
/// dialect features are intentionally not implemented; this table is numeric.
pub fn parse_gen_cost_csv(content: &str) -> Result<Vec<GenCostPatch>> {
    let mut lines = content
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());
    let Some((_, header)) = lines.next() else {
        return Err(Error::InvalidGenCostPatch {
            row: 0,
            reason: "empty generator cost CSV".into(),
        });
    };
    let header = split_csv_line(header);
    let col = |name: &'static str| {
        header
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| Error::InvalidGenCostPatch {
                row: 0,
                reason: format!("missing required column `{name}`"),
            })
    };
    let gen_index_col = col("gen_index")?;
    let bus_col = col("bus")?;
    let c2_col = col("c2")?;
    let c1_col = col("c1")?;
    let c0_col = col("c0")?;
    let startup_col = header.iter().position(|h| h == "startup");
    let shutdown_col = header.iter().position(|h| h == "shutdown");

    let mut out = Vec::new();
    for (line_no, line) in lines {
        let row = line_no + 1;
        let fields = split_csv_line(line);
        let get = |idx: usize, name: &'static str| {
            fields
                .get(idx)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| Error::InvalidGenCostPatch {
                    row,
                    reason: format!("missing value for `{name}`"),
                })
        };
        let gen_index = parse_usize(get(gen_index_col, "gen_index")?, row, "gen_index")?;
        let bus = BusId(parse_usize(get(bus_col, "bus")?, row, "bus")?);
        let c2 = parse_f64(get(c2_col, "c2")?, row, "c2")?;
        let c1 = parse_f64(get(c1_col, "c1")?, row, "c1")?;
        let c0 = parse_f64(get(c0_col, "c0")?, row, "c0")?;
        let startup = match startup_col {
            Some(idx) => fields
                .get(idx)
                .filter(|s| !s.is_empty())
                .map_or(Ok(0.0), |s| parse_f64(s, row, "startup"))?,
            None => 0.0,
        };
        let shutdown = match shutdown_col {
            Some(idx) => fields
                .get(idx)
                .filter(|s| !s.is_empty())
                .map_or(Ok(0.0), |s| parse_f64(s, row, "shutdown"))?,
            None => 0.0,
        };
        out.push(GenCostPatch {
            gen_index,
            bus,
            cost: GenCost {
                model: 2,
                startup,
                shutdown,
                ncost: 3,
                coeffs: vec![c2, c1, c0],
            },
        });
    }
    Ok(out)
}

fn split_csv_line(line: &str) -> Vec<String> {
    line.split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .collect()
}

fn parse_usize(value: &str, row: usize, field: &'static str) -> Result<usize> {
    value
        .parse::<usize>()
        .map_err(|_| Error::InvalidGenCostPatch {
            row,
            reason: format!("`{field}` is not a non-negative integer: {value}"),
        })
}

fn parse_f64(value: &str, row: usize, field: &'static str) -> Result<f64> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| Error::InvalidGenCostPatch {
            row,
            reason: format!("`{field}` is not a number: {value}"),
        })?;
    if parsed.is_finite() {
        Ok(parsed)
    } else {
        Err(Error::InvalidGenCostPatch {
            row,
            reason: format!("`{field}` is not finite: {parsed}"),
        })
    }
}

fn validate_cost(cost: &GenCost, row: usize) -> Result<()> {
    for (field, value) in [("startup", cost.startup), ("shutdown", cost.shutdown)] {
        if !value.is_finite() {
            return Err(Error::InvalidGenCostPatch {
                row,
                reason: format!("`{field}` is not finite: {value}"),
            });
        }
    }
    for (idx, value) in cost.coeffs.iter().enumerate() {
        if !value.is_finite() {
            return Err(Error::InvalidGenCostPatch {
                row,
                reason: format!("cost coefficient {idx} is not finite: {value}"),
            });
        }
    }
    Ok(())
}
