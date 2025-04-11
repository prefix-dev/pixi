use std::str::FromStr;

use pixi_toml::{TomlFromStr, TomlIndexMap};
use toml_span::{
    de_helpers::{expected, TableHelper},
    value::ValueInner,
    DeserError, Value,
};

use crate::{
    task::{Alias, CmdArgs, Dependency, Execute, TaskArg},
    warning::Deprecation,
    EnvironmentName, Task, TaskName, WithWarnings,
};

impl<'de> toml_span::Deserialize<'de> for TaskArg {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = match value.take() {
            ValueInner::String(str) => {
                return Ok(TaskArg {
                    name: str.into_owned(),
                    default: None,
                })
            }
            ValueInner::Table(table) => TableHelper::from((table, value.span)),
            inner => return Err(expected("string or table", inner, value.span).into()),
        };

        let name = th.required::<String>("arg")?;
        let default = th.optional::<String>("default");

        th.finalize(None)?;

        Ok(TaskArg { name, default })
    }
}

/// A task defined in the manifest.
pub type TomlTask = WithWarnings<Task>;

impl<'de> toml_span::Deserialize<'de> for TomlTask {
    fn deserialize(value: &mut toml_span::Value<'de>) -> Result<Self, DeserError> {
        let mut th = match value.take() {
            ValueInner::String(str) => return Ok(Task::Plain(str.into_owned()).into()),
            ValueInner::Table(table) => TableHelper::from((table, value.span)),
            ValueInner::Array(array) => {
                let mut deps = Vec::new();
                for mut item in array {
                    match item.take() {
                        ValueInner::Table(table) => {
                            let mut th = TableHelper::from((table, item.span));
                            let name = th.required::<String>("task")?;
                            let args = th.optional::<Vec<String>>("args");
                            let environment = th
                                .optional::<String>("environment")
                                .map(|env| EnvironmentName::from_str(&env))
                                .transpose()
                                .map_err(|e| {
                                    DeserError::from(expected(
                                        "valid environment name",
                                        ValueInner::String(e.attempted_parse.into()),
                                        item.span,
                                    ))
                                })?;

                            deps.push(Dependency::new(&name, args, environment));
                        }
                        _ => return Err(expected("table", item.take(), item.span).into()),
                    }
                }
                return Ok(Task::Alias(Alias {
                    depends_on: deps,
                    description: None,
                })
                .into());
            }
            inner => return Err(expected("string or table", inner, value.span).into()),
        };

        let cmd = th.optional("cmd");
        let mut warnings = Vec::new();

        let mut depends_on = |th: &mut TableHelper| {
            let mut depends_on = th.take("depends-on");
            if let Some((_, mut value)) = depends_on.take() {
                let deps = match value.take() {
                    ValueInner::Array(array) => array
                        .into_iter()
                        .map(|mut item| {
                            let span = item.span;
                            match item.take() {
                                ValueInner::String(str) => Ok::<Dependency, DeserError>(
                                    Dependency::new(str.as_ref(), None, None),
                                ),
                                ValueInner::Table(table) => {
                                    let mut th = TableHelper::from((table, span));
                                    let name = th.required::<String>("task")?;
                                    let args = th.optional::<Vec<String>>("args");
                                    let environment = th
                                        .optional::<String>("environment")
                                        .map(|env| EnvironmentName::from_str(&env))
                                        .transpose()
                                        .map_err(|e| {
                                            DeserError::from(expected(
                                                "valid environment name",
                                                ValueInner::String(e.attempted_parse.into()),
                                                span,
                                            ))
                                        })?;

                                    Ok(Dependency::new(&name, args, environment))
                                }
                                inner => Err(expected("string or table", inner, span).into()),
                            }
                        })
                        .collect::<Result<Vec<Dependency>, DeserError>>()?,
                    ValueInner::String(str) => Vec::from([Dependency::from(str.as_ref())]),
                    inner => {
                        return Err::<Vec<Dependency>, DeserError>(
                            expected("string or array", inner, value.span).into(),
                        );
                    }
                };

                return Ok(deps);
            }

            if let Some((key, mut value)) = th.table.remove_entry("depends_on") {
                warnings
                    .push(Deprecation::renamed_field("depends_on", "depends-on", key.span).into());
                let deps = match value.take() {
                    ValueInner::Array(array) => array
                        .into_iter()
                        .map(|mut item| {
                            let span = item.span;
                            match item.take() {
                                ValueInner::String(str) => {
                                    Ok::<Dependency, DeserError>(Dependency::from(str.as_ref()))
                                }
                                ValueInner::Table(table) => {
                                    let mut th = TableHelper::from((table, span));
                                    let name = th.required::<String>("task")?;
                                    let args = th.optional::<Vec<String>>("args");
                                    let environment = th
                                        .optional::<String>("environment")
                                        .map(|env| EnvironmentName::from_str(&env))
                                        .transpose()
                                        .map_err(|e| {
                                            DeserError::from(expected(
                                                "valid environment name",
                                                ValueInner::String(e.attempted_parse.into()),
                                                span,
                                            ))
                                        })?;
                                    // If the creating a new dependency fails, it means the environment name is invalid and exists hence we can safely unwrap the environment
                                    Ok(Dependency::new(&name, args, environment))
                                }
                                inner => Err(expected("string or table", inner, span).into()),
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    ValueInner::String(str) => Vec::from([Dependency::from(str.as_ref())]),
                    inner => return Err(expected("string or array", inner, value.span).into()),
                };

                return Ok(deps);
            }

            Ok(vec![])
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
            let args = th.optional::<Vec<TaskArg>>("args");

            let mut have_default = false;
            for arg in args.as_ref().unwrap_or(&vec![]) {
                if arg.default.is_some() {
                    have_default = true;
                }
                if have_default && arg.default.is_none() {
                    return Err(expected(
                        "default value required after previous arguments with defaults",
                        ValueInner::Table(Default::default()),
                        value.span,
                    )
                    .into());
                }
            }
            th.finalize(None)?;

            Task::Execute(Box::new(Execute {
                cmd,
                inputs,
                outputs,
                depends_on,
                cwd,
                env,
                description,
                clean_env,
                args: args.map(|args| args.into_iter().map(|arg| (arg, None)).collect()),
            }))
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
