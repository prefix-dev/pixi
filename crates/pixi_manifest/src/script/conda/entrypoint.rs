use std::str::FromStr;

use itertools::Itertools;
use pixi_toml::custom_error;
use rattler_conda_types::Platform;
use toml_span::{DeserError, Value, de_helpers::expected, value::ValueInner};

/// The command that runs a `conda-script` file.
#[derive(Debug, Clone)]
pub enum Entrypoint {
    /// One command for every platform.
    Uniform(String),
    /// A command per platform selector.
    PerPlatform(Vec<(EntrypointSelector, String)>),
}

/// A platform key of an entrypoint table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrypointSelector {
    /// Any Unix platform.
    Unix,
    /// Any Linux platform.
    Linux,
    /// Any macOS platform.
    Osx,
    /// Any Windows platform.
    Win,
    /// One specific conda platform.
    Platform(Platform),
}

impl Entrypoint {
    /// The command for `platform`, taking the most specific matching key:
    /// the exact platform wins over its family (`linux`, `osx`, `win`),
    /// which wins over `unix`. Returns `None` when no key matches.
    pub fn select(&self, platform: Platform) -> Option<&str> {
        match self {
            Entrypoint::Uniform(command) => Some(command),
            Entrypoint::PerPlatform(commands) => {
                let lookup = |selector: EntrypointSelector| {
                    commands.iter().find_map(|(candidate, command)| {
                        (*candidate == selector).then_some(command.as_str())
                    })
                };
                lookup(EntrypointSelector::Platform(platform))
                    .or_else(|| {
                        let family = if platform.is_linux() {
                            EntrypointSelector::Linux
                        } else if platform.is_osx() {
                            EntrypointSelector::Osx
                        } else if platform.is_windows() {
                            EntrypointSelector::Win
                        } else {
                            return None;
                        };
                        lookup(family)
                    })
                    .or_else(|| {
                        platform
                            .is_unix()
                            .then(|| lookup(EntrypointSelector::Unix))
                            .flatten()
                    })
            }
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for Entrypoint {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let span = value.span;
        match value.take() {
            ValueInner::String(command) => Ok(Entrypoint::Uniform(command.into_owned())),
            ValueInner::Table(table) => {
                if table.is_empty() {
                    return Err(custom_error(
                        "the entrypoint table must contain at least one platform key",
                        span,
                    )
                    .into());
                }
                let mut errors = DeserError { errors: Vec::new() };
                let mut commands = Vec::new();
                for (key, mut command) in table.into_iter().sorted_by_key(|(key, _)| key.span.start)
                {
                    let selector = match key.name.as_ref() {
                        "unix" => Some(EntrypointSelector::Unix),
                        "linux" => Some(EntrypointSelector::Linux),
                        "osx" => Some(EntrypointSelector::Osx),
                        "win" => Some(EntrypointSelector::Win),
                        name => match Platform::from_str(name) {
                            Ok(platform) => Some(EntrypointSelector::Platform(platform)),
                            Err(_) => {
                                errors.errors.push(custom_error(
                                    format!(
                                        "'{name}' is neither a platform family (`unix`, `linux`, `osx`, `win`) nor a conda platform"
                                    ),
                                    key.span,
                                ));
                                None
                            }
                        },
                    };
                    let command = match command.take() {
                        ValueInner::String(command) => Some(command.into_owned()),
                        inner => {
                            errors
                                .errors
                                .push(expected("a string", inner, command.span));
                            None
                        }
                    };
                    if let (Some(selector), Some(command)) = (selector, command) {
                        commands.push((selector, command));
                    }
                }
                if errors.errors.is_empty() {
                    Ok(Entrypoint::PerPlatform(commands))
                } else {
                    Err(errors)
                }
            }
            inner => Err(expected("a string or a table of platforms", inner, span).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use toml_span::de_helpers::TableHelper;

    use super::*;

    fn parse_entrypoint(toml: &str) -> Entrypoint {
        let mut value = toml_span::parse(toml).unwrap();
        let mut th = TableHelper::new(&mut value).unwrap();
        let entrypoint = th.required::<Entrypoint>("entrypoint").unwrap();
        th.finalize(None).unwrap();
        entrypoint
    }

    #[test]
    fn a_uniform_entrypoint_matches_every_platform() {
        let entrypoint = parse_entrypoint(r#"entrypoint = "python ${SCRIPT}""#);
        assert_eq!(
            entrypoint.select(Platform::Linux64),
            Some("python ${SCRIPT}")
        );
        assert_eq!(entrypoint.select(Platform::Win64), Some("python ${SCRIPT}"));
    }

    #[test]
    fn the_most_specific_platform_key_wins() {
        let entrypoint = parse_entrypoint(
            r#"entrypoint = { unix = "unix", linux = "linux", linux-64 = "linux-64", win = "win" }"#,
        );
        assert_eq!(entrypoint.select(Platform::Linux64), Some("linux-64"));
        assert_eq!(entrypoint.select(Platform::LinuxAarch64), Some("linux"));
        assert_eq!(entrypoint.select(Platform::Osx64), Some("unix"));
        assert_eq!(entrypoint.select(Platform::Win64), Some("win"));
        assert_eq!(entrypoint.select(Platform::WinArm64), Some("win"));
    }

    #[test]
    fn a_platform_without_a_matching_key_selects_nothing() {
        let windows_only = parse_entrypoint(r#"entrypoint = { win = "win" }"#);
        assert_eq!(windows_only.select(Platform::Linux64), None);

        let unix_only = parse_entrypoint(r#"entrypoint = { unix = "unix" }"#);
        assert_eq!(unix_only.select(Platform::Win64), None);
        assert_eq!(unix_only.select(Platform::LinuxRiscv64), Some("unix"));
    }
}
