use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use miette::Error;
use pixi_build_backend::generated_recipe::BackendConfig;
use serde::{Deserialize, Serialize};

/// Top level config struct for the Mojo backend.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct MojoBackendConfig {
    /// Environment Variables
    #[serde(default)]
    pub env: IndexMap<String, String>,

    /// Dir that can be specified for outputting pixi debug state.
    #[serde(alias = "debug_dir")]
    pub debug_dir: Option<PathBuf>,

    /// Extra input globs to include in addition to the default ones.
    #[serde(default)]
    pub extra_input_globs: Vec<String>,

    /// Binary executables to produce.
    pub bins: Option<Vec<MojoBinConfig>>,

    /// Packages to produce.
    pub pkg: Option<MojoPkgConfig>,

    /// List of compilers to use (e.g., ["mojo", "c", "cxx"])
    /// If not specified, defaults to ["mojo"]
    pub compilers: Option<Vec<String>>,
}

impl BackendConfig for MojoBackendConfig {
    fn debug_dir(&self) -> Option<&Path> {
        self.debug_dir.as_deref()
    }

    /// Merge this configuration with a target-specific configuration.
    /// Target-specific values override base values using the following rules:
    ///
    /// - env: Platform env vars override base, others merge
    /// - debug_dir: Not allowed to have target specific value
    /// - extra_input_globs: Platform-specific completely replaces base
    /// - bins: Any bins with matching not-None names will be merged,
    ///   Any set-settings on the platform specific pkg override base
    ///   Any bins found only in target_config will be kept
    /// - pkg: Any set-settings on the platform specific pkg override base
    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
        if target_config.debug_dir.is_some() {
            miette::bail!("`debug_dir` cannot have a target specific value");
        }

        let pkg = if target_config.pkg.is_some() {
            if self.pkg.is_some() {
                Some(
                    self.pkg
                        .as_ref()
                        .unwrap()
                        .merge_with_target_config(target_config.pkg.as_ref().unwrap())?,
                )
            } else {
                target_config.pkg.clone()
            }
        } else {
            self.pkg.clone()
        };

        let bins = if target_config.bins.is_some() {
            if self.bins.is_some() {
                // Both base and target have binaries configured
                // Override base with anything found in both target and base.
                // If something is found only in base, drop it.
                // If something is found only in target, drop it.
                let base_bins: HashMap<_, _> = self
                    .bins
                    .as_ref()
                    .unwrap()
                    .iter()
                    .filter(|p| p.name.is_some())
                    .map(|p| (p.name.clone().unwrap(), p.clone()))
                    .collect();

                Some(
                    target_config
                        .bins
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(|p| match p.name.as_ref() {
                            Some(name) => {
                                if let Some(base_bin) = base_bins.get(name) {
                                    base_bin.merge_with_target_config(p)
                                } else {
                                    Ok(p.clone())
                                }
                            }
                            None => Ok(p.clone()),
                        })
                        .collect::<miette::Result<_>>()?,
                )
            } else {
                target_config.bins.clone()
            }
        } else {
            self.bins.clone()
        };

        Ok(Self {
            env: {
                let mut merged_env = self.env.clone();
                merged_env.extend(target_config.env.clone());
                merged_env
            },
            debug_dir: self.debug_dir.clone(),
            extra_input_globs: if target_config.extra_input_globs.is_empty() {
                self.extra_input_globs.clone()
            } else {
                target_config.extra_input_globs.clone()
            },
            bins,
            pkg,
            compilers: target_config
                .compilers
                .clone()
                .or_else(|| self.compilers.clone()),
        })
    }
}

