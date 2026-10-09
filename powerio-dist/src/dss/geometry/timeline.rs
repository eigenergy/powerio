//! Source-order electrical actions required by shared geometry calculations.
use super::{EarthModel, cache::Context};
use crate::dss::{line, line_history::Snapshot, linecode, raw::RawDss};
use std::collections::BTreeMap;

#[derive(Debug)]
pub(in crate::dss) enum Action {
    New {
        point: Snapshot,
        earth: Option<String>,
    },
    Property {
        point: Snapshot,
        offset: usize,
    },
    Copy {
        point: Snapshot,
        frequency: f64,
    },
    Solve {
        frequency: f64,
    },
    Terminal {
        object: usize,
        terminal: usize,
        conductor: usize,
        closed: bool,
    },
}

type States = BTreeMap<usize, Result<line::State, String>>;

pub(in crate::dss) fn resolve(raw: &RawDss) -> States {
    execute(raw, true).0
}

pub(in crate::dss) fn earth_before_clear(raw: &RawDss) -> Result<EarthModel, String> {
    execute(raw, false).1
}

#[expect(
    clippy::too_many_lines,
    reason = "source-order actions share one calculation context"
)]
fn execute(raw: &RawDss, finalize: bool) -> (States, Result<EarthModel, String>) {
    let mut context = Context::new(raw);
    let mut states = States::new();
    let mut faults = BTreeMap::new();
    let mut budget = 1_usize << 24;
    for action in &raw.geometry_timeline {
        match *action {
            Action::New { point, ref earth } => {
                if raw.objects[point.object].class == "line" {
                    let state =
                        EarthModel::parse(earth.as_deref().unwrap_or("deri")).map(|earth| {
                            let mut state = line::State::new(
                                raw.line_history[&point.object].constructor_frequency,
                                &raw.objects[point.object].name,
                            );
                            state.earth_model = earth;
                            state
                        });
                    states.insert(point.object, state);
                } else if let Err(e) = context.observe(point, None) {
                    faults.insert(point.object, e);
                }
            }
            Action::Property { point, offset } => {
                let prop = &raw.objects[point.object].props[offset];
                if let Some(result) = states.get_mut(&point.object) {
                    if let Ok(state) = result {
                        let change = (|| match prop.name.as_deref() {
                            Some("geometry") => {
                                let source = raw.line_history[&point.object]
                                    .geometries
                                    .get(&offset)
                                    .ok_or("missing LineGeometry binding")?
                                    .ok_or("LineGeometry did not exist at assignment")?;
                                if let Some(e) = faults.get(&source.object) {
                                    return Err(e.clone());
                                }
                                let definition = context.definition(source.object)?;
                                state.fetch_geometry(source.object, &definition, &mut context)
                            }
                            Some("spacing") => {
                                let source = raw.line_history[&point.object]
                                    .spacings
                                    .get(&offset)
                                    .ok_or("missing LineSpacing binding")?
                                    .ok_or("LineSpacing did not exist at assignment")?;
                                state.fetch_spacing(source.object, &context)
                            }
                            Some("wires" | "cncables") => {
                                let objects = raw.line_history[&point.object]
                                    .wires
                                    .get(&offset)
                                    .ok_or("missing conductor bindings")?
                                    .iter()
                                    .map(|p| {
                                        p.map(|s| s.object)
                                            .ok_or("conductor did not exist at assignment")
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                state.bind_conductors(
                                    prop.name.as_deref() == Some("cncables"),
                                    &objects,
                                    &context,
                                )
                            }
                            Some("linecode") => {
                                let source = raw.line_history[&point.object]
                                    .linecodes
                                    .get(&offset)
                                    .ok_or("missing LineCode binding")?
                                    .ok_or("LineCode did not exist at assignment")?;
                                let code = linecode::resolve_with_budget(raw, source, &mut budget)?;
                                state.fetch_code(&code);
                                Ok(())
                            }
                            _ => {
                                state.apply(prop, raw)?;
                                if prop.name.as_deref() == Some("rho") {
                                    state.update_geometry_rho(&mut context)?;
                                }
                                Ok(())
                            }
                        })();
                        if let Err(e) = change {
                            *result = Err(e);
                        }
                    }
                } else if let Err(e) = context.observe(point, prop.name.as_deref()) {
                    faults.insert(point.object, e);
                }
            }
            Action::Copy { point, frequency } => {
                if raw.objects[point.object].class == "line" {
                    let source = raw.line_history[&point.object].copies[point.copies - 1]
                        .source
                        .object;
                    let source = states
                        .get(&source)
                        .cloned()
                        .unwrap_or_else(|| Err("missing source Line".into()));
                    if let Some(result) = states.get_mut(&point.object) {
                        match (result.as_mut(), source) {
                            (Ok(state), Ok(source)) => state.make_like(&source),
                            (Ok(_), Err(e)) => *result = Err(e),
                            (Err(_), _) => {}
                        }
                    }
                } else {
                    let result = context.observe(point, Some("like")).and_then(|()| {
                        if raw.objects[point.object].class == "linegeometry" {
                            context.calculate(point.object, frequency)?;
                        }
                        Ok(())
                    });
                    if let Err(e) = result {
                        faults.insert(point.object, e);
                    }
                }
            }
            Action::Solve { frequency } => solve(&mut states, &mut context, frequency, &mut budget),
            Action::Terminal {
                object,
                terminal,
                conductor,
                closed,
            } => {
                if let Some(result) = states.get_mut(&object)
                    && let Ok(state) = result
                    && let Err(e) = state.set_closed(terminal, conductor, closed)
                {
                    *result = Err(e);
                }
            }
        }
    }
    if finalize {
        solve(
            &mut states,
            &mut context,
            raw.snapshot_frequency(),
            &mut budget,
        );
    }
    (states, context.active_earth)
}

fn solve(states: &mut States, context: &mut Context<'_>, frequency: f64, budget: &mut usize) {
    for result in states.values_mut() {
        if let Err(error) = result {
            context.active_earth = Err(format!(
                "prior unresolved Line calculation may affect active earth model: {error}"
            ));
        }
        if let Ok(state) = result {
            let Some(remaining) = budget.checked_sub(state.n * state.n * 6 + 1) else {
                *result = Err("geometry Line evaluation work limit exceeded".into());
                continue;
            };
            *budget = remaining;
        }
        if let Ok(state) = result
            && state.enabled
            && let Err(e) = state.primitive_with_geometry(frequency, context)
        {
            *result = Err(e);
        }
    }
}

#[cfg(test)]
mod tests;
