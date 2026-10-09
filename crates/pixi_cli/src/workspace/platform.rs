use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::str::FromStr;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::{WorkspaceLocator, environment::LockFileUsage};
use pixi_manifest::{
    EnvironmentName, FeatureName, FeaturesExt, HasWorkspaceManifest, PixiPlatform,
    PixiPlatformName, PlatformEdit, PlatformMove,
    platform::{
        candidate_subdirs, capability_satisfied_by, host::machine_virtual_packages,
        subdir_default_virtual_packages,
    },
};
use rattler_conda_types::{
    GenericVirtualPackage, PackageName, Subdir, Version,
    virtual_package_detector::VirtualPackageName,
};

use crate::{
    cli_config::{ScriptWorkspaceConfig, script_lock_file_usage},
    cli_interface::{CliInterface, cli_context},
};

/// Commands to manage workspace platforms.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: ScriptWorkspaceConfig,

    #[clap(subcommand)]
    pub command: Command,
}

/// Common virtual-package shortcut flags shared by `add` and `edit`. Wrapped
/// in a clap struct so the rules (parsing, validation, conversion to
/// `GenericVirtualPackage`) live in one place.
///
/// Mirrors the TOML's built-in virtual-package shortcuts. Other capabilities
/// use trailing `name=version[=build_string]` positionals, with an optional
/// canonical `__` prefix.
#[derive(Parser, Debug, Default, Clone)]
pub struct VirtualPackageArgs {
    /// Declare a `__cuda` virtual package at the given version, e.g. `12.0`.
    /// Valid on any subdir.
    #[clap(long, value_name = "VERSION")]
    pub cuda: Option<String>,

    /// Declare a `__cuda_arch` virtual package (GPU compute capability) at the
    /// given version, e.g. `8.6`. Requires `--cuda` (or an existing `__cuda`),
    /// matching the conda CEP coupling. Serialized as `cuda = { driver, arch }`.
    #[clap(long, value_name = "VERSION")]
    pub cuda_arch: Option<String>,

    /// Declare a `__archspec` virtual package with the given microarchitecture
    /// string, e.g. `x86_64_v3`. Valid on any subdir.
    #[clap(long, value_name = "ARCH")]
    pub archspec: Option<String>,

    /// Declare a `__glibc` virtual package at the given version, e.g. `2.28`.
    /// Only valid on linux subdirs.
    #[clap(long, value_name = "VERSION")]
    pub glibc: Option<String>,

    /// Declare a `__linux` virtual package at the given kernel version,
    /// e.g. `5.10`. Only valid on linux subdirs.
    #[clap(long, value_name = "VERSION")]
    pub linux: Option<String>,

    /// Declare a `__osx` virtual package at the given macOS version,
    /// e.g. `14.0`. Only valid on osx subdirs.
    #[clap(long, visible_alias = "osx", value_name = "VERSION")]
    pub macos: Option<String>,

    /// Declare a `__win` virtual package at the given Windows version,
    /// e.g. `10`. Only valid on win subdirs.
    #[clap(long, value_name = "VERSION")]
    pub windows: Option<String>,
}

impl VirtualPackageArgs {
    /// Whether any of the flags were supplied by the user.
    pub fn is_empty(&self) -> bool {
        self.cuda.is_none()
            && self.cuda_arch.is_none()
            && self.archspec.is_none()
            && self.glibc.is_none()
            && self.linux.is_none()
            && self.macos.is_none()
            && self.windows.is_none()
    }

    /// Translate the built-in flags plus arbitrary trailing
    /// `name=version[=build_string]` positionals into
    /// [`GenericVirtualPackage`] values. `subdir` rejects invalid combinations
    /// such as `--glibc` on `win-64`.
    pub fn into_specs(
        self,
        subdir: Subdir,
        custom_specs: &[String],
    ) -> miette::Result<Vec<GenericVirtualPackage>> {
        let mut specs = Vec::new();
        let mut seen_names = HashSet::new();

        if let Some(value) = self.cuda {
            let version = parse_virtual_package_version("--cuda", &value)?;
            push_unique(
                &mut specs,
                &mut seen_names,
                "__cuda",
                version,
                String::new(),
            )?;
        }
        if let Some(value) = self.cuda_arch {
            // The CEP coupling (`__cuda_arch` requires `__cuda`) is enforced by
            // the platform model once the full virtual-package set is known --
            // here we only collect the spec, since `edit` may add `--cuda-arch`
            // to a platform that already declares `__cuda`.
            let version = parse_virtual_package_version("--cuda-arch", &value)?;
            push_unique(
                &mut specs,
                &mut seen_names,
                "__cuda_arch",
                version,
                String::new(),
            )?;
        }
        if let Some(value) = self.archspec {
            if value.is_empty() {
                miette::bail!("--archspec requires a non-empty microarchitecture string");
            }
            pixi_manifest::platform::validate_archspec_name(&value)
                .map_err(|message| miette::miette!("{message}"))?;
            push_unique(
                &mut specs,
                &mut seen_names,
                "__archspec",
                zero_version(),
                value,
            )?;
        }
        if let Some(value) = self.glibc {
            require_subdir_family(subdir, Subdir::is_linux, "--glibc", "linux")?;
            let version = parse_virtual_package_version("--glibc", &value)?;
            push_unique(
                &mut specs,
                &mut seen_names,
                "__glibc",
                version,
                String::new(),
            )?;
        }
        if let Some(value) = self.linux {
            require_subdir_family(subdir, Subdir::is_linux, "--linux", "linux")?;
            let version = parse_virtual_package_version("--linux", &value)?;
            push_unique(
                &mut specs,
                &mut seen_names,
                "__linux",
                version,
                String::new(),
            )?;
        }
        if let Some(value) = self.macos {
            require_subdir_family(subdir, Subdir::is_osx, "--macos", "osx")?;
            let version = parse_virtual_package_version("--macos", &value)?;
            push_unique(&mut specs, &mut seen_names, "__osx", version, String::new())?;
        }
        if let Some(value) = self.windows {
            require_subdir_family(subdir, Subdir::is_windows, "--windows", "win")?;
            let version = parse_virtual_package_version("--windows", &value)?;
            push_unique(&mut specs, &mut seen_names, "__win", version, String::new())?;
        }

        for spec in custom_specs {
            let gvp = parse_virtual_package_spec(spec)?;
            let name = gvp.name.as_normalized().to_string();
            if !seen_names.insert(name.clone()) {
                miette::bail!(
                    "virtual package '{name}' was specified more than once on the command line"
                );
            }
            specs.push(gvp);
        }

        Ok(specs)
    }
}