impl MojoBackendConfig {
    /// Auto-derive the bins and pkg config if they have not been specified by the user,
    /// or if they have only been partially specified.
    ///
    /// See [`MojoPkgConfig`] and [`MojoBinConfig`] for details on how they are derived.
    /// The following rules are applied for choosing when to use derived configs:
    ///
    /// - If a `pkg` has been specified by user, don't derive `bins`
    /// - If a `bin` has been specified by user, don't derive `pkg`
    /// - If both a `pkg` and `bin` have been auto-derived, only keep the `bin`
    pub fn auto_derive(
        &self,
        manifest_root: &Path,
        project_name: &str,
    ) -> miette::Result<(Option<Vec<MojoBinConfig>>, Option<MojoPkgConfig>)> {
        // Update bins configs
        let (mut bins, bin_autodetected) =
            MojoBinConfig::auto_derive(self.bins.as_ref(), manifest_root, project_name)?;

        // Update pkg config
        let (mut pkg, pkg_autodetected) =
            MojoPkgConfig::auto_derive(self.pkg.as_ref(), manifest_root, project_name)?;

        // Make sure we have at least one of the two
        if bins.is_none() && pkg.is_none() {
            return Err(Error::msg("No bin or pkg configuration detected."));
        }

        // If we are auto-generating both, keep only the bin
        if bin_autodetected && pkg_autodetected {
            pkg = None;
        }
        // If either wasn't auto-detected, disable auto-detection of the other
        else if bin_autodetected && (!pkg_autodetected && pkg.is_some()) {
            // If I'm publishing a pkg, I may not want to also publish a bin
            bins = None
        } else if (!bin_autodetected && bins.is_some()) && pkg_autodetected {
            // If I'm publishing a bin, I may not want to publish the associated pkg
            pkg = None
        }

        Ok((bins, pkg))
    }
}

/// Config object for a Mojo binary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MojoBinConfig {
    /// Name of the binary.
    ///
    /// This will default to the name of the project for the first
    /// binary selected, any dashes will be replaced with `_`.
    pub name: Option<String>,

    /// Path to file that has the `main` method.
    ///
    /// This will default to looking for a `main.mojo` file in:
    /// - `<manifest_root>/main.mojo`
    pub path: Option<String>,

    /// Extra args to pass to the compiler.
    #[serde(default, rename(serialize = "extra_args"))]
    pub extra_args: Option<Vec<String>>,
}

impl MojoBinConfig {
    /// Fill in any missing info and or try to find our default options.
    ///
    /// - If None, try to find a `main.mojo` file in manifest_root
    /// - If any, for the first one, see if name or path need to be filled in
    /// - If any, verify that there are no name collisions
    pub fn auto_derive(
        conf: Option<&Vec<Self>>,
        manifest_root: &Path,
        project_name: &str,
    ) -> miette::Result<(Option<Vec<Self>>, bool)> {
        let main = Self::find_main(manifest_root).map(|p| p.display().to_string());

        // No configuration specified
        if conf.is_none() {
            if let Some(main) = main {
                return Ok((
                    Some(vec![Self {
                        name: Some(project_name.to_owned()),
                        path: Some(main),
                        ..Default::default()
                    }]),
                    true,
                ));
            } else {
                return Ok((None, false));
            }
        }

        // Some configuration specified
        let mut conf = conf.unwrap().clone(); // checked above
        if conf.is_empty() {
            return Ok((None, false));
        }

        if conf[0].name.is_none() {
            conf[0].name = Some(project_name.to_owned());
        }
        if conf[0].path.is_none() {
            if main.is_none() {
                return Err(Error::msg("Could not find main.mojo for configured binary"));
            }
            conf[0].path = main;
        }

        // Verify no name collisions and that the rest of the binaries have a name and path
        let mut names = HashSet::new();
        for (i, c) in conf.iter().enumerate() {
            if c.name.is_none() {
                return Err(Error::msg(format!(
                    "Binary configuration {} is missing a name.",
                    i + 1
                )));
            }
            if c.path.is_none() {
                return Err(Error::msg(format!(
                    "Binary configuration {} is missing a path.",
                    c.name.as_ref().unwrap(),
                )));
            }
            if names.contains(c.name.as_deref().unwrap()) {
                return Err(Error::msg(format!(
                    "Binary name has been used twice: {}",
                    c.name.as_ref().unwrap()
                )));
            }

            names.insert(c.name.clone().unwrap());
        }

        Ok((Some(conf), false))
    }

    /// Try to find main.mojo in:
    /// - <manifest_root>/main.mojo
    fn find_main(root: &Path) -> Option<PathBuf> {
        let mut path = root.join("main");
        for ext in ["mojo", "🔥"] {
            path.set_extension(ext);
            if path.exists() {
                return Some(path);
            }
        }
        None
    }

    /// Merge with a target-specific configuration.
    ///
    /// All target-settings that are not None will override base.
    ///
    /// **Note** bins must have the same name to be merged.
    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
        if self.name.is_some() && target_config.name.is_some() {
            if self.name.as_ref().unwrap() != target_config.name.as_ref().unwrap() {
                miette::bail!("Both bins must have a set name to be merged");
            }
        } else {
            miette::bail!("Both bins must have a set name to be merged");
        }

