use std::{
    collections::{HashMap, HashSet},
    path::Path,
    str::FromStr,
};

use itertools::Itertools;
use pixi_install_pypi::UnresolvedPypiRecord;
use pixi_manifest::{
    FeaturesExt,
    pypi::pypi_options::NoBuild,
};
use pixi_pypi_spec::PixiPypiSource;
use pypi_modifiers::Tags;
use rattler_conda_types::{ChannelUrl, NamedChannelOrUrl, Platform};
use rattler_lock::{LockedPackage, PypiIndexes, UrlOrPath};
use crate::lock_file::records_by_name::HasNameVersion;
use url::Url;
use uv_distribution_filename::{DistExtension, ExtensionError, SourceDistExtension, WheelFilename};

use super::errors::{EnvironmentUnsat, IndexesMismatch, verify_exclude_newer};
use crate::workspace::{Environment, grouped_environment::GroupedEnvironment};

/// Verifies that all the requirements of the specified `environment` can be
/// satisfied with the packages present in the lock-file.
///
/// This function returns a [`EnvironmentUnsat`] error if a verification issue
/// occurred. The [`EnvironmentUnsat`] error should contain enough information
/// for the user and developer to figure out what went wrong.
pub fn verify_environment_satisfiability(
    environment: &Environment<'_>,
    locked_environment: rattler_lock::Environment<'_>,
) -> Result<(), EnvironmentUnsat> {
    let grouped_env = GroupedEnvironment::from(environment.clone());

    // Check if the channels in the lock file match our current configuration. Note
    // that the order matters here. If channels are added in a different order,
    // the solver might return a different result.
    let config = environment.channel_config();
    let channels: Vec<ChannelUrl> = grouped_env
        .channels()
        .into_iter()
        .map(|channel| channel.clone().into_base_url(&config))
        .try_collect()?;

    let locked_channels: Vec<ChannelUrl> = locked_environment
        .channels()
        .iter()
        .map(|c| {
            NamedChannelOrUrl::from_str(&c.url)
                .unwrap_or_else(|_err| NamedChannelOrUrl::Name(c.url.clone()))
                .into_base_url(&config)
        })
        .try_collect()?;

    // Check if channels match or were only extended (appended).
    // If locked_channels is a prefix of channels, only lower-priority channels were added,
    // which doesn't affect existing package selections due to channel priority semantics.
    if channels.starts_with(&locked_channels) {
        if channels.len() > locked_channels.len() {
            // Channels were extended - lock file needs update but packages are still valid
            return Err(EnvironmentUnsat::ChannelsExtended);
        }
        // Exact match - channels are identical, no error
    } else {
        // Channels were removed, reordered, or prepended - need full re-solve
        return Err(EnvironmentUnsat::ChannelsMismatch);
    }

    let platforms = environment.platforms();
    let locked_platforms = locked_environment
        .platforms()
        .map(|p| p.subdir())
        .collect::<HashSet<_>>();
    let additional_platforms = locked_platforms
        .difference(&platforms)
        .copied()
        .collect::<HashSet<_>>();
    if !additional_platforms.is_empty() {
        return Err(EnvironmentUnsat::AdditionalPlatformsInLockFile(
            additional_platforms,
        ));
    }

    // Do some more checks if we have pypi dependencies
    // 1. Check if the PyPI indexes are present and match
    // 2. Check if we have a no-build option set, that we only have binary packages,
    //    or an editable source
    // 3. Check that wheel tags still are possible with current system requirements
    let pypi_dependencies = environment.pypi_dependencies(None);
    if !pypi_dependencies.is_empty() {
        let group_pypi_options = grouped_env.pypi_options();
        let indexes = rattler_lock::PypiIndexes::from(group_pypi_options.clone());

        // Check if the indexes in the lock file match our current configuration.
        verify_pypi_indexes(locked_environment, indexes)?;

        let no_build_check = PypiNoBuildCheck::new(group_pypi_options.no_build.as_ref());
        let pypi_wheel_tags_check = PypiWheelTagsCheck::new(environment, &locked_environment);

        // Actually check all pypi packages in one iteration
        for (lock_platform, package_it) in locked_environment.pypi_packages_by_platform() {
            let platform = lock_platform.subdir();
            for package_data in package_it {
                let record = UnresolvedPypiRecord::from(package_data.clone());
                let pypi_source = pypi_dependencies
                    .get(record.name())
                    .and_then(|specs| specs.last())
                    .map(|spec| &spec.source);
                no_build_check.check(&record, pypi_source)?;
                pypi_wheel_tags_check.check(platform, &record)?;
            }
        }
    }

    // Verify solver options
    let expected_solve_strategy = environment.solve_strategy().into();
    if locked_environment.solve_options().strategy != expected_solve_strategy {
        return Err(EnvironmentUnsat::SolveStrategyMismatch {
            locked_strategy: locked_environment.solve_options().strategy,
            expected_strategy: expected_solve_strategy,
        });
    }

    let expected_channel_priority = environment
        .channel_priority()
        .unwrap_or_default()
        .unwrap_or_default()
        .into();
    if locked_environment.solve_options().channel_priority != expected_channel_priority {
        return Err(EnvironmentUnsat::ChannelPriorityMismatch {
            locked_priority: locked_environment.solve_options().channel_priority,
            expected_priority: expected_channel_priority,
        });
    }

    let locked_prerelease_mode = locked_environment
        .solve_options()
        .pypi_prerelease_mode
        .into();
    let expected_prerelease_mode = grouped_env
        .pypi_options()
        .prerelease_mode
        .unwrap_or_default();
    if locked_prerelease_mode != expected_prerelease_mode {
        return Err(EnvironmentUnsat::PypiPrereleaseModeMismatch {
            locked_mode: locked_prerelease_mode,
            expected_mode: expected_prerelease_mode,
        });
    }

    let resolved_exclude_newer = environment.exclude_newer_config_resolved(&config)?;

    let exclude_newer = resolved_exclude_newer
        .as_ref()
        .cloned()
        .map(rattler_solve::ExcludeNewer::from);

    if let Err(err) = verify_exclude_newer(exclude_newer.as_ref(), &locked_environment) {
        return Err(EnvironmentUnsat::ExcludeNewerMismatch(err));
    }

    Ok(())
}