fn push_unique(
    specs: &mut Vec<GenericVirtualPackage>,
    seen: &mut HashSet<String>,
    conda_name: &str,
    version: Version,
    build_string: String,
) -> miette::Result<()> {
    let name = virtual_package_name(conda_name);
    let normalized = name.as_normalized().to_string();
    if !seen.insert(normalized.clone()) {
        miette::bail!(
            "virtual package '{normalized}' was specified more than once on the command line"
        );
    }
    specs.push(GenericVirtualPackage {
        name,
        version,
        build_string,
    });
    Ok(())
}

fn require_subdir_family(
    subdir: Subdir,
    predicate: impl Fn(Subdir) -> bool,
    flag: &str,
    family: &str,
) -> miette::Result<()> {
    if !predicate(subdir) {
        miette::bail!(
            "{flag} only applies to {family} subdirs, but the platform's subdir is '{}'",
            subdir.as_str()
        );
    }
    Ok(())
}

fn virtual_package_name(name: &str) -> PackageName {
    PackageName::try_from(name).expect("static virtual package name should be valid")
}

fn zero_version() -> Version {
    Version::from_str("0").expect("'0' is a valid Version")
}

fn parse_virtual_package_version(flag: &str, value: &str) -> miette::Result<Version> {
    Version::from_str(value)
        .into_diagnostic()
        .map_err(|e| miette::miette!("{flag}: '{value}' is not a valid version: {e}"))
}

fn canonical_virtual_package_name(key: &str) -> miette::Result<PackageName> {
    let name = if key.starts_with("__") {
        VirtualPackageName::try_from(key)
    } else {
        VirtualPackageName::try_from(format!("__{key}"))
    };
    name.map(VirtualPackageName::into_package_name)
        .map_err(|error| miette::miette!("'{key}' is not a valid virtual-package name: {error}"))
}

fn parse_virtual_package_spec(spec: &str) -> miette::Result<GenericVirtualPackage> {
    let mut parts = spec.splitn(3, '=');
    let key = parts.next().unwrap_or("");
    let name = canonical_virtual_package_name(key)?;
    let version_str = parts.next().ok_or_else(|| {
        miette::miette!(
            "'{spec}' is not a virtual-package spec: expected name=version[=build_string]"
        )
    })?;
    let version = Version::from_str(version_str)
        .into_diagnostic()
        .map_err(|error| {
            miette::miette!("'{version_str}' is not a valid virtual-package version: {error}")
        })?;
    let build_string = parts.next().unwrap_or("").to_string();
    pixi_manifest::platform::validate_virtual_package_build_string(&name, &build_string)
        .map_err(|message| miette::miette!("{message}"))?;
    Ok(GenericVirtualPackage {
        name,
        version,
        build_string,
    })
}

fn is_virtual_package_positional(input: &str) -> bool {
    input
        .split_once('=')
        .is_some_and(|(_, value)| Subdir::from_str(value).is_err())
}

/// Parse a positional add argument. Accepts either a bare subdir
/// (`linux-64`) or `<name>=<subdir>` (`gpu-linux=linux-64`).
fn parse_add_positional(input: &str) -> miette::Result<(PixiPlatformName, Subdir)> {
    if let Some((name, subdir)) = input.split_once('=') {
        let name = PixiPlatformName::try_from(name)
            .into_diagnostic()
            .map_err(|e| miette::miette!("invalid platform name '{name}': {e}"))?;
        let subdir = Subdir::from_str(subdir)
            .into_diagnostic()
            .map_err(|e| miette::miette!("'{subdir}' is not a valid conda subdir: {e}"))?;
        Ok((name, subdir))
    } else {
        let subdir = Subdir::from_str(input)
            .into_diagnostic()
            .map_err(|e| miette::miette!("'{input}' is not a valid conda subdir: {e}"))?;
        Ok((subdir.into(), subdir))
    }
}

#[derive(Parser, Debug, Default)]
pub struct AddArgs {
    /// Platforms to add, optionally followed by arbitrary virtual-package specs.
    ///
    /// A bare conda subdir (`linux-64`) or `<name>=<subdir>`
    /// (`gpu-linux=linux-64`) adds a platform. Any other
    /// `<name>=<version>[=<build_string>]` entry declares a virtual package on
    /// the single platform in the same invocation. The canonical `__` prefix
    /// is optional.
    ///
    /// With `--auto-detect`, give at most a single bare `<name>` to name the
    /// detected platform. Virtual-package specs override detected values.
    ///
    /// When any virtual package is set, exactly one platform may be given.
    #[clap(
        num_args=0..,
        value_name = "PLATFORM|NAME=PLATFORM|VP=VERSION[=BUILD]",
    )]
    pub platform: Vec<String>,

    /// Declare an arbitrary virtual package explicitly. Use this when its
    /// version is also a conda subdir name, which would otherwise parse as a
    /// `<name>=<subdir>` platform entry.
    #[clap(long = "virtual-package", value_name = "NAME=VERSION[=BUILD]")]
    pub custom_virtual_packages: Vec<String>,

    /// Detect this machine's platform (subdir and virtual packages) instead of
    /// naming a subdir. Optionally pass a single `<name>` to name it; any
    /// virtual-package flags override the detected values. The detected
    /// platform is placed at the top of the list.
    #[clap(long, visible_alias = "auto-detected", visible_alias = "current")]
    pub auto_detect: bool,

    #[clap(flatten)]
    pub virtual_packages: VirtualPackageArgs,

    /// Don't update the environment, only add changed packages to the
    /// lock file.
    #[clap(long, env = "PIXI_NO_INSTALL")]
    pub no_install: bool,

    /// The name of the feature to add the platform to.
    #[clap(long, short)]
    pub feature: Option<FeatureName>,

    /// The environment to add the platform to. The platform is written to
    /// the platforms defined inline on the environment.
    #[clap(long, short, conflicts_with = "feature")]
    pub environment: Option<EnvironmentName>,
}