        let path = if target_config.path.is_some() {
            target_config.path.clone()
        } else {
            self.path.clone()
        };

        let extra_args = if target_config.extra_args.is_some() {
            target_config.extra_args.clone()
        } else {
            self.extra_args.clone()
        };

        Ok(Self {
            name: self.name.clone(),
            path,
            extra_args,
        })
    }
}

/// Config object for a Mojo package.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MojoPkgConfig {
    /// Name to give the mojo package (.mojopkg suffix will be added).
    ///
    /// This will default to the name of the project, any dashes will
    /// be replaced with `_`.
    pub name: Option<String>,

    /// Path to the directory that constitutes the package.
    ///
    /// This will default to lookingo for a folder with an `__init__.mojo` in
    /// in the following order:
    /// - `<manifest_root>/<package_name>/__init__.mojo`
    /// - `<manifest_root>/src/__init__.mojo`
    pub path: Option<String>,

    /// Extra args to pass to the compiler.
    #[serde(default, rename(serialize = "extra_args"))]
    pub extra_args: Option<Vec<String>>,
}

impl MojoPkgConfig {
    /// Fill in any missing info anod or try to find our default options.
    ///
    /// - If None, try to find a `<project_name>` or `src` dir with an `__init__.mojo` file in it.
    /// - If Some, see if name or path need to be filled in.
    pub fn auto_derive(
        conf: Option<&Self>,
        manifest_root: &Path,
        package_name: &str,
    ) -> miette::Result<(Option<Self>, bool)> {
        if let Some(conf) = conf {
            // A conf was given, make sure it has a name and path
            let mut conf = conf.clone();
            if conf.name.is_none() {
                conf.name = Some(package_name.to_owned());
            }

            let path = Self::find_init_parent(manifest_root, package_name);
            if conf.path.is_none() {
                if path.is_none() {
                    return Err(Error::msg(format!(
                        "Could not find valid package path for {}",
                        conf.name.unwrap()
                    )));
                }
                conf.path = path.map(|p| p.display().to_string());
            }
            Ok((Some(conf), false))
        } else {
            // No conf given check if we can find a valid package
            let path = Self::find_init_parent(manifest_root, package_name);
            if path.is_none() {
                return Ok((None, false));
            }
            Ok((
                Some(Self {
                    name: Some(package_name.to_owned()),
                    path: path.map(|p| p.display().to_string()),
                    ..Default::default()
                }),
                true,
            ))
        }
    }

    /// Find the parent directory of a possible package `__init__.mojo` file.
    ///
    /// This checks (in this order):
    /// - `<project_name>`
    /// - src
    ///
    /// and returns the first one found.
    fn find_init_parent(root: &Path, project_name: &str) -> Option<PathBuf> {
        for dir in [project_name, "src"] {
            let mut path = root.join(dir).join("__init__");
            for ext in ["mojo", "🔥"] {
                path.set_extension(ext);
                if path.exists() {
                    return Some(root.join(dir));
                }
            }
        }
        None
    }

    /// Merge with a target-specific configuration.
    ///
    /// All target-settings that are not None will override base.
    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
        let name = if target_config.name.is_some() {
            target_config.name.clone()
        } else {
            self.name.clone()
        };

        let path = if target_config.path.is_some() {
            target_config.path.clone()
        } else {
            self.path.clone()
        };

        let extra_args = if target_config.extra_args.is_some() {
            target_config.extra_args.clone()
        } else {
            self.extra_args.clone()
        };

        Ok(Self {
            name,
            path,
            extra_args,
        })
    }
}