struct PypiWheelTagsCheck {
    platform_wheel_tags: HashMap<Platform, Tags>,
}

impl PypiWheelTagsCheck {
    pub fn new(
        environment: &Environment,
        locked_environment: &rattler_lock::Environment<'_>,
    ) -> Self {
        let platform_wheel_tags = {
            let system_requirements = environment.system_requirements();
            locked_environment
                .packages_by_platform()
                .flat_map(|(lock_platform, packages)| {
                    let platform = lock_platform.subdir();
                    packages.map(move |package| (platform, package))
                })
                .filter_map(|(platform, package)| match package {
                    LockedPackage::Conda(rattler_lock::CondaPackageData::Binary(package)) => {
                        Some((platform, package))
                    }
                    _ => None,
                })
                .filter(move |(_, package)| {
                    pypi_modifiers::pypi_tags::is_python_record(&package.package_record)
                })
                .filter_map(|(platform, package)| {
                    pypi_modifiers::pypi_tags::get_pypi_tags(
                        platform,
                        &system_requirements,
                        &package.package_record,
                    )
                    .ok()
                    .map(|tags| (platform, tags))
                })
                .collect::<HashMap<_, _>>()
        };

        PypiWheelTagsCheck {
            platform_wheel_tags,
        }
    }

    pub fn check(
        &self,
        platform: Platform,
        package_data: &UnresolvedPypiRecord,
    ) -> Result<(), EnvironmentUnsat> {
        let package_data = package_data.as_package_data();
        let Some(package_file_name) = package_data.location().file_name() else {
            return Ok(());
        };
        let Some(platform_tags) = self.platform_wheel_tags.get(&platform) else {
            return Ok(());
        };
        let Ok(wheel) = WheelFilename::from_str(package_file_name) else {
            return Ok(());
        };
        if !wheel.is_compatible(platform_tags) {
            Err(EnvironmentUnsat::PypiWheelTagsMismatch {
                wheel: wheel.name.to_string(),
            })
        } else {
            Ok(())
        }
    }
}

// Check if we are disallowing all source packages or only a subset
#[derive(Eq, PartialEq)]
enum Check {
    All,
    Packages(HashSet<pep508_rs::PackageName>),
}

