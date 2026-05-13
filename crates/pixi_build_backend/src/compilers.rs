//! We could expose the `default_compiler` function from the `rattler-build`
//! crate

use std::{
    collections::{BTreeMap, HashSet},
    fmt::Display,
    ops::Deref,
};

use itertools::Itertools;
use rattler_build_jinja::Variable;
use rattler_build_recipe::stage0::{
    ConditionalList, Item, JinjaTemplate, SerializableMatchSpec, Value,
};
use rattler_build_types::NormalizedKey;
use rattler_conda_types::Platform;

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
        // CUDA: matches conda-forge's global pinning since CUDA 12 (the legacy
        // `nvcc` package was CUDA 11 and earlier). CUDA is only supported on
        // Linux/Windows, but we return the same name on macOS to keep this
        // function platform-agnostic for `cuda`.
        "cuda" => "cuda-nvcc",
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

/// Returns the default compiler variants that backends should seed for the
/// given host platform.
///
/// These mirror conda-forge's global pinning so that recipes which use
/// `${{ compiler(...) }}` resolve to sensible packages out of the box:
///
/// * On Windows, `c_compiler` and `cxx_compiler` default to `vs2022`
///   (rattler-build's built-in default is `vs2017`, which is too old for most
///   CI runners, and conda-forge moved off `vs2019` in 2024).
/// * On Linux and Windows, `cuda_compiler` defaults to `cuda-nvcc`, matching
///   the `cuda_compiler: cuda-nvcc  # [linux or win]` line in conda-forge's
///   pinning. CUDA is not supported on macOS.
pub fn default_compiler_variants(
    host_platform: Platform,
) -> BTreeMap<NormalizedKey, Vec<Variable>> {
    let mut variants = BTreeMap::new();

    if host_platform.is_windows() {
        variants.insert(NormalizedKey::from("c_compiler"), vec!["vs2022".into()]);
        variants.insert(NormalizedKey::from("cxx_compiler"), vec!["vs2022".into()]);
    }

    if host_platform.is_linux() || host_platform.is_windows() {
        variants.insert(
            NormalizedKey::from("cuda_compiler"),
            vec!["cuda-nvcc".into()],
        );
    }

    variants
}

/// Create a jinja template item for a matchspec.
fn template_item(template: JinjaTemplate) -> Item<SerializableMatchSpec> {
    Item::Value(Value::new_template(template, None))
}

/// Returns the compiler template function for the specified language.
pub fn compiler_requirement(language: &Language) -> Item<SerializableMatchSpec> {
    let template = JinjaTemplate::new(format!("${{{{ compiler('{language}') }}}}"))
        .expect("valid jinja template");
    template_item(template)
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
    requirements: &mut ConditionalList<SerializableMatchSpec>,
    dependencies: &crate::traits::targets::Dependencies<S>,
    host_platform: &Platform,
) {
    for compiler_str in compilers {
        // Check if the specific compiler is already present in build dependencies
        let language_compiler = default_compiler(host_platform, compiler_str);
        let source_package_name = pixi_build_types::SourcePackageName::from(
            rattler_conda_types::PackageName::new_unchecked(language_compiler),
        );

        if !dependencies.build.contains_key(&source_package_name) {
            let template = JinjaTemplate::new(format!("${{{{ compiler('{compiler_str}') }}}}"))
                .expect("valid jinja template");
            requirements.push(template_item(template));
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
    requirements: &mut ConditionalList<SerializableMatchSpec>,
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
        let template = JinjaTemplate::new(format!("${{{{ stdlib('{stdlib}') }}}}"))
            .expect("valid jinja template");
        requirements.push(template_item(template));
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

    #[test]
    fn test_default_compiler_cuda() {
        assert_eq!(default_compiler(&Platform::Linux64, "cuda"), "cuda-nvcc");
        assert_eq!(default_compiler(&Platform::Win64, "cuda"), "cuda-nvcc");
        // CUDA is unsupported on macOS but `default_compiler` is platform
        // agnostic for cuda so it still returns the conda-forge package name.
        assert_eq!(default_compiler(&Platform::Osx64, "cuda"), "cuda-nvcc");
    }

    #[test]
    fn test_default_compiler_variants_linux() {
        let variants = default_compiler_variants(Platform::Linux64);
        assert_eq!(
            variants.get(&NormalizedKey::from("cuda_compiler")),
            Some(&vec!["cuda-nvcc".into()])
        );
        assert!(!variants.contains_key(&NormalizedKey::from("c_compiler")));
        assert!(!variants.contains_key(&NormalizedKey::from("cxx_compiler")));
    }

    #[test]
    fn test_default_compiler_variants_windows() {
        let variants = default_compiler_variants(Platform::Win64);
        assert_eq!(
            variants.get(&NormalizedKey::from("c_compiler")),
            Some(&vec!["vs2022".into()])
        );
        assert_eq!(
            variants.get(&NormalizedKey::from("cxx_compiler")),
            Some(&vec!["vs2022".into()])
        );
        assert_eq!(
            variants.get(&NormalizedKey::from("cuda_compiler")),
            Some(&vec!["cuda-nvcc".into()])
        );
    }

    #[test]
    fn test_default_compiler_variants_macos() {
        let variants = default_compiler_variants(Platform::Osx64);
        assert!(variants.is_empty());
    }
}