#[derive(Parser, Debug)]
pub struct EditArgs {
    /// Name of the platform to edit.
    pub name: PixiPlatformName,

    /// Arbitrary virtual-package specs (`name=version[=build_string]`) to
    /// declare or update, with an optional canonical `__` prefix. Use the
    /// built-in flags (`--cuda`, `--archspec`, ...) for specialized syntax.
    #[clap(value_name = "NAME=VERSION[=BUILD]")]
    pub custom_virtual_packages: Vec<String>,

    /// Set a new conda subdir for this platform.
    #[clap(long, value_name = "SUBDIR")]
    pub subdir: Option<Subdir>,

    #[clap(flatten)]
    pub virtual_packages: VirtualPackageArgs,

    /// Remove a virtual package by friendly or canonical name. Can be repeated.
    #[clap(long = "remove-virtual-package", value_name = "NAME", num_args = 1)]
    pub remove_virtual_packages: Vec<String>,

    /// Clear all virtual packages before applying any add/upsert operations.
    #[clap(long)]
    pub clear_virtual_packages: bool,

    /// Don't update the environment, only refresh the lock-file.
    #[clap(long, env = "PIXI_NO_INSTALL")]
    pub no_install: bool,
}

/// Reorder a workspace platform. Exactly one of `--before`, `--after`,
/// `--to-top`, `--to-bottom` is required. Order is selection priority: the
/// first declared platform the current machine can run is the one used.
#[derive(Parser, Debug)]
#[clap(group = clap::ArgGroup::new("anchor").required(true).multiple(false))]
pub struct MoveArgs {
    /// Name of the platform to move.
    pub name: PixiPlatformName,

    /// Move it directly before this platform.
    #[clap(long, value_name = "PLATFORM", group = "anchor")]
    pub before: Option<PixiPlatformName>,

    /// Move it directly after this platform.
    #[clap(long, value_name = "PLATFORM", group = "anchor")]
    pub after: Option<PixiPlatformName>,

    /// Move it to the top of the list (highest selection priority).
    #[clap(long, group = "anchor")]
    pub to_top: bool,

    /// Move it to the bottom of the list (lowest selection priority).
    #[clap(long, group = "anchor")]
    pub to_bottom: bool,

    /// Don't update the environment, only refresh the lock-file.
    #[clap(long, env = "PIXI_NO_INSTALL")]
    pub no_install: bool,
}

#[derive(Parser, Debug, Default)]
pub struct RemoveArgs {
    /// The platform name(s) to remove.
    #[clap(required = true, num_args=1.., value_name = "PLATFORM")]
    pub platforms: Vec<PixiPlatformName>,

    /// Don't update the environment, only remove the platform(s) from the
    /// lock file.
    #[clap(long, env = "PIXI_NO_INSTALL")]
    pub no_install: bool,

    /// The name of the feature to remove the platform from.
    #[clap(long, short)]
    pub feature: Option<FeatureName>,

    /// The environment to remove the platform from. The platform is removed
    /// from the platforms defined inline on the environment.
    #[clap(long, short, conflicts_with = "feature")]
    pub environment: Option<EnvironmentName>,
}

#[derive(Parser, Debug, Default)]
pub struct ListArgs {
    /// Emit machine-readable JSON instead of the human view.
    #[clap(long)]
    pub json: bool,

