use std::str::FromStr;

use pixi_toml::{TomlFromStr, TomlIndexMap};
use toml_span::{
    DeserError, ErrorKind, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::{
    EnvironmentName, Task, TaskName, WithWarnings,
    task::{Alias, ArgName, CmdArgs, Dependency, Execute, GlobPatterns, TaskArg, TemplateString},
    warning::Deprecation,
};

impl<'de> toml_span::Deserialize<'de> for TemplateString {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        Ok(TemplateString::new(value.take_string(None)?.into_owned()))
    }
}

impl<'de> toml_span::Deserialize<'de> for GlobPatterns {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::Array(array) => {
                let mut args: Vec<TemplateString> = Vec::with_capacity(array.len());
                for mut value in array {
                    args.push(TemplateString::new(value.take_string(None)?.into_owned()));
                }
                Ok(GlobPatterns::new(args))
            }
            _ => Err(expected(
                "an array of args e.g. [\"main.py\", \"tests_*\"]",
                value.take(),
                value.span,
            )
            .into()),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for TaskArg {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = match value.take() {
            ValueInner::String(str) => {
                let name = ArgName::from_str(&str).map_err(|e| {
                    DeserError::from(toml_span::Error {
                        kind: ErrorKind::Custom(e.to_string().into()),
                        span: value.span,
                        line_info: None,
                    })
                })?;
                return Ok(TaskArg {
                    name,
                    default: None,
                });
            }
            ValueInner::Table(table) => TableHelper::from((table, value.span)),
            inner => return Err(expected("string or table", inner, value.span).into()),
        };

        let name = th.required::<TomlFromStr<ArgName>>("arg")?.into_inner();
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
            ValueInner::String(str) => return Ok(Task::Plain(str.into_owned().into()).into()),
            ValueInner::Table(table) => TableHelper::from((table, value.span)),
            ValueInner::Array(array) => {
                let mut deps = Vec::new();
                for mut item in array {
                    match item.take() {
                        ValueInner::Table(table) => {
                            let mut th = TableHelper::from((table, item.span));
                            let name = th.required::<String>("task")?;
                            let args = th.optional::<Vec<TemplateString>>("args");
                            let environment = th
                                .optional::<TomlFromStr<EnvironmentName>>("environment")
                                .map(TomlFromStr::into_inner);

                            deps.push(Dependency::new(&name, args, environment));
                        }
                        ValueInner::String(str) => {
                            deps.push(Dependency::from(str.as_ref()));
                        }
                        value => return Err(expected("table", value, item.span).into()),
                    }
                }
                return Ok(Task::Alias(Alias {
                    depends_on: deps,
                    description: None,
                    args: None,
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
                                    let args = th.optional::<Vec<TemplateString>>("args");
                                    let environment = th
                                        .optional::<TomlFromStr<EnvironmentName>>("environment")
                                        .map(TomlFromStr::into_inner);

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
                                    let args = th.optional::<Vec<TemplateString>>("args");
                                    let environment = th
                                        .optional::<TomlFromStr<EnvironmentName>>("environment")
                                        .map(TomlFromStr::into_inner);
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
            let depends_on = depends_on(&mut th)?;
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
            for arg in args.iter().flat_map(|a| a.iter()) {
                if arg.default.is_some() {
                    have_default = true;
                }
                if have_default && arg.default.is_none() {
                    return Err(DeserError::from(toml_span::Error {
                        kind: ErrorKind::Custom(
                            "default value required after previous arguments with defaults".into(),
                        ),
                        span: value.span,
                        line_info: None,
                    }));
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
                args,
            }))
        } else {
            let depends_on = depends_on(&mut th)?;
            let description = th.optional("description");
            let args = th.optional::<Vec<TaskArg>>("args");
            th.finalize(None)?;

            Task::Alias(Alias {
                depends_on,
                description,
                args,
            })
        };

        Ok(WithWarnings::from(task).with_warnings(warnings))
    }
}

impl<'de> toml_span::Deserialize<'de> for CmdArgs {
    fn deserialize(value: &mut toml_span::Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(str) => Ok(CmdArgs::Single(str.into_owned().into())),
            ValueInner::Array(arr) => {
                let mut args = Vec::with_capacity(arr.len());
                for mut item in arr {
                    args.push(item.take_string(None)?.into_owned());
                }
                Ok(CmdArgs::Multiple(
                    args.into_iter().map(|s| s.into()).collect(),
                ))
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
    use crate::toml::FromTomlStr;
    use pixi_test_utils::format_parse_error;

    fn expect_parse_failure(pixi_toml: &str) -> String {
        let parse_error = <TomlTask as crate::toml::FromTomlStr>::from_toml_str(pixi_toml)
            .expect_err("parsing should fail");

        format_parse_error(pixi_toml, parse_error)
    }

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

    #[test]
    fn test_additional_task_keys() {
        insta::assert_snapshot!(expect_parse_failure(
            r#"
            cmd = "test"
            depends = ["a", "b"]
        "#
        ));
    }

    #[test]
    fn test_depends_on_is_list() {
        insta::assert_snapshot!(expect_parse_failure(
            r#"
            cmd = "test"
            depends-on = { task = "z" }
        "#
        ));
    }
}