/// Clean the package name for use in [`MojoPkgConfig`] and [`MojoBinconfig`].
///
/// This just entails converting - to _.
pub fn clean_project_name(s: &str) -> String {
    s.to_owned().replace("-", "_")
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_ensure_deseralize_from_empty() {
        let json_data = json!({});
        serde_json::from_value::<MojoBackendConfig>(json_data).unwrap();
    }

    #[derive(Debug)]
    enum ExpectedBinResult {
        /// A possible binary name that would be found, as well as whether or not
        /// it was auto-detected, or whether it was found via user configuration.
        Success {
            binary_name: Option<&'static str>,
            autodetected: bool,
        },
        /// There was some misconfiguration resulting in an error to show the user.
        Error(&'static str),
    }

    struct BinTestCase {
        /// User specified config.
        config: Option<Vec<MojoBinConfig>>,
        /// Path to a project `main.mojo` file to be created in a temp dir.
        main_file: Option<&'static str>,
        /// The expected result of the test.
        expected: ExpectedBinResult,
    }

    #[rstest]
    #[case::no_config_no_main(BinTestCase {
        config: None,
        main_file: None,
        expected: ExpectedBinResult::Success { binary_name: None, autodetected: false }
    })]
    #[case::no_config_with_main_mojo(BinTestCase {
        config: None,
        main_file: Some("main.mojo"),
        expected: ExpectedBinResult::Success { binary_name: Some("test_project"), autodetected: true }
    })]
    #[case::no_config_with_main_fire(BinTestCase {
        config: None,
        main_file: Some("main.🔥"),
        expected: ExpectedBinResult::Success { binary_name: Some("test_project"), autodetected: true }
    })]
    #[case::empty_config(BinTestCase {
        config: Some(vec![]),
        main_file: None,
        expected: ExpectedBinResult::Success { binary_name: None, autodetected: false }
    })]
    #[case::config_missing_name_and_path(BinTestCase {
        config: Some(vec![MojoBinConfig::default()]),
        main_file: Some("main.mojo"),
        expected: ExpectedBinResult::Success { binary_name: Some("test_project"), autodetected: false }
    })]
    #[case::config_missing_path_no_main(BinTestCase {
        config: Some(vec![MojoBinConfig::default()]),
        main_file: None,
        expected: ExpectedBinResult::Error("Could not find main.mojo for configured binary")
    })]
    #[case::multiple_bins_missing_name(BinTestCase {
        config: Some(vec![
            MojoBinConfig { name: Some("bin1".to_string()), path: Some("main1.mojo".to_string()), ..Default::default() },
            MojoBinConfig { path: Some("main2.mojo".to_string()), ..Default::default() },
        ]),
        main_file: None,
        expected: ExpectedBinResult::Error("Binary configuration 2 is missing a name.")
    })]
    #[case::multiple_bins_missing_path(BinTestCase {
        config: Some(vec![
            MojoBinConfig { name: Some("bin1".to_string()), path: Some("main1.mojo".to_string()), ..Default::default() },
            MojoBinConfig { name: Some("bin2".to_string()), ..Default::default() },
        ]),
        main_file: None,
        expected: ExpectedBinResult::Error("Binary configuration bin2 is missing a path.")
    })]
    #[case::duplicate_names(BinTestCase {
        config: Some(vec![
            MojoBinConfig { name: Some("mybin".to_string()), path: Some("main1.mojo".to_string()), ..Default::default() },
            MojoBinConfig { name: Some("mybin".to_string()), path: Some("main2.mojo".to_string()), ..Default::default() },
        ]),
        main_file: None,
        expected: ExpectedBinResult::Error("Binary name has been used twice: mybin")
    })]
    fn test_mojo_bin_config_fill_defaults(#[case] test_case: BinTestCase) {
        let temp = TempDir::new().unwrap();
        let manifest_root = temp.path().to_path_buf();

        // Write the main.mojo file specified by test case, if present
        if let Some(filename) = test_case.main_file {
            std::fs::write(manifest_root.join(filename), "def main():\n    pass").unwrap();
        }

        let result =
            MojoBinConfig::auto_derive(test_case.config.as_ref(), &manifest_root, "test_project");

        match test_case.expected {
            ExpectedBinResult::Success {
                binary_name: expected_name,
                autodetected: expected_autodetected,
            } => {
                let (bins, autodetected) = result.unwrap();
                assert_eq!(autodetected, expected_autodetected);

                if let Some(expected_name) = expected_name {
                    assert!(bins.is_some());
                    let bins = bins.unwrap();
                    assert_eq!(bins.len(), 1);
                    assert_eq!(bins[0].name, Some(expected_name.to_string()));
                    if let Some(filename) = test_case.main_file {
                        assert_eq!(
                            bins[0].path,
                            Some(manifest_root.join(filename).display().to_string())
                        );
                    }
                } else {
                    assert_eq!(bins, None);
                }
            }
            ExpectedBinResult::Error(expected_error) => {
                assert!(result.is_err());
                assert_eq!(result.unwrap_err().to_string(), expected_error);
            }
        }
    }

    #[derive(Debug)]
    enum ExpectedPkgResult {
        /// A possible pkg name that would be found, as well as whether or not
        /// it was auto-detected, or whether it was found via user configuration.
        Success {
            name: Option<&'static str>,
            autodetected: bool,
        },
        /// An expected error message that the user would be shown.
        Error(&'static str),
    }

    struct PkgTestCase {
        /// User defined config for the pkg
        config: Option<MojoPkgConfig>,
        /// Path to a `__init__.mojo` file to be created in a temp dir.
        init_file: Option<(&'static str, &'static str)>, // (directory, filename)
        /// Expected result of the test.
        expected: ExpectedPkgResult,
    }

    #[rstest]
    #[case::no_config_no_init(PkgTestCase {
        config: None,
        init_file: None,
        expected: ExpectedPkgResult::Success { name: None, autodetected: false }
    })]
    #[case::no_config_with_init_in_project_dir(PkgTestCase {
        config: None,
        init_file: Some(("test_project", "__init__.mojo")),
        expected: ExpectedPkgResult::Success { name: Some("test_project"), autodetected: true }
    })]
    #[case::no_config_with_init_in_src(PkgTestCase {
        config: None,
        init_file: Some(("src", "__init__.mojo")),
        expected: ExpectedPkgResult::Success { name: Some("test_project"), autodetected: true }
    })]
    #[case::no_config_with_init_fire_emoji(PkgTestCase {
        config: None,
        init_file: Some(("src", "__init__.🔥")),
        expected: ExpectedPkgResult::Success { name: Some("test_project"), autodetected: true }
    })]
    #[case::config_missing_name_and_path(PkgTestCase {
        config: Some(MojoPkgConfig::default()),
        init_file: Some(("src", "__init__.mojo")),
        expected: ExpectedPkgResult::Success { name: Some("test_project"), autodetected: false }
    })]
    #[case::config_with_all_fields(PkgTestCase {
        config: Some(MojoPkgConfig {
            name: Some("mypackage".to_string()),
            path: Some("custom/path".to_string()),
            extra_args: Some(vec!["-O3".to_string()]),
        }),
        init_file: None,
        expected: ExpectedPkgResult::Success { name: Some("mypackage"), autodetected: false }
    })]
    #[case::config_missing_path_no_init(PkgTestCase {
        config: Some(MojoPkgConfig::default()),
        init_file: None,
        expected: ExpectedPkgResult::Error("Could not find valid package path for test_project")
    })]
    fn test_mojo_pkg_config_fill_defaults(#[case] test_case: PkgTestCase) {
        let temp = TempDir::new().unwrap();
        let manifest_root = temp.path().to_path_buf();

        if let Some((dir, filename)) = test_case.init_file {
            let init_dir = manifest_root.join(dir);
            std::fs::create_dir_all(&init_dir).unwrap();
            std::fs::write(init_dir.join(filename), "").unwrap();
        }

        let result =
            MojoPkgConfig::auto_derive(test_case.config.as_ref(), &manifest_root, "test_project");

        match test_case.expected {
            ExpectedPkgResult::Success {
                name: expected_name,
                autodetected: expected_autodetected,
            } => {
                let (pkg, autodetected) = result.unwrap();
                assert_eq!(autodetected, expected_autodetected);

                if let Some(expected_name) = expected_name {
                    assert!(pkg.is_some());
                    let pkg = pkg.unwrap();
                    assert_eq!(pkg.name, Some(expected_name.to_string()));

                    // For the custom config case, check the custom path and args
                    if expected_name == "mypackage" {
                        assert_eq!(pkg.path, Some("custom/path".to_string()));
                        assert_eq!(pkg.extra_args, Some(vec!["-O3".to_string()]));
                    } else if let Some((dir, _)) = test_case.init_file {
                        assert_eq!(
                            pkg.path,
                            Some(manifest_root.join(dir).display().to_string())
                        );
                    }
                } else {
                    assert_eq!(pkg, None);
                }
            }
            ExpectedPkgResult::Error(expected_error) => {
                assert!(result.is_err());
                assert_eq!(result.unwrap_err().to_string(), expected_error);
            }
        }
    }

    #[rstest]
    #[case("my-project", "my_project")]
    #[case("test_project", "test_project")]
    #[case("some-complex-name", "some_complex_name")]
    #[case("nodashes", "nodashes")]
    #[case("multiple-dashes-here", "multiple_dashes_here")]
    fn test_clean_project_name(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(clean_project_name(input), expected);
    }
}
