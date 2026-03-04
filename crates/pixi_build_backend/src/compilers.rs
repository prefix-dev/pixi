//! We could expose the `default_compiler` function from the `rattler-build`
//! crate

use std::{collections::HashSet, fmt::Display, ops::Deref};

use itertools::Itertools;
use rattler_build::NormalizedKey;
use rattler_conda_types::Platform;
use recipe_stage0::{
    matchspec::PackageDependency,
    recipe::{Item, Value},
};

pub enum Language<'a> {
    C,
    Cxx,
    Fortran,
    Rust,
    Other(&'a str),
}

impl Display for Language<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::C => write!(f, "c"),
            Language::Cxx => write!(f, "cxx"),
            Language::Fortran => write!(f, "fortran"),
            Language::Rust => write!(f, "rust"),
            Language::Other(name) => write!(f, "{name}"),
        }
    }
}

pub fn default_compiler(platform: &Platform, language: &str) -> String {
    match language {
        // Platform agnostic compilers
        "fortran" => "gfortran",
        // Platform specific compilers
        "c" | "cxx" => {
            if platform.is_windows() {
                match language {
                    "c" => "vs2019",
                    "cxx" => "vs2019",
                    _ => unreachable!(),
                }
            } else if platform.is_osx() {
                match language {
                    "c" => "clang",
                    "cxx" => "clangxx",
                    _ => unreachable!(),
                }
            } else if matches!(platform, Platform::EmscriptenWasm32) {
                match language {
                    "c" => "emscripten",
                    "cxx" => "emscripten",
                    _ => unreachable!(),
                }
            } else {
                match language {
                    "c" => "gcc",
                    "cxx" => "gxx",
                    _ => unreachable!(),
                }
            }
        }
        _ => language,
    }
    .to_string()
}

/// Returns the compiler template function for the specified language.
pub fn compiler_requirement(language: &Language) -> Item<PackageDependency> {
    format!("${{{{ compiler('{language}') }}}}")
        .parse()
        .expect("Failed to parse compiler requirement")
}

/// Add configured compilers to build requirements if they are not already
/// present.
///
/// # Arguments
/// * `compilers` - List of compiler names (e.g., ["c", "cxx", "rust", "cuda"])
/// * `requirements` - Mutable reference to the requirements to modify
/// * `dependencies` - The Dependencies struct containing build/host/run dependencies
/// * `host_platform` - The target platform for determining default compiler
///   names
pub fn add_compilers_to_requirements<S>(
    compilers: &[String],
    requirements: &mut Vec<Item<PackageDependency>>,
    dependencies: &crate::traits::targets::Dependencies<S>,
    host_platform: &Platform,
) {
    for compiler_str in compilers {
        // Check if the specific compiler is already present in build dependencies
        let language_compiler = default_compiler(host_platform, compiler_str);
        let source_package_name = pixi_build_types::SourcePackageName::from(language_compiler);

        if !dependencies.build.contains_key(&source_package_name) {
            let template = format!("${{{{ compiler('{compiler_str}') }}}}");
            requirements.push(Item::Value(Value::Template(template)));
        }
    }
}

/// Returns the standard library for a given language, if applicable.
///
/// The implementation just always returns `c` for all languages except for some
/// exceptions.
fn stdlib_for_language(lang: &str) -> Option<&'static str> {
    match lang {
        "go-nocgo" => None,
        _ => Some("c"),
    }
}

pub fn add_stdlib_to_requirements(
    compilers: &[String],
    requirements: &mut Vec<Item<PackageDependency>>,
    variants: &HashSet<NormalizedKey>,
) {
    // For each compiler check if there is a variant stdlib(compiler) key.
    for stdlib in compilers
        .iter()
        .map(Deref::deref)
        .filter_map(stdlib_for_language)
        .unique()
    {
        let stdlib_key = format!("{stdlib}_stdlib");
        if !variants.contains(&NormalizedKey(stdlib_key)) {
            continue;
        }

        // If the stdlib key exists, add it to the requirements
        let template = format!("${{{{ stdlib('{stdlib}') }}}}");
        requirements.push(Item::Value(Value::Template(template)));
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_yaml_snapshot;

    use super::*;

    #[test]
    fn test_compiler_requirements_fortran() {
        let result = compiler_requirement(&Language::Fortran);
        assert_yaml_snapshot!(result);
    }

    #[test]
    fn test_compiler_requirements_c() {
        let result = compiler_requirement(&Language::C);
        assert_yaml_snapshot!(result);
    }

    #[test]
    fn test_compiler_requirements_cxx() {
        let result = compiler_requirement(&Language::Cxx);
        assert_yaml_snapshot!(result);
    }

    #[test]
    fn test_compiler_requirements_rust() {
        let result = compiler_requirement(&Language::Other("rust"));
        assert_yaml_snapshot!(result);
    }

    #[test]
    fn test_compiler_requirements_python() {
        let result = compiler_requirement(&Language::Other("python"));
        assert_yaml_snapshot!(result);
    }
}
