//! Assignment-time OpenDSS transformer-code state (pinned DSS C-API).
//!
//! XfmrCode.pas at 87d85c2622c8281b92255335bc7c09b11191b21d is the
//! behavioral reference. Winding voltage/rating use kV/kVA; R and X use
//! percent. Source-order copy prefixes reuse the bounded raw history.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::{
    line_history::Snapshot,
    raw::{RawDss, RawProp},
};

#[derive(Clone, Debug)]
pub(super) struct Winding {
    pub delta: bool,
    pub kv: f64,
    pub kva: f64,
    pub tap: f64,
    pub r: f64,
    pub rneut: f64,
    pub xneut: f64,
}

impl Default for Winding {
    fn default() -> Self {
        Self {
            delta: false,
            kv: 12.47,
            kva: 1000.0,
            tap: 1.0,
            r: 0.2,
            rneut: -1.0,
            xneut: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct State {
    pub phases: usize,
    pub windings: Vec<Winding>,
    active: usize,
    pub x: [f64; 3],
    pub xsc: Vec<f64>,
    changed: bool,
    pub extras: BTreeMap<String, String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            phases: 3,
            windings: vec![Winding::default(); 2],
            active: 0,
            x: [7.0, 35.0, 30.0],
            xsc: vec![0.0],
            changed: false,
            extras: [
                ("thermal", 2.0),
                ("n", 0.8),
                ("m", 0.8),
                ("flrise", 65.0),
                ("hsrise", 15.0),
                ("%loadloss", 0.4),
                ("%noloadloss", 0.0),
                ("%imag", 0.0),
                ("ppm_antifloat", 1.0),
                ("normhkva", 1100.0),
                ("emerghkva", 1500.0),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.to_string()))
            .collect(),
        }
    }
}

impl State {
    fn finish_edit(&mut self) {
        if self.changed && self.windings.len() <= 3 {
            let n = self.xsc.len();
            self.xsc.copy_from_slice(&self.x[..n]);
        }
        self.changed = false;
    }

    fn make_like(&mut self, source: &Self) {
        // Native MakeLike omits excitation/antifloat and the selector.
        let (active, changed) = (self.active, self.changed);
        let retained: Vec<_> = ["%imag", "ppm_antifloat"]
            .iter()
            .map(|&k| (k.to_owned(), self.extras[k].clone()))
            .collect();
        *self = source.clone();
        self.active = active;
        self.changed = changed;
        self.extras.extend(retained);
    }

    fn load_loss(&mut self) {
        self.extras.insert(
            "%loadloss".into(),
            (self.windings[0].r + self.windings[1].r).to_string(),
        );
    }

    fn ratings(&mut self) {
        self.extras
            .insert("normhkva".into(), (1.1 * self.windings[0].kva).to_string());
        self.extras
            .insert("emerghkva".into(), (1.5 * self.windings[0].kva).to_string());
    }

    fn set_count(&mut self, key: &str, value: f64) -> Result<(), String> {
        if !(1.0..=64.0).contains(&value) || value.fract() != 0.0 {
            return Err(format!("invalid XfmrCode {key}"));
        }
        let n = value as usize;
        match key {
            "phases" => self.phases = n,
            "windings" if n >= 2 => {
                self.windings = vec![Winding::default(); n];
                self.xsc.resize(n * (n - 1) / 2, 30.0);
            }
            "wdg" if n <= self.windings.len() => self.active = n - 1,
            _ => return Err(format!("invalid XfmrCode {key}")),
        }
        Ok(())
    }

    fn apply_array(&mut self, key: &str, p: &RawProp, raw: &RawDss) -> Result<(), String> {
        let values = p
            .value
            .to_vector(Some(&raw.vars))
            .map_err(|e| e.to_string())?;
        let n = if key == "xscarray" {
            self.xsc.len()
        } else {
            self.windings.len()
        };
        if values.len() != n || values.iter().any(|v| !v.is_finite()) {
            return Err(format!("invalid XfmrCode {key} array"));
        }
        if key == "xscarray" {
            self.xsc = values;
        } else {
            let scalar = match key {
                "kvs" => "kv",
                "kvas" => "kva",
                "taps" => "tap",
                _ => "%r",
            };
            for (w, v) in self.windings.iter_mut().zip(values) {
                set_number(w, scalar, v);
            }
            if key == "kvas" {
                self.ratings();
            }
            if key == "%rs" {
                self.load_loss();
            }
        }
        Ok(())
    }

