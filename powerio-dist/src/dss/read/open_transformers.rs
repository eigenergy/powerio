//! Fixed Transformer ports retain their winding circuit at internal zero-current nodes.
use super::{BusSpec, C, DistShunt, DistTransformer, RawDss, RawObject, Reader};
use crate::DistSwitch;

impl Reader<'_> {
    #[allow(clippy::too_many_lines)]
    pub(super) fn transformer_terminal_state(
        &mut self,
        object: &RawObject,
        raw: &RawDss,
        index: usize,
        mut transformer: DistTransformer,
        ports: &[Vec<String>],
    ) -> Result<DistTransformer, String> {
        let history = &raw.line_history[&index];
        let events: Vec<_> = history
            .events
            .iter()
            .filter(|e| e.terminal_state.is_some())
            .collect();
        let Some(first) = events.first() else {
            return Ok(transformer);
        };
        if !(1..=3).contains(&transformer.phases) {
            return Err("fixed transformer ports require one to three phases".into());
        }
        if history.copies.len() > first.position.copies
            || object.props[first.position.properties..]
                .iter()
                .any(|p| matches!(p.name.as_deref(), Some("phases" | "windings" | "xfmrcode")))
        {
            return Err("transformer layout/copy edits after a terminal command require ordered terminal reallocation".into());
        }
        if object.get("enabled").is_some_and(|v| !v.to_bool()) {
            return Err("disabled transformer with terminal commands is not qualified".into());
        }
        let mut closed = vec![vec![true; transformer.phases + 1]; transformer.windings.len()];
        for event in events {
            let (terminal, conductor, state) = event.terminal_state.unwrap();
            let port = closed
                .get_mut(terminal - 1)
                .ok_or("transformer terminal index exceeds winding count")?;
            if conductor == 0 {
                // Set_ConductorClosed(0) changes phase conductors only, not the neutral.
                port[..transformer.phases].fill(state);
            } else {
                *port
                    .get_mut(conductor - 1)
                    .ok_or("transformer conductor index exceeds terminal width")? = state;
            }
        }
        if closed.iter().flatten().all(|&state| state) {
            return Ok(transformer);
        }
        let base = format!("_pio_open_transformer_{}", object.name);
        let mut stem = base.clone();
        let mut suffix = 0;
        while self
            .buses
            .keys()
            .any(|s| s.contains(&stem.to_ascii_lowercase()))
            || raw.objects.iter().any(|o| {
                o.name
                    .to_ascii_lowercase()
                    .contains(&stem.to_ascii_lowercase())
                    || o.props.iter().any(|p| {
                        p.value
                            .text
                            .to_ascii_lowercase()
                            .contains(&stem.to_ascii_lowercase())
                    })
            })
            || raw.buscoords.iter().any(|b| {
                b.bus
                    .to_ascii_lowercase()
                    .contains(&stem.to_ascii_lowercase())
            })
        {
            suffix += 1;
            stem = format!("{base}_{suffix}");
        }
        let mut interfaces = Vec::new();
        for (i, winding) in transformer.windings.iter_mut().enumerate() {
            let original_bus = winding.bus.clone();
            let original_map = winding.terminal_map.clone();
            let bus = format!("{stem}_w{}", i + 1);
            let moved = closed[i][..original_map.len()].iter().any(|&state| !state);
            let mut contacts = vec![None; original_map.len()];
            if moved {
                let nodes = original_map
                    .iter()
                    .enumerate()
                    .map(|(c, node)| {
                        if closed[i][c] && node == "0" {
                            0
                        } else {
                            i32::try_from(c + 1).expect("transformer has at most four conductors")
                        }
                    })
                    .collect();
                winding.terminal_map = self.terminals(
                    &BusSpec {
                        name: bus.clone(),
                        nodes,
                    },
                    transformer.phases,
                    original_map.len(),
                    original_map.len(),
                );
                winding.bus = bus;
                for (c, node) in original_map.iter().enumerate() {
                    if closed[i][c] && node != "0" {
                        let name = format!("{stem}_w{}_c{}", i + 1, c + 1);
                        self.net.switches_mut().push(DistSwitch::new(
                            &name,
                            &original_bus,
                            &winding.bus,
                            vec![node.clone()],
                            vec![winding.terminal_map[c].clone()],
                            false,
                        ));
                        contacts[c] = Some(name);
                    }
                }
            }
            // DoYprimCalcs adds EPSILON to every external diagonal whenever any
            // conductor opens. Keep this outside the unchanged winding circuit.
            let n = ports[i].len();
            let mut g = vec![vec![0.0; n]; n];
            for (c, row) in g.iter_mut().enumerate() {
                row[c] = 1e-12;
            }
            let name = format!("{stem}_w{}_stabilization", i + 1);
            self.net.shunts_mut().push(DistShunt::new(
                &name,
                &original_bus,
                ports[i].clone(),
                g,
                vec![vec![0.0; n]; n],
            ));
            interfaces.push(
                serde_json::json!({"bus": original_bus, "terminal_map": ports[i],
                "closed": closed[i], "moved": moved, "contacts": contacts, "stabilization": name}),
            );
        }
        // This is boundary provenance, not an alternative set of electrical parameters.
        transformer
            .extras
            .insert("dss_open_transformer_ports".into(), interfaces.into());
        self.warn(&C::READ_DSS_RETAINED_SOURCE_ONLY, format!("Transformer.{}: fixed open ports retain the winding circuit at internal zero-current terminals, ideal closed contacts and native 1e-12 S external diagonal stabilization", object.name));
        Ok(transformer)
    }
}
