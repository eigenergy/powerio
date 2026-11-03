//! Apply static loadshape effects at the reference assignment, not at EOF.
use super::{C, Executor, Loader, MAX_TOTAL_INCLUDE_BYTES, RawDss, RawProp, VarMap};

impl RawDss {
    pub(in crate::dss) fn load_power(
        &self,
        name: &str,
    ) -> Option<&super::super::load_power::LoadPower> {
        self.index
            .get(&("load".into(), name.to_ascii_lowercase()))
            .and_then(|index| self.load_powers.get(index))
    }
}

impl<L: Loader> Executor<'_, L> {
    pub(super) fn static_load_property(
        &mut self,
        index: usize,
        prop: &RawProp,
        vars: Option<&VarMap>,
        ctx: &dyn Fn(String) -> String,
    ) {
        let key = prop.name.as_deref().unwrap_or("");
        match self.raw.objects[index].class.as_str() {
            "loadshape" => {
                let mut shape = self.raw.load_shapes.remove(&index).unwrap_or_default();
                shape.property(key, &prop.value, vars, |name| self.profile_bytes(name, ctx));
                self.raw.load_shapes.insert(index, shape);
            }
            "load" => {
                if matches!(key, "daily" | "yearly" | "duty") {
                    // Native LoadShape.Find never binds the reserved none name.
                    if prop.value.text.eq_ignore_ascii_case("none") {
                        return;
                    }
                    let shape = self
                        .raw
                        .index
                        .get(&("loadshape".into(), prop.value.text.to_ascii_lowercase()))
                        .and_then(|index| self.raw.load_shapes.get(index));
                    if let Some(shape) = shape.filter(|shape| shape.use_actual) {
                        match shape.actual_power() {
                            Ok((p, q)) => {
                                self.raw.load_powers.entry(index).or_default().actual(p, q);
                            }
                            Err(error) => self.raw.warn(
                                &C::READ_DSS_LOADSHAPE_UNRESOLVED,
                                ctx(format!(
                                    "load {}: UseActual {key}={} cannot be materialized: {error}",
                                    self.raw.objects[index].name, prop.value.text
                                )),
                            ),
                        }
                    }
                } else {
                    self.raw
                        .load_powers
                        .entry(index)
                        .or_default()
                        .property(key, &prop.value, vars);
                }
            }
            _ => {}
        }
    }

    fn profile_bytes(
        &mut self,
        name: &str,
        ctx: &dyn Fn(String) -> String,
    ) -> Result<Vec<u8>, String> {
        let path = self
            .resolve_or_warn("loadshape", name, ctx)
            .ok_or("profile escapes acquisition root")?;
        if !self.charge_include("loadshape", &path, ctx) {
            return Err("profile acquisition budget exhausted".into());
        }
        let bytes = self.loader.load_bytes(&path).map_err(|error| {
            self.warn_load_error("loadshape", &path, &error, ctx);
            error.to_string()
        })?;
        self.include_bytes = self.include_bytes.saturating_add(bytes.len());
        if self.include_bytes > MAX_TOTAL_INCLUDE_BYTES {
            return Err("profile byte budget exhausted".into());
        }
        Ok(bytes)
    }

    pub(super) fn finish_static_load(&mut self, index: usize) {
        if let Some(power) = self.raw.load_powers.get_mut(&index) {
            power.finish();
        }
        if let Some(shape) = self.raw.load_shapes.get_mut(&index) {
            shape.finish();
        }
    }

    pub(super) fn copy_static_load(&mut self, target: usize, source: usize) {
        if let Some(power) = self.raw.load_powers.get(&source).cloned() {
            self.raw
                .load_powers
                .entry(target)
                .or_default()
                .make_like(&power);
        }
        if let Some(shape) = self.raw.load_shapes.get(&source).cloned() {
            self.raw
                .load_shapes
                .entry(target)
                .or_default()
                .make_like(&shape);
        }
    }
}
