use toml_span::{DeserError, Value, de_helpers::TableHelper};

/// Options for `pixi audit`, from the `[workspace.audit]` table.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AuditOptions {
    /// Vulnerability ids or aliases (e.g. CVE or GHSA ids) to suppress.
    pub ignore: Vec<String>,
}

impl<'de> toml_span::Deserialize<'de> for AuditOptions {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let ignore = th.optional("ignore").unwrap_or_default();
        th.finalize(None)?;
        Ok(AuditOptions { ignore })
    }
}
