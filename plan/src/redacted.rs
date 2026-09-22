// SPDX-License-Identifier: GPL-3.0-or-later

//! A string wrapper that serializes normally (it has to cross the UI/backend
//! socket) but never prints its contents through `Debug`, `Display`, or
//! `schemars`. Used for the user's password so it can't leak into logs,
//! error messages, or test snapshots by accident.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Redacted(String);

impl Redacted {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Redacted(\"[REDACTED]\")")
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl JsonSchema for Redacted {
    fn schema_name() -> String {
        "Redacted".to_string()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        String::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_never_show_the_value() {
        let secret = Redacted::new("hunter2");
        assert_eq!(format!("{secret:?}"), "Redacted(\"[REDACTED]\")");
        assert_eq!(format!("{secret}"), "[REDACTED]");
    }

    #[test]
    fn serializes_to_the_real_value_for_the_wire_protocol() {
        let secret = Redacted::new("hunter2");
        assert_eq!(serde_json::to_string(&secret).unwrap(), "\"hunter2\"");
    }

    #[test]
    fn expose_returns_the_real_value() {
        let secret = Redacted::new("hunter2");
        assert_eq!(secret.expose(), "hunter2");
    }
}