    fn apply(&mut self, p: &RawProp, raw: &RawDss) -> Result<(), String> {
        let key = p.name.as_deref().ok_or("unnamed XfmrCode property")?;
        let number = || {
            let v = p.value.to_f64(Some(&raw.vars)).map_err(|e| e.to_string())?;
            if v.is_finite() {
                Ok(v)
            } else {
                Err(format!("nonfinite XfmrCode {key}"))
            }
        };
        match key {
            "phases" | "windings" | "wdg" => self.set_count(key, number()?)?,
            "conn" => {
                let w = self
                    .windings
                    .get_mut(self.active)
                    .ok_or("invalid XfmrCode winding selector")?;
                w.delta = connection(&p.value.text)?;
            }
            "kv" | "kva" | "tap" | "%r" | "rneut" | "xneut" => {
                let v = number()?;
                let w = self
                    .windings
                    .get_mut(self.active)
                    .ok_or("invalid XfmrCode winding selector")?;
                set_number(w, key, v);
                if key == "kva" {
                    if self.active == 0 {
                        for w in &mut self.windings[1..] {
                            w.kva = v;
                        }
                        self.ratings();
                    } else if self.windings.len() == 2 {
                        self.windings[0].kva = v;
                    }
                } else if key == "%r" {
                    self.load_loss();
                }
            }
            "conns" => {
                let items = p.value.to_string_list(Some(&raw.vars));
                if items.len() != self.windings.len() {
                    return Err("XfmrCode connection array has wrong length".into());
                }
                for (w, v) in self.windings.iter_mut().zip(items) {
                    w.delta = connection(&v)?;
                }
            }
            "kvs" | "kvas" | "taps" | "%rs" | "xscarray" => self.apply_array(key, p, raw)?,
            "xhl" | "xht" | "xlt" | "x12" | "x13" | "x23" => {
                let i = match key {
                    "xhl" | "x12" => 0,
                    "xht" | "x13" => 1,
                    _ => 2,
                };
                self.x[i] = number()?;
                self.changed = true;
            }
            "%loadloss" => {
                let v = number()?;
                self.windings[0].r = v / 2.0;
                self.windings[1].r = v / 2.0;
                self.extras.insert(key.into(), v.to_string());
            }
            "thermal" | "n" | "m" | "flrise" | "hsrise" | "%noloadloss" | "%imag"
            | "ppm_antifloat" | "normhkva" | "emerghkva" => {
                self.extras.insert(key.into(), number()?.to_string());
            }
            // These definition properties do not change the AC primitive or
            // fixed tap. The original code object remains retained metadata.
            "maxtap" | "mintap" | "numtaps" | "rdcohms" | "seasons" | "ratings" => {}
            _ => return Err(format!("unqualified XfmrCode property {key}")),
        }
        Ok(())
    }
}

fn set_number(w: &mut Winding, key: &str, value: f64) {
    match key {
        "kv" => w.kv = value,
        "kva" => w.kva = value,
        "tap" => w.tap = value,
        "%r" => w.r = value,
        "rneut" => w.rneut = value,
        "xneut" => w.xneut = value,
        _ => unreachable!(),
    }
}

fn connection(text: &str) -> Result<bool, String> {
    let text = text.to_ascii_lowercase();
    if text == "y" || text == "ln" || (!text.is_empty() && "wye".starts_with(&text)) {
        Ok(false)
    } else if text == "ll" || (!text.is_empty() && "delta".starts_with(&text)) {
        Ok(true)
    } else {
        Err("invalid XfmrCode winding connection".into())
    }
}

pub(super) fn resolve(raw: &RawDss, point: Snapshot) -> Result<State, String> {
    let mut cache = HashMap::new();
    let mut pending = vec![(point, false)];
    let mut visiting = HashSet::new();
    let mut budget = 1_usize << 22;
    while let Some((point, ready)) = pending.pop() {
        if cache.contains_key(&point) {
            continue;
        }
        let history = raw
            .line_history
            .get(&point.object)
            .ok_or("missing XfmrCode history")?;
        if raw.objects[point.object].class != "xfmrcode" {
            return Err("expected XfmrCode history".into());
        }
        let copies = history
            .copies
            .get(..point.copies)
            .ok_or("invalid XfmrCode copy prefix")?;
        if ready {
            let state = replay(raw, point, &cache, &mut budget)?;
            visiting.remove(&point);
            cache.insert(point, state);
        } else {
            if !visiting.insert(point) {
                return Err("cyclic XfmrCode history".into());
            }
            charge(&mut budget, 1)?;
            pending.push((point, true));
            for copy in copies.iter().rev() {
                pending.push((copy.source, false));
            }
        }
    }
    cache
        .remove(&point)
        .ok_or_else(|| "missing resolved XfmrCode".into())
}

fn charge(budget: &mut usize, amount: usize) -> Result<(), String> {
    *budget = budget
        .checked_sub(amount)
        .ok_or("XfmrCode evaluation work limit exceeded")?;
    Ok(())
}

fn replay(
    raw: &RawDss,
    point: Snapshot,
    cache: &HashMap<Snapshot, State>,
    budget: &mut usize,
) -> Result<State, String> {
    let object = &raw.objects[point.object];
    let props = object
        .props
        .get(..point.properties)
        .ok_or("invalid XfmrCode property prefix")?;
    let edits = object
        .edits
        .get(..point.edits)
        .ok_or("invalid XfmrCode edit prefix")?;
    let copies = &raw.line_history[&point.object].copies[..point.copies];
    let mut state = State::default();
    let (mut property, mut edit, mut copy) = (0, 0, 0);
    loop {
        let next = copies.get(copy);
        charge(budget, state.windings.len() * state.windings.len() + 1)?;
        if edit < edits.len() && edits[edit] == property && next.is_none_or(|c| edit < c.edit_start)
        {
            state.finish_edit();
            edit += 1;
        } else if let Some(c) = next.filter(|c| c.start == property && c.edit_start == edit) {
            state.make_like(cache.get(&c.source).ok_or("unresolved XfmrCode copy")?);
            if c.source.object == point.object {
                // MakeLike(self) reallocates its own winding records before
                // copying them, so the native source records are defaults.
                state.windings.fill(Winding::default());
            }
            property = c.end;
            edit = c.edit_end;
            copy += 1;
        } else if property < props.len() {
            state.apply(&props[property], raw)?;
            property += 1;
        } else {
            break;
        }
    }
    if edit != edits.len() || copy != copies.len() {
        return Err("unconsumed XfmrCode history".into());
    }
    Ok(state)
}