    /// Output the workspace platform names in machine readable format (space
    /// delimited). This output is used for autocomplete.
    #[arg(long, hide(true), conflicts_with = "json")]
    pub machine_readable: bool,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Adds a platform(s) to the workspace file and updates the lock file.
    #[clap(visible_alias = "a")]
    Add(AddArgs),
    /// Edit an existing workspace platform's subdir and/or virtual packages.
    #[clap(visible_alias = "e")]
    Edit(EditArgs),
    /// Reorder a workspace platform, changing its selection priority.
    #[clap(visible_alias = "mv")]
    Move(MoveArgs),
    /// List every workspace platform with full detail, preceded by the
    /// auto-detected host as a separate entry.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove platform(s) from the workspace file and updates the lock file.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

impl Args {
    fn validate_script_options(&self) -> miette::Result<()> {
        if self.workspace_config.script.is_none() {
            return Ok(());
        }

        let (feature, environment) = match &self.command {
            Command::Add(args) => (&args.feature, &args.environment),
            Command::Remove(args) => (&args.feature, &args.environment),
            Command::Edit(_) | Command::Move(_) | Command::List(_) => return Ok(()),
        };

        let mut unsupported = Vec::new();
        if feature.is_some() {
            unsupported.push("--feature");
        }
        if environment.is_some() {
            unsupported.push("--environment");
        }

        if unsupported.is_empty() {
            Ok(())
        } else {
            Err(miette::miette!(
                help = "A PEP 723 script has one implicit default run environment.",
                "`pixi workspace platform --script` does not support {}",
                unsupported.join(", ")
            ))
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    args.validate_script_options()?;

    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()
        .await?;

    let lock_file_usage = script_lock_file_usage(
        LockFileUsage::Update,
        args.workspace_config.script.is_some(),
        workspace.lock_file_path().is_file(),
    )?;
    let workspace_ctx = cli_context(workspace.clone());

    // Keep mutation state off the enclosing CLI dispatch futures.
    match args.command {
        Command::Add(args) => Box::pin(execute_add(&workspace_ctx, args, lock_file_usage)).await,
        Command::Edit(args) => Box::pin(execute_edit(&workspace_ctx, args, lock_file_usage)).await,
        Command::Move(args) => Box::pin(execute_move(&workspace_ctx, args, lock_file_usage)).await,
        Command::List(args) => execute_list(&workspace_ctx, args).await,
        Command::Remove(args) => {
            Box::pin(execute_remove(
                &workspace,
                &workspace_ctx,
                args,
                lock_file_usage,
            ))
            .await
        }
    }
}

async fn execute_add(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: AddArgs,
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    // A right-hand side that names a conda subdir is a platform entry.
    // Everything else containing `=` is an arbitrary virtual-package spec.
    let (mut custom_specs, platform_entries): (Vec<String>, Vec<String>) = args
        .platform
        .into_iter()
        .partition(|input| is_virtual_package_positional(input));
    custom_specs.extend(args.custom_virtual_packages);

    // `--auto-detect` detects this machine instead of naming a subdir; any
    // virtual-package flags then override the detected values.
    if args.auto_detect {
        if platform_entries.len() > 1 {
            miette::bail!(
                "`--auto-detect` accepts at most one platform name; got {}",
                platform_entries.len()
            );
        }
        let explicit_name = match platform_entries.first() {
            None => None,
            Some(entry) if entry.contains('=') => miette::bail!(
                "`--auto-detect` detects this machine's subdir; pass a bare `<name>`, not `<name>=<subdir>`"
            ),
            Some(name) => Some(
                PixiPlatformName::try_from(name.as_str())
                    .into_diagnostic()
                    .map_err(|e| miette::miette!("invalid platform name '{name}': {e}"))?,
            ),
        };
        return execute_add_auto_detected(
            workspace_ctx,
            explicit_name,
            args.virtual_packages,
            &custom_specs,
            args.no_install,
            crate::cli_config::feature_from_flags(args.environment.as_ref(), args.feature.as_ref()),
            lock_file_usage,
        )
        .await;
    }

    if platform_entries.is_empty() {
        miette::bail!("at least one platform argument is required");
    }

    let virtual_packages_present = !args.virtual_packages.is_empty() || !custom_specs.is_empty();

    if virtual_packages_present && platform_entries.len() != 1 {
        miette::bail!(
            "virtual-package flags or `name=version` positionals require exactly one platform argument; got {}",
            platform_entries.len()
        );
    }

    let parsed: Vec<(PixiPlatformName, Subdir)> = platform_entries
        .iter()
        .map(|raw| parse_add_positional(raw))
        .collect::<miette::Result<_>>()?;

    // Reject duplicate platform positionals so `add linux-64 linux-64` fails
    // loudly instead of silently collapsing, mirroring the virtual-package
    // dedup above.
    let mut seen_platforms = HashSet::new();
    for (name, _) in &parsed {
        if !seen_platforms.insert(name.clone()) {
            miette::bail!("platform '{name}' was specified more than once on the command line");
        }
    }

    let mut platforms: Vec<PixiPlatform> = Vec::with_capacity(parsed.len());
    if virtual_packages_present {
        let (name, subdir) = parsed.into_iter().next().expect("len checked above");
        // Virtual packages attach to "rich" platforms only. A bare subdir
        // entry like `linux-64` is locked to mirror the underlying conda
        // subdir exactly; the model rejects mutations on these, so reject at
        // parse time before we go through any solve.
        if name.as_str() == subdir.as_str() {
            miette::bail!(
                "virtual packages require a custom platform name; use `<name>=<subdir>` (e.g. `gpu-{subdir}={subdir}`) instead of the bare subdir"
            );
        }
        let specs = args.virtual_packages.into_specs(subdir, &custom_specs)?;
        platforms.push(PixiPlatform::new_with_defaults(name, subdir, specs).into_diagnostic()?);
    } else {
        for (name, subdir) in parsed {
            platforms
                .push(PixiPlatform::new_with_defaults(name, subdir, Vec::new()).into_diagnostic()?);
        }
    }

    workspace_ctx
        .add_platforms(
            platforms,
            args.no_install,
            crate::cli_config::feature_from_flags(args.environment.as_ref(), args.feature.as_ref()),
            lock_file_usage,
        )
        .await
}

/// Detect this machine, apply any virtual-package overrides on top, and hand the
/// resulting platform to the workspace context for content-dedup, front
/// placement, and reporting.
async fn execute_add_auto_detected(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    explicit_name: Option<PixiPlatformName>,
    virtual_packages: VirtualPackageArgs,
    custom_specs: &[String],
    no_install: bool,
    feature: FeatureName,
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    let host = workspace_ctx
        .workspace()
        .detect_host_with_all_detectors()
        .await?;
    let subdir = host.subdir();
    let detected = host.platform().into_diagnostic()?.clone();
    let overrides = virtual_packages.into_specs(subdir, custom_specs)?;
    let merged = merge_virtual_packages(detected.customised_virtual_packages(), overrides);
    let explicit = explicit_name.is_some();
    let candidate =
        PixiPlatform::from_detection(explicit_name, subdir, merged).into_diagnostic()?;
    workspace_ctx
        .add_auto_detected_platform(candidate, explicit, no_install, feature, lock_file_usage)
        .await
}

/// Merge user-supplied virtual packages over detected ones: a user spec
/// replaces a detected package of the same name, the rest are kept.
fn merge_virtual_packages(
    detected: Vec<GenericVirtualPackage>,
    overrides: Vec<GenericVirtualPackage>,
) -> Vec<GenericVirtualPackage> {
    let overridden: HashSet<_> = overrides.iter().map(|p| p.name.clone()).collect();
    let mut merged: Vec<GenericVirtualPackage> = detected
        .into_iter()
        .filter(|d| !overridden.contains(&d.name))
        .collect();
    merged.extend(overrides);
    merged
}

async fn execute_edit(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: EditArgs,
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    // For `edit`, we don't yet know the platform's subdir if --subdir wasn't
    // supplied, so resolve from the workspace first.
    let subdir = match args.subdir {
        Some(s) => s,
        None => {
            let existing = workspace_ctx
                .get_workspace_platform(&args.name)
                .await
                .ok_or_else(|| {
                    miette::miette!(
                        "workspace does not define a platform named '{}'",
                        args.name.as_str()
                    )
                })?;
            existing.subdir()
        }
    };
    let insert_or_update_virtual_packages = args
        .virtual_packages
        .clone()
        .into_specs(subdir, &args.custom_virtual_packages)?;

    let remove_virtual_packages: Vec<PackageName> = args
        .remove_virtual_packages
        .iter()
        .map(|key| {
            canonical_virtual_package_name(key)
                .map_err(|error| miette::miette!("--remove-virtual-package: {error}"))
        })
        .collect::<miette::Result<_>>()?;

    let edit = PlatformEdit {
        set_subdir: args.subdir,
        clear_virtual_packages: args.clear_virtual_packages,
        insert_or_update_virtual_packages,
        remove_virtual_packages,
    };

    if edit.is_noop() {
        miette::bail!(
            "nothing to do: pass at least one of --subdir, a virtual-package flag (--cuda, --archspec, --glibc, --linux, --macos, --windows), a `name=version` positional, --remove-virtual-package, or --clear-virtual-packages"
        );
    }

    workspace_ctx
        .edit_platform(args.name, edit, args.no_install, lock_file_usage)
        .await
}

async fn execute_move(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: MoveArgs,
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    let target = match (args.to_top, args.to_bottom, args.before, args.after) {
        (true, _, _, _) => PlatformMove::ToTop,
        (_, true, _, _) => PlatformMove::ToBottom,
        (_, _, Some(before), _) => PlatformMove::Before(before),
        (_, _, _, Some(after)) => PlatformMove::After(after),
        _ => unreachable!("clap's required, exclusive 'anchor' group guarantees one is set"),
    };

    workspace_ctx
        .move_platform(args.name, target, args.no_install, lock_file_usage)
        .await
}

/// Print every workspace platform in full detail, preceded by the
/// auto-detected host as a separate (synthetic) entry. The host comes first
/// so users see what their machine reports before the manifest's declared
/// view. Workspace platforms are emitted in declaration order, separated
/// from the host entry by a dim `---` line in the human view.
async fn execute_list(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: ListArgs,
) -> miette::Result<()> {
    let workspace = workspace_ctx.workspace();
    let workspace_platforms: Vec<PixiPlatform> = workspace
        .workspace_manifest()
        .workspace
        .platforms
        .iter()
        .cloned()
        .collect();

    if args.machine_readable {
        let names = workspace_platforms
            .iter()
            .map(|p| p.name().as_str())
            .collect::<Vec<_>>()
            .join(" ");
        pixi_utils::io::ignore_broken_pipe(writeln!(std::io::stdout(), "{names}"))
            .into_diagnostic()?;
        return Ok(());
    }

    let host = workspace.detect_host_with_all_detectors().await?;
    let machine = HostMachine::from_host(&host);

    if args.json {
        let mut platforms: Vec<serde_json::Value> =
            Vec::with_capacity(workspace_platforms.len() + 1);
        platforms.push(autodetected_to_json(&machine));
        // Reuse the complete host detection, and probe each other subdir once.
        let mut probed: HashMap<Subdir, Vec<GenericVirtualPackage>> = HashMap::new();
        for p in &workspace_platforms {
            let detected: &[GenericVirtualPackage] = if p.subdir() == machine.subdir {
                &machine.detected
            } else {
                probed
                    .entry(p.subdir())
                    .or_insert_with(|| machine_virtual_packages(p.subdir()))
            };
            let users = environments_and_features_using(workspace, p);
            platforms.push(show_to_json(p, &users, detected));
        }

        let value = serde_json::json!({
            "current_subdir": machine.subdir.as_str(),
            "platforms": platforms,
        });
        let _ = writeln!(
            std::io::stdout(),
            "{}",
            serde_json::to_string_pretty(&value).into_diagnostic()?
        );
        return Ok(());
    }

    let mut stdout = std::io::stdout();
    print_autodetected_host(&mut stdout, &machine);

    if !workspace_platforms.is_empty() {
        let _ = writeln!(stdout, "\n{}", console::style("Platforms:").bold().bright());
    }
    let _ = write!(
        stdout,
        "{}",
        format_workspace_platform_rows(workspace, &machine)
    );

    Ok(())
}

/// Renders the rows under the `Platforms:` header: every workspace platform
/// in declaration order, each followed by its usage lines.
fn format_workspace_platform_rows(
    workspace: &pixi_core::Workspace,
    machine: &HostMachine,
) -> String {
    let reachability = MachineReachability::compute(workspace, machine);
    let multiple_environments = workspace.environments().len() > 1;
    workspace
        .workspace_manifest()
        .workspace
        .platforms
        .iter()
        .map(|p| {
            let users = environments_and_features_using(workspace, p);
            format_workspace_platform_row(p, machine, &users, &reachability, multiple_environments)
        })
        .collect()
}

async fn execute_remove(
    workspace: &pixi_core::Workspace,
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: RemoveArgs,
    lock_file_usage: LockFileUsage,
) -> miette::Result<()> {
    let workspace_platforms = workspace.workspace_manifest().workspace.platforms.clone();
    let platforms = args
        .platforms
        .iter()
        .map(|name| {
            workspace_platforms
                .iter()
                .find(|p| p.name() == name)
                .cloned()
                .ok_or_else(|| {
                    miette::miette!(
                        "workspace does not define a platform named '{}'",
                        name.as_str()
                    )
                })
        })
        .collect::<miette::Result<Vec<_>>>()?;
    workspace_ctx
        .remove_platforms(
            platforms,
            args.no_install,
            crate::cli_config::feature_from_flags(args.environment.as_ref(), args.feature.as_ref()),
            lock_file_usage,
        )
        .await
}

/// Pretty-print rattler's host detection as a "diagnostic" header rather
/// than another `<name>:` row.
fn print_autodetected_host(stdout: &mut std::io::Stdout, machine: &HostMachine) {
    let _ = writeln!(stdout, "Your current machine was detected as:");
    let _ = writeln!(
        stdout,
        "    {}",
        inline_entry_body(machine.subdir, &machine.detected)
    );
}

/// Walk all environments + features in the workspace and collect the names of
/// those that reference `platform` by name. Used by the `list` output so the
/// user can see what would break if they remove the entry. Platforms declared
/// inline on an environment live on its synthesized feature; those are
/// reported as inline environment declarations, not as features.
fn environments_and_features_using(
    workspace: &pixi_core::Workspace,
    platform: &PixiPlatform,
) -> PlatformUsers {
    let mut features = Vec::new();
    let mut inline_environments = Vec::new();
    let mut environments = Vec::new();
    let manifest = workspace.workspace_manifest();
    let name = platform.name();

    for (feature_name, feature) in manifest.all_features() {
        if let Some(platforms) = &feature.platforms
            && platforms.contains(name)
        {
            if let Some(environment_name) = feature_name.environment_name() {
                inline_environments.push(environment_name.to_string());
            } else {
                features.push(feature_name.to_string());
            }
        }
    }

    for env in workspace.environments() {
        if env.platforms().contains(name) {
            environments.push(env.name().to_string());
        }
    }

    PlatformUsers {
        features,
        inline_environments,
        environments,
    }
}

struct PlatformUsers {
    features: Vec<String>,
    /// Environments that declare the platform inline (on their synthesized
    /// feature) rather than through a `[feature.<name>]` table.
    inline_environments: Vec<String>,
    environments: Vec<String>,
}

/// Snapshot of the local machine used to color platform rows in `list`:
/// the subdir we target, which subdirs we can run packages from (that one plus
/// arch fallbacks) and which virtual packages rattler detected on the host.
struct HostMachine {
    subdir: Subdir,
    candidate_subdirs: Vec<Subdir>,
    detected: Vec<GenericVirtualPackage>,
}

impl HostMachine {
    fn from_host(host: &pixi_core::host::HostDetection) -> Self {
        let subdir = host.subdir();
        let candidate_subdirs = candidate_subdirs(subdir);
        let detected = host.capabilities().to_vec();
        HostMachine {
            subdir,
            candidate_subdirs,
            detected,
        }
    }

    /// `true` when a platform with this subdir can actually run on the
    /// current host -- includes architecture fallbacks (`Win64` → `Win32`,
    /// `Osx*` → `Osx64`).
    fn covers_subdir(&self, subdir: Subdir) -> bool {
        self.candidate_subdirs.contains(&subdir)
    }

    /// `true` when the host provides the capability `declared` names.
    ///
    /// Shares [`capability_satisfied_by`] with the selection machinery, so what
    /// `list` calls supported is what `run` will actually pick. Rolling the
    /// version comparison by hand here silently disagreed about `__archspec`,
    /// which is matched by microarchitecture rather than by version.
    fn satisfies(&self, declared: &GenericVirtualPackage) -> bool {
        capability_satisfied_by(declared, &self.detected)
    }

    /// Does the current machine support running this platform? Combines
    /// the subdir check with the per-VP satisfaction check on the user-
    /// customized virtual packages (subdir defaults are pixi's baseline
    /// and not considered host requirements). Used to color both the
    /// row itself and the env/feature names that reference it.
    fn supports(&self, platform: &PixiPlatform) -> bool {
        let subdir = platform.subdir();
        if !self.covers_subdir(subdir) {
            return false;
        }
        pixi_manifest::toml::inline_virtual_package_specs(
            platform.declared_virtual_packages(),
            Some(&subdir_default_virtual_packages(subdir)),
        )
        .iter()
        .all(|spec| spec.packages.iter().all(|p| self.satisfies(p)))
    }
}

/// Names of environments and features that have no platform supported by
/// the current machine. Used to dim those names in the `Used in ...`
/// continuation lines so they stand out as "won't run here".
struct MachineReachability {
    unreachable_environments: HashSet<String>,
    unreachable_features: HashSet<String>,
}

impl MachineReachability {
    fn compute(workspace: &pixi_core::Workspace, machine: &HostMachine) -> Self {
        let manifest = workspace.workspace_manifest();
        let supported: HashSet<&str> = manifest
            .workspace
            .platforms
            .iter()
            .filter(|p| machine.supports(p))
            .map(|p| p.name().as_str())
            .collect();

        let unreachable_environments: HashSet<String> = workspace
            .environments()
            .iter()
            .filter(|env| {
                !env.platforms()
                    .iter()
                    .any(|name| supported.contains(name.as_str()))
            })
            .map(|env| env.name().to_string())
            .collect();

        let unreachable_features: HashSet<String> = manifest
            .user_features()
            .filter_map(|(name, feat)| {
                // Only features that pin a `platforms = [...]` list can be
                // "unreachable"; an implicit-platforms feature inherits
                // the workspace's set and is reachable iff any workspace
                // platform is reachable.
                let platforms = feat.platforms.as_ref()?;
                let reachable = platforms.iter().any(|n| supported.contains(n.as_str()));
                (!reachable).then(|| name.to_string())
            })
            .collect();

        MachineReachability {
            unreachable_environments,
            unreachable_features,
        }
    }
}

/// One row in the `Platforms:` block. Supported platforms are bold; blocking
/// subdir / virtual packages are dimmed. Followed by indented usage lines:
/// `Used in environments:` (only when the workspace has more than one
/// environment), `Used in features    :`, and `Declared inline in
/// environments:`, each emitted only when the manifest references the
/// platform, with unreachable names dimmed.
fn format_workspace_platform_row(
    platform: &PixiPlatform,
    machine: &HostMachine,
    users: &PlatformUsers,
    reachability: &MachineReachability,
    multiple_environments: bool,
) -> String {
    let subdir = platform.subdir();
    let subdir_ok = machine.covers_subdir(subdir);

    let mut parts: Vec<String> = Vec::new();
    let subdir_text = format!("platform={}", subdir.as_str());
    parts.push(if subdir_ok {
        subdir_text
    } else {
        console::style(subdir_text).dim().to_string()
    });

    let mut all_vps_ok = true;
    for spec in pixi_manifest::toml::inline_virtual_package_specs(
        platform.declared_virtual_packages(),
        Some(&subdir_default_virtual_packages(subdir)),
    ) {
        let satisfied = spec.packages.iter().all(|p| machine.satisfies(p));
        if !satisfied {
            all_vps_ok = false;
        }
        parts.push(if satisfied {
            spec.rendered
        } else {
            console::style(spec.rendered).dim().to_string()
        });
    }

    let supported = subdir_ok && all_vps_ok;
    let name_styled = if supported {
        console::style(platform.name().as_str()).bold().bright()
    } else {
        // Unstyled but kept as the rest of the row's prefix; without this
        // the unsupported names blend in with the body keys.
        console::style(platform.name().as_str())
    };
    let suffix = if supported {
        " (supported by current machine)"
    } else {
        ""
    };
    let mut row = format!("{name_styled}: {body}{suffix}\n", body = parts.join(", "),);
    // Indented usage lines. The labels are padded so the two colons line
    // up when both are emitted; either is omitted if nothing references
    // the platform from that side. Names of environments/features that
    // have no reachable platform on this machine are dimmed so users can
    // see at a glance which references they can act on locally.
    if multiple_environments && !users.environments.is_empty() {
        row.push_str(&format!(
            "    Used in environments: {}\n",
            format_user_names(&users.environments, &reachability.unreachable_environments),
        ));
    }
    if !users.features.is_empty() {
        row.push_str(&format!(
            "    Used in features    : {}\n",
            format_user_names(&users.features, &reachability.unreachable_features),
        ));
    }
    if !users.inline_environments.is_empty() {
        row.push_str(&format!(
            "    Declared inline in environments: {}\n",
            format_user_names(
                &users.inline_environments,
                &reachability.unreachable_environments
            ),
        ));
    }
    row
}

/// Join `names` as a comma-separated list, dimming any entry that's in
/// `unreachable`.
fn format_user_names(names: &[String], unreachable: &HashSet<String>) -> String {
    names
        .iter()
        .map(|name| {
            if unreachable.contains(name) {
                console::style(name).dim().to_string()
            } else {
                name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Plain (no styling) `platform=...[, key=value, ...]` body used by the
/// host-detection header. The header is informational, so the body is
/// emitted verbatim without the match-aware dimming the workspace rows
/// use.
fn inline_entry_body(subdir: Subdir, declared: &[GenericVirtualPackage]) -> String {
    let mut parts = vec![format!("platform={}", subdir.as_str())];
    parts.extend(render_friendly(
        declared,
        Some(&subdir_default_virtual_packages(subdir)),
    ));
    parts.join(", ")
}

/// Render `declared` virtual packages in the friendly `key=value` form used
/// consistently across `pixi info` and `pixi workspace platform` text and
/// JSON output. When `baseline` (the subdir defaults) is given, entries
/// matching it are filtered out.
fn render_friendly(
    declared: &[GenericVirtualPackage],
    baseline: Option<&[GenericVirtualPackage]>,
) -> Vec<String> {
    pixi_manifest::toml::inline_virtual_package_specs(declared, baseline)
        .into_iter()
        .map(|spec| spec.rendered)
        .collect()
}

/// `detected` is what this machine reports for the row's subdir, so a consumer
/// can diff it against the row's declared packages. Sparse for a subdir this
/// machine cannot speak about, which is the honest answer.
fn show_to_json(
    platform: &PixiPlatform,
    users: &PlatformUsers,
    detected: &[GenericVirtualPackage],
) -> serde_json::Value {
    let detected: Vec<String> = render_friendly(detected, None);
    serde_json::json!({
        "name": platform.name().as_str(),
        "subdir": platform.subdir().as_str(),
        "virtual_packages": render_friendly(
            platform.declared_virtual_packages(),
            Some(&subdir_default_virtual_packages(platform.subdir())),
        ),
        "detected_virtual_packages": detected,
        "features": users.features,
        "declared_inline_in_environments": users.inline_environments,
        "environments": users.environments,
    })
}

/// JSON counterpart to [`print_autodetected_host`]. Carries the same data
/// shape as a real platform entry plus an `is_autodetected: true` marker so
/// downstream tooling can tell synthetic rows apart from declared ones.
fn autodetected_to_json(machine: &HostMachine) -> serde_json::Value {
    let detected: Vec<String> = render_friendly(&machine.detected, None);
    serde_json::json!({
        "name": "current",
        "subdir": machine.subdir.as_str(),
        "virtual_packages": Vec::<String>::new(),
        "detected_virtual_packages": detected,
        "features": Vec::<String>::new(),
        "declared_inline_in_environments": Vec::<String>::new(),
        "environments": Vec::<String>::new(),
        "is_current": true,
        "is_autodetected": true,
    })
}

#[cfg(test)]
mod tests {
    use pixi_core::Workspace;

    use super::*;

    /// A workspace where a real feature and an inline environment both
    /// declare the sole workspace platform.
    fn inline_declaration_workspace() -> Workspace {
        Workspace::from_str(
            std::path::Path::new("pixi.toml"),
            r#"
            [workspace]
            name = "platform-test"
            channels = []
            platforms = ["linux-64"]

            [feature.cuda]
            platforms = ["linux-64"]

            [environments]
            gpu = ["cuda"]

            [environments.dev]
            platforms = ["linux-64"]

            [environments.dev.dependencies]
            git = "*"
            "#,
        )
        .unwrap()
    }

    /// A host that runs linux-64 with no customised virtual packages.
    fn linux_machine() -> HostMachine {
        HostMachine {
            subdir: Subdir::Linux64,
            candidate_subdirs: vec![Subdir::Linux64],
            detected: Vec::new(),
        }
    }

    #[test]
    fn list_reports_inline_environment_declarations_separately() {
        let workspace = inline_declaration_workspace();
        let rows = format_workspace_platform_rows(&workspace, &linux_machine());
        insta::assert_snapshot!(rows, @r"
        linux-64: platform=linux-64 (supported by current machine)
            Used in environments: default, gpu, dev
            Used in features    : cuda
            Declared inline in environments: dev
        ");
    }

    #[test]
    fn list_json_reports_inline_environment_declarations_separately() {
        let workspace = inline_declaration_workspace();
        let platform = (&workspace)
            .workspace_manifest()
            .workspace
            .platforms
            .iter()
            .next()
            .expect("manifest declares one platform");
        let users = environments_and_features_using(&workspace, platform);
        let json = show_to_json(platform, &users, &[]);
        assert_eq!(json["features"], serde_json::json!(["cuda"]));
        assert_eq!(
            json["declared_inline_in_environments"],
            serde_json::json!(["dev"])
        );
        assert_eq!(
            json["environments"],
            serde_json::json!(["default", "gpu", "dev"])
        );
    }

    #[test]
    fn into_specs_rejects_glibc_on_windows() {
        let args = VirtualPackageArgs {
            glibc: Some("2.28".into()),
            ..Default::default()
        };
        let err = args.into_specs(Subdir::Win64, &[]).unwrap_err();
        assert!(
            err.to_string()
                .contains("--glibc only applies to linux subdirs"),
            "{err}"
        );
    }

    #[test]
    fn into_specs_rejects_macos_on_linux() {
        let args = VirtualPackageArgs {
            macos: Some("14.0".into()),
            ..Default::default()
        };
        let err = args.into_specs(Subdir::Linux64, &[]).unwrap_err();
        assert!(
            err.to_string()
                .contains("--macos only applies to osx subdirs"),
            "{err}"
        );
    }

    #[test]
    fn into_specs_accepts_glibc_on_linux() {
        let args = VirtualPackageArgs {
            glibc: Some("2.28".into()),
            ..Default::default()
        };
        let specs = args.into_specs(Subdir::Linux64, &[]).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name.as_normalized(), "__glibc");
        assert_eq!(specs[0].version.to_string(), "2.28");
    }

    #[test]
    fn into_specs_cuda_and_cuda_arch_produce_both_packages() {
        let args = VirtualPackageArgs {
            cuda: Some("12.0".into()),
            cuda_arch: Some("8.6".into()),
            ..Default::default()
        };
        let specs = args.into_specs(Subdir::Linux64, &[]).unwrap();
        let by_name: std::collections::HashMap<_, _> = specs
            .iter()
            .map(|s| (s.name.as_normalized(), s.version.to_string()))
            .collect();
        assert_eq!(by_name.get("__cuda").map(String::as_str), Some("12.0"));
        assert_eq!(by_name.get("__cuda_arch").map(String::as_str), Some("8.6"));
    }

    #[test]
    fn parse_virtual_package_spec_adds_prefix_and_keeps_build_string() {
        let package = parse_virtual_package_spec("site_service=1=special=build").unwrap();
        assert_eq!(package.name.as_normalized(), "__site_service");
        assert_eq!(package.build_string, "special=build");
    }

    #[test]
    fn into_specs_rejects_unknown_archspec() {
        let args = VirtualPackageArgs {
            archspec: Some("x86-64-v3".into()),
            ..Default::default()
        };
        let error = args.into_specs(Subdir::Linux64, &[]).unwrap_err();
        assert!(
            error.to_string().contains("did you mean 'x86_64_v3'"),
            "{error}"
        );
        let error = parse_virtual_package_spec("archspec=0=x86_64_v3=oops").unwrap_err();
        assert!(
            error.to_string().contains("not a known archspec"),
            "{error}"
        );
    }

    #[test]
    fn into_specs_rejects_custom_positional_duplicate_of_builtin_flag() {
        let args = VirtualPackageArgs {
            cuda: Some("12.0".into()),
            ..Default::default()
        };
        let error = args
            .into_specs(Subdir::Linux64, &["__cuda=11.0".to_string()])
            .unwrap_err();
        assert!(error.to_string().contains("more than once"), "{error}");
    }

    #[test]
    fn add_positionals_distinguish_platforms_from_virtual_packages() {
        assert!(!is_virtual_package_positional("gpu=linux-64"));
        assert!(is_virtual_package_positional("amdgpu=0"));
        assert!(is_virtual_package_positional("site_service=2=h1"));
    }

    #[test]
    fn remove_name_adds_canonical_prefix() {
        assert_eq!(
            canonical_virtual_package_name("amdgpu")
                .unwrap()
                .as_normalized(),
            "__amdgpu"
        );
    }
}