pub struct PypiNoBuildCheck {
    check: Option<Check>,
}

impl PypiNoBuildCheck {
    pub fn new(no_build: Option<&NoBuild>) -> Self {
        let check = match no_build {
            // Ok, so we are allowed to build any source package
            Some(NoBuild::None) | None => None,
            // We are not allowed to build any source package
            Some(NoBuild::All) => Some(Check::All),
            // We are not allowed to build a subset of source packages
            Some(NoBuild::Packages(hash_set)) => {
                let packages = hash_set
                    .iter()
                    .filter_map(|name| pep508_rs::PackageName::new(name.to_string()).ok())
                    .collect();
                Some(Check::Packages(packages))
            }
        };

        Self { check }
    }

    pub fn check(
        &self,
        package_data: &UnresolvedPypiRecord,
        source: Option<&PixiPypiSource>,
    ) -> Result<(), EnvironmentUnsat> {
        let package_data = package_data.as_package_data();
        let Some(check) = &self.check else {
            return Ok(());
        };

        // Determine if we do not accept non-wheels for all packages or only for a
        // subset Check all the currently locked packages if we are making any
        // violations
        // Small helper function to get the dist extension from a url
        fn pypi_dist_extension_from_url(url: &Url) -> Result<DistExtension, ExtensionError> {
            // Take the file name from the url
            let path = url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .unwrap_or_default();
            // Convert the path to a dist extension
            DistExtension::from_path(Path::new(path))
        }

        let extension = match &**package_data.location() {
            // Get the extension from the url
            UrlOrPath::Url(url) => {
                if url.scheme().starts_with("git+") {
                    // Just choose some source extension, does not really matter, cause it is
                    // actually a directory, this is just for the check
                    Ok(DistExtension::Source(SourceDistExtension::TarGz))
                } else {
                    pypi_dist_extension_from_url(url)
                }
            }
            UrlOrPath::Path(path) => {
                // Editables are allowed with no-build
                // Check this before is_dir() because the path may be relative
                // and not resolve correctly from the current working directory
                let is_editable = source
                    .map(|source| match source {
                        PixiPypiSource::Path { path: _, editable } => editable.unwrap_or_default(),
                        _ => false,
                    })
                    .unwrap_or_default();
                if is_editable {
                    return Ok(());
                }
                let path = Path::new(path.as_str());
                if path.is_dir() {
                    // Non-editable source packages might not be allowed
                    Ok(DistExtension::Source(SourceDistExtension::TarGz))
                } else {
                    // Could be a reference to a wheel or sdist
                    DistExtension::from_path(path)
                }
            }
        }?;

        match extension {
            // Wheels are fine
            DistExtension::Wheel => Ok(()),
            // Check if we have a source package that we are not allowed to build
            // it could be that we are only disallowing for certain source packages
            DistExtension::Source(_) => match check {
                Check::All => Err(EnvironmentUnsat::NoBuildWithNonBinaryPackages(
                    package_data.name().to_string(),
                )),
                Check::Packages(hash_set) => {
                    if hash_set.contains(package_data.name()) {
                        Err(EnvironmentUnsat::NoBuildWithNonBinaryPackages(
                            package_data.name().to_string(),
                        ))
                    } else {
                        Ok(())
                    }
                }
            },
        }
    }
}

fn verify_pypi_indexes(
    locked_environment: rattler_lock::Environment<'_>,
    indexes: PypiIndexes,
) -> Result<(), EnvironmentUnsat> {
    match locked_environment.pypi_indexes() {
        None => {
            // Mismatch when there should be an index but there is not
            if locked_environment
                .lock_file()
                .version()
                .should_pypi_indexes_be_present()
                && locked_environment
                    .pypi_packages_by_platform()
                    .any(|(_platform, mut packages)| packages.next().is_some())
            {
                return Err(IndexesMismatch {
                    current: indexes,
                    previous: None,
                }
                .into());
            }
        }
        Some(locked_indexes) => {
            if locked_indexes != &indexes {
                return Err(IndexesMismatch {
                    current: indexes,
                    previous: Some(locked_indexes.clone()),
                }
                .into());
            }
        }
    }
    Ok(())
}
