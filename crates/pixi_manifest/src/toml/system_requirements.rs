use std::str::FromStr;

use pixi_toml::TomlFromStr;
use rattler_conda_types::Version;
use toml_span::{
    DeserError, Error, ErrorKind, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::{LibCSystemRequirement, SystemRequirements, system_requirements::LibCFamilyAndVersion};

impl<'de> toml_span::Deserialize<'de> for SystemRequirements {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let macos = th
            .optional::<TomlFromStr<_>>("macos")
            .map(TomlFromStr::into_inner);
        let linux = th
            .optional::<TomlFromStr<_>>("linux")
            .map(TomlFromStr::into_inner);
        let cuda = th
            .optional::<TomlFromStr<_>>("cuda")
            .map(TomlFromStr::into_inner);
        let libc = th.optional("libc");
        let archspec = th.optional("archspec");

        th.finalize(None)?;

        Ok(Self {
            macos,
            linux,
            cuda,
            archspec,
            libc,
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for LibCSystemRequirement {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(str) => {
                let version = Version::from_str(&str).map_err(|e| Error {
                    kind: ErrorKind::Custom(e.to_string().into()),
                    span: value.span,
                    line_info: None,
                })?;
                Ok(LibCSystemRequirement::GlibC(version))
            }
            ValueInner::Table(table) => {
                let mut th = TableHelper::from((table, value.span));

                let family = th.optional("family");
                let version = th.required::<TomlFromStr<_>>("version")?.into_inner();

                th.finalize(None)?;
                Ok(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                    family,
                    version,
                }))
            }
            inner => Err(expected("a string or table", inner, value.span).into()),
        }
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;
    use rattler_virtual_packages::{Cuda, LibC, Linux, Osx, VirtualPackage};

    use super::*;
    use crate::{toml::FromTomlStr, utils::test_utils::format_parse_error};

    #[test]
    fn system_requirements_works() {
        let file_content = r#"
        linux = "5.11"
        cuda = "12.2"
        macos = "10.15"
        libc = { family = "glibc", version = "2.12" }
        "#;

        let system_requirements: SystemRequirements =
            SystemRequirements::from_toml_str(file_content).unwrap();

        let expected_requirements: Vec<VirtualPackage> = vec![
            VirtualPackage::Linux(Linux {
                version: Version::from_str("5.11").unwrap(),
            }),
            VirtualPackage::Cuda(Cuda {
                version: Version::from_str("12.2").unwrap(),
            }),
            VirtualPackage::Osx(Osx {
                version: Version::from_str("10.15").unwrap(),
            }),
            VirtualPackage::LibC(LibC {
                version: Version::from_str("2.12").unwrap(),
                family: "glibc".to_string(),
            }),
        ];

        assert_eq!(
            system_requirements.virtual_packages(),
            expected_requirements
        );
    }

    #[test]
    fn test_version_misspelled() {
        let input = r#"
        libc = { veion = "2.12" }
        "#;

        let result = SystemRequirements::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, result));
    }

    #[test]
    fn test_unknown_key() {
        let input = r#"
        lib = "2.12"
        "#;

        let result = SystemRequirements::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, result));
    }

    #[test]
    fn test_family_misspelled() {
        let input = r#"
        [libc]
        version = "2.12"
        fam = "glibc"
        "#;

        let result = SystemRequirements::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, result));
    }

    #[test]
    fn test_libc_misspelled() {
        let input = r#"
        [lic]
        version = "2.12"
        family = "glibc"
        "#;

        let result = SystemRequirements::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, result));
    }
}
