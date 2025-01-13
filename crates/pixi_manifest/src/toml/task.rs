use pixi_toml::{OneOrMany, TomlFromStr, TomlIndexMap, TomlWith};
use toml_span::{
    de_helpers::{expected, TableHelper},
    value::ValueInner,
    DeserError, Value,
};

use crate::{
    task::{Alias, CmdArgs, Execute},
    Task, TaskName,
};

impl<'de> toml_span::Deserialize<'de> for Task {
    fn deserialize(value: &mut toml_span::Value<'de>) -> Result<Self, DeserError> {
        let mut th = match value.take() {
            ValueInner::String(str) => return Ok(Task::Plain(str.into_owned())),
            ValueInner::Table(table) => TableHelper::from((table, value.span)),
            inner => return Err(expected("string or table", inner, value.span).into()),
        };

        let cmd = th.optional("cmd");

        if let Some(cmd) = cmd {
            let inputs = th.optional("inputs");
            let outputs = th.optional("outputs");
            let depends_on = th
                .optional::<TomlWith<_, OneOrMany<TomlFromStr<_>>>>("depends-on")
                .map(TomlWith::into_inner)
                .unwrap_or_default();
            let cwd = th
                .optional::<TomlFromStr<_>>("cwd")
                .map(TomlFromStr::into_inner);
            let env = th
                .optional::<TomlIndexMap<_, _>>("env")
                .map(TomlIndexMap::into_inner);
            let description = th.optional("description");
            let clean_env = th.optional("clean-env").unwrap_or(false);

            // Deprecated fields
            let deprecated_depends_on = th.table.remove_entry("depends_on");

            th.finalize(None)?;

            if let Some((depends_on, _)) = deprecated_depends_on {
                return Err(DeserError::from(toml_span::Error {
                    kind: toml_span::ErrorKind::Deprecated {
                        old: "depends_on",
                        new: "depends-on",
                    },
                    span: depends_on.span,
                    line_info: None,
                }));
            }

            Ok(Self::Execute(Execute {
                cmd,
                inputs,
                outputs,
                depends_on,
                cwd,
                env,
                description,
                clean_env,
            }))
        } else {
            let depends_on = th
                .optional::<TomlWith<_, Vec<TomlFromStr<_>>>>("depends-on")
                .map(TomlWith::into_inner)
                .unwrap_or_default();
            let description = th.optional("description");
            th.finalize(None)?;

            Ok(Self::Alias(Alias {
                depends_on,
                description,
            }))
        }
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
    use insta::assert_snapshot;

    use super::*;
    use crate::{toml::FromTomlStr, utils::test_utils::format_parse_error};

    #[test]
    fn test_depends_on_deprecation() {
        let input = r#"
        cmd = "test"
        depends_on = ["a", "b"]
        "#;

        let result = Task::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, result));
    }
}
