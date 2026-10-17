//! Branch-preserving load decomposition for canonical OpenDSS output.

use super::{Configuration, DistLoad, DistLoadVoltageModel, MulticonductorNetwork, delta_edges};
use std::collections::{BTreeMap, BTreeSet};

fn parameters(model: &DistLoadVoltageModel) -> Vec<&[f64]> {
    match model {
        DistLoadVoltageModel::ConstantPower { v_nom }
        | DistLoadVoltageModel::ConstantCurrent { v_nom }
        | DistLoadVoltageModel::ConstantImpedance { v_nom } => vec![v_nom],
        DistLoadVoltageModel::Exponential {
            v_nom,
            gamma_p,
            gamma_q,
        }
        | DistLoadVoltageModel::BoundedExponential {
            v_nom,
            gamma_p,
            gamma_q,
            ..
        } => vec![v_nom, gamma_p, gamma_q],
        DistLoadVoltageModel::Zip {
            v_nom,
            alpha_z,
            alpha_i,
            alpha_p,
            beta_z,
            beta_i,
            beta_p,
        }
        | DistLoadVoltageModel::BoundedZip {
            v_nom,
            alpha_z,
            alpha_i,
            alpha_p,
            beta_z,
            beta_i,
            beta_p,
            ..
        } => vec![v_nom, alpha_z, alpha_i, alpha_p, beta_z, beta_i, beta_p],
    }
}

pub(super) fn phase_model(model: &DistLoadVoltageModel, phase: usize) -> DistLoadVoltageModel {
    let mut model = model.clone();
    let arrays = match &mut model {
        DistLoadVoltageModel::ConstantPower { v_nom }
        | DistLoadVoltageModel::ConstantCurrent { v_nom }
        | DistLoadVoltageModel::ConstantImpedance { v_nom } => vec![v_nom],
        DistLoadVoltageModel::Exponential {
            v_nom,
            gamma_p,
            gamma_q,
        }
        | DistLoadVoltageModel::BoundedExponential {
            v_nom,
            gamma_p,
            gamma_q,
            ..
        } => vec![v_nom, gamma_p, gamma_q],
        DistLoadVoltageModel::Zip {
            v_nom,
            alpha_z,
            alpha_i,
            alpha_p,
            beta_z,
            beta_i,
            beta_p,
        }
        | DistLoadVoltageModel::BoundedZip {
            v_nom,
            alpha_z,
            alpha_i,
            alpha_p,
            beta_z,
            beta_i,
            beta_p,
            ..
        } => vec![v_nom, alpha_z, alpha_i, alpha_p, beta_z, beta_i, beta_p],
    };
    for (i, values) in arrays.into_iter().enumerate() {
        // Empty nominal voltages are a supported legacy convention. Public
        // emission validates every supplied array before selecting a branch.
        if i != 0 || values.len() != 1 {
            *values = values.get(phase).copied().into_iter().collect();
        }
    }
    model
}

pub(super) fn needs_split(load: &DistLoad) -> bool {
    #[allow(clippy::float_cmp)] // Any nonzero imbalance needs separate native loads.
    let nonuniform = |xs: &[f64]| xs.windows(2).any(|pair| pair[0] != pair[1]);
    nonuniform(&load.p_nom)
        || nonuniform(&load.q_nom)
        || parameters(&load.voltage_model)
            .iter()
            .any(|xs| nonuniform(xs))
}

pub(super) fn branch_maps(
    load: &DistLoad,
    return_index: Option<usize>,
) -> Option<Vec<Vec<String>>> {
    let n = load.p_nom.len();
    let map = &load.terminal_map;
    if n == 0 || load.q_nom.len() != n {
        return None;
    }
    if load.configuration == Configuration::Delta {
        if !((n >= 3 && map.len() == n) || (n < 3 && map.len() == n + 1)) {
            return None;
        }
        return Some(
            delta_edges(map.len(), n)
                .into_iter()
                .map(|(a, b)| vec![map[a].clone(), map[b].clone()])
                .collect(),
        );
    }
    let return_index = return_index.or_else(|| (map.len() == n + 1).then_some(n));
    let hot: Vec<_> = (0..map.len())
        .filter(|i| Some(*i) != return_index)
        .collect();
    if hot.len() != n {
        return None;
    }
    Some(
        hot.into_iter()
            .map(|i| {
                let mut branch = vec![map[i].clone()];
                if let Some(r) = return_index {
                    branch.push(map[r].clone());
                }
                branch
            })
            .collect(),
    )
}

pub(super) fn validate(net: &MulticonductorNetwork) -> Result<(), String> {
    let mut names = BTreeSet::new();
    let grounded: BTreeMap<_, _> = net
        .buses()
        .iter()
        .map(|bus| (bus.id.to_ascii_lowercase(), bus.grounded.as_slice()))
        .collect();
    for load in net.loads() {
        if !names.insert(load.name.to_ascii_lowercase()) {
            return Err(format!("load {}: duplicate native object name", load.name));
        }
        let n = load.p_nom.len();
        for (i, array) in parameters(&load.voltage_model).iter().enumerate() {
            let legacy_scalar_voltage = i == 0
                && array.len() == 1
                && !matches!(
                    load.voltage_model,
                    DistLoadVoltageModel::BoundedZip { .. }
                        | DistLoadVoltageModel::BoundedExponential { .. }
                );
            if (i != 0 || !array.is_empty()) && array.len() != n && !legacy_scalar_voltage {
                return Err(format!(
                    "load {}: voltage-law array must align with its {n} power branches",
                    load.name
                ));
            }
        }
        {
            let grounded = grounded
                .get(&load.bus.to_ascii_lowercase())
                .copied()
                .unwrap_or_default();
            let indices: Vec<_> = load
                .terminal_map
                .iter()
                .enumerate()
                .filter_map(|(i, t)| grounded.contains(t).then_some(i))
                .collect();
            let return_index = (indices.len() == 1).then(|| indices[0]);
            let misordered = load.configuration != Configuration::Delta
                && return_index.is_some_and(|i| i + 1 != load.terminal_map.len());
            if (needs_split(load) || misordered) && branch_maps(load, return_index).is_none() {
                return Err(format!(
                    "load {}: branch powers do not align with its terminal map",
                    load.name
                ));
            }
        }
    }
    Ok(())
}
