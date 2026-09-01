use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::validation::valid_nonempty_text;

/// Stable identity of one component in a PowerIO value.
///
/// The component type qualifies the source supplied or PowerIO assigned local
/// identity. This keeps, for example, a load named `main` distinct from a
/// generator named `main` without exposing a table row position.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ComponentId {
    component_type: Box<str>,
    local_id: Box<str>,
}

impl ComponentId {
    /// Construct a component identity from its structural type and local
    /// identity.
    ///
    /// # Errors
    /// Either part is empty, contains NUL, or exceeds the common identifier
    /// bound.
    pub fn new(
        component_type: impl Into<String>,
        local_id: impl Into<String>,
    ) -> Result<Self, Error> {
        let component_type = component_type.into();
        let local_id = local_id.into();
        if !valid_nonempty_text(&component_type) || !valid_nonempty_text(&local_id) {
            return Err(Error::new(
                &crate::codes::VALIDATE_COMPONENT_INVALID_ID,
                "a component type and local identity must both be nonempty and bounded",
            ));
        }
        Ok(Self {
            component_type: component_type.into_boxed_str(),
            local_id: local_id.into_boxed_str(),
        })
    }

    /// The component's structural type, such as `load` or `switch`.
    #[must_use]
    pub fn component_type(&self) -> &str {
        &self.component_type
    }

    /// The identity within that component type.
    #[must_use]
    pub fn local_id(&self) -> &str {
        &self.local_id
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.component_type, self.local_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_type_qualified() {
        let load = ComponentId::new("load", "main").unwrap();
        let generator = ComponentId::new("generator", "main").unwrap();
        assert_ne!(load, generator);
        assert_eq!(load.component_type(), "load");
        assert_eq!(load.local_id(), "main");
        assert_eq!(load.to_string(), "load/main");
    }

    #[test]
    fn invalid_parts_are_rejected() {
        assert!(ComponentId::new("", "main").is_err());
        assert!(ComponentId::new("load", "").is_err());
        assert!(ComponentId::new("load", "bad\0id").is_err());
    }
}
