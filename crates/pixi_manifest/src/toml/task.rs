use pixi_toml::{OneOrMany, TomlFromStr, TomlIndexMap, TomlWith};
use toml_span::{
    de_helpers::{expected, TableHelper},
    value::ValueInner,
    DeserError, Value,
};

use crate::{
    task::{Alias, CmdArgs, Execute},
    warning::Deprecation,
    Task, TaskName, WithWarnings,
};

/// A task defined in the manifest.
pub type TomlTask = WithWarnings<Task>;

impl<'de> toml_span::Deserialize<'de> for TomlTask {
    fn deserialize(value: &mut toml_span::Value<'de>) -> Result<Self, DeserError> {
        let mut th = match value.take() {
            ValueInner::String(str) => return Ok(Task::Plain(str.into_owned()).into()),
            ValueInner::Table(table) => TableHelper::from((table, value.span)),
            inner => return Err(expected("string or table", inner, value.span).into()),
        };

        let cmd = th.optional("cmd");
        let mut warnings = Vec::new();

        let mut depends_on = |th: &mut TableHelper| {
            let depends_on = th.optional::<TomlWith<_, OneOrMany<TomlFromStr<_>>>>("depends-on");
            if let Some(depends_on) = depends_on {
                return Some(depends_on.into_inner());
            }

            if let Some((key, mut value)) = th.table.remove_entry("depends_on") {
                warnings
                    .push(Deprecation::renamed_field("depends_on", "depends-on", key.span).into());
                return match TomlWith::<_, OneOrMany<TomlFromStr<_>>>::deserialize(&mut value) {
                    Ok(depends_on) => Some(depends_on.into_inner()),
                    Err(err) => {
                        th.errors.extend(err.errors);
                        None
                    }
                };
            }

            None
        };

        let task = if let Some(cmd) = cmd {
            let inputs = th.optional("inputs");
            let outputs = th.optional("outputs");
            let depends_on = depends_on(&mut th).unwrap_or_default();
            let cwd = th
                .optional::<TomlFromStr<_>>("cwd")
                .map(TomlFromStr::into_inner);
            let env = th
                .optional::<TomlIndexMap<_, _>>("env")
                .map(TomlIndexMap::into_inner);
            let description = th.optional("description");
            let clean_env = th.optional("clean-env").unwrap_or(false);

            th.finalize(None)?;

            Task::Execute(Execute {
                cmd,
                inputs,
                outputs,
                depends_on,
                cwd,
                env,
                description,
                clean_env,
            })
        } else {
            let depends_on = depends_on(&mut th).unwrap_or_default();
            let description = th.optional("description");
            th.finalize(None)?;

            Task::Alias(Alias {
                depends_on,
                description,
            })
        };

        Ok(WithWarnings::from(task).with_warnings(warnings))
    }
}

impl<'de> toml_span::Deserialize<'de> for CmdArgs {
    fn deserialize(value: &mut toml_span::Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(str) => Ok(CmdArgs::Single(str.into_owned())),
            ValueInner::Array(arr) => {
                let mut args = Vec::with_capacity(arr.len());
                for mut item in arr {
                    args.push(item.take_string(None)?.into_owned());
                }
                Ok(CmdArgs::Multiple(args))
            }
            inner => Err(expected("string or array", inner, value.span).into()),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for TaskName {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        TomlFromStr::deserialize(value).map(TomlFromStr::into_inner)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{toml::FromTomlStr, utils::test_utils::format_parse_error};

    #[test]
    fn test_depends_on_deprecation() {
        let input = r#"
        cmd = "test"
        depends_on = ["a", "b"]
        "#;

        let mut parsed = TomlTask::from_toml_str(input).unwrap();
        assert_eq!(parsed.warnings.len(), 1);
        insta::assert_snapshot!(format_parse_error(input, parsed.warnings.remove(0)));
    }
}
