use std::collections::HashSet;
use std::io::Write;
use std::str::FromStr;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::WorkspaceLocator;
use pixi_manifest::{
    FeaturesExt, HasWorkspaceManifest, PixiPlatform, PixiPlatformName, PlatformEdit,
};
use rattler_conda_types::{GenericVirtualPackage, PackageName, Platform, Version};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace platforms.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(subcommand)]
    pub command: Command,
}

/// Common virtual-package shortcut flags shared by `add` and `edit`. Wrapped
/// in a clap struct so the rules (parsing, validation, conversion to
/// `GenericVirtualPackage`) live in one place.
///
/// Mirrors the TOML's per-virtual-package keys (`cuda`, `archspec`, `libc`,
/// `linux`, `macos`, `windows`). Virtual packages without a friendly flag are
/// declared as trailing `__name[=version[=build_string]]` positionals on the
/// surrounding `add` / `edit` command, matching the `__name` raw-key escape
/// hatch in the TOML layer.
#[derive(Parser, Debug, Default, Clone)]
pub struct VirtualPackageArgs {
    /// Declare a `__cuda` virtual package at the given version, e.g. `12.0`.
    /// Valid on any subdir.
    #[clap(long, value_name = "VERSION")]
    pub cuda: Option<String>,

    /// Declare a `__archspec` virtual package with the given microarchitecture
    /// string, e.g. `x86_64_v3`. Valid on any subdir.
    #[clap(long, value_name = "ARCH")]
    pub archspec: Option<String>,

    /// Declare a `__glibc` virtual package at the given version, e.g. `2.28`.
    /// Only valid on linux subdirs.
    #[clap(long, value_name = "VERSION")]
    pub libc: Option<String>,

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
            && self.archspec.is_none()
            && self.libc.is_none()
            && self.linux.is_none()
            && self.macos.is_none()
            && self.windows.is_none()
    }

    /// Translate the friendly flags plus any trailing raw `__name=value`
    /// positionals into a vector of [`GenericVirtualPackage`]. `subdir` is
    /// used to reject nonsensical combinations (e.g. `--libc` on win-64).
    pub fn into_specs(
        self,
        subdir: Platform,
        raw_specs: &[String],
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
        if let Some(value) = self.archspec {
            if value.is_empty() {
                miette::bail!("--archspec requires a non-empty microarchitecture string");
            }
            push_unique(
                &mut specs,
                &mut seen_names,
                "__archspec",
                zero_version(),
                value,
            )?;
        }
        if let Some(value) = self.libc {
            require_subdir_family(subdir, Platform::is_linux, "--libc", "linux")?;
            let version = parse_virtual_package_version("--libc", &value)?;
            push_unique(
                &mut specs,
                &mut seen_names,
                "__glibc",
                version,
                String::new(),
            )?;
        }
        if let Some(value) = self.linux {
            require_subdir_family(subdir, Platform::is_linux, "--linux", "linux")?;
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
            require_subdir_family(subdir, Platform::is_osx, "--macos", "osx")?;
            let version = parse_virtual_package_version("--macos", &value)?;
            push_unique(&mut specs, &mut seen_names, "__osx", version, String::new())?;
        }
        if let Some(value) = self.windows {
            require_subdir_family(subdir, Platform::is_windows, "--windows", "win")?;
            let version = parse_virtual_package_version("--windows", &value)?;
            push_unique(&mut specs, &mut seen_names, "__win", version, String::new())?;
        }

        for raw in raw_specs {
            let gvp = parse_raw_virtual_package(raw)?;
            // Reject duplicates so the order of `--cuda 12.0 __cuda=11.0`
            // can't silently shadow the friendly value.
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
    subdir: Platform,
    predicate: impl Fn(Platform) -> bool,
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

fn parse_raw_virtual_package(spec: &str) -> miette::Result<GenericVirtualPackage> {
    let mut parts = spec.split('=');
    let name_str = parts.next().unwrap_or("");
    if !name_str.starts_with("__") {
        miette::bail!(
            "'{spec}' is not a virtual package spec: name must start with '__' (e.g. '__cuda=12.0')"
        );
    }
    let name = PackageName::try_from(name_str)
        .into_diagnostic()
        .map_err(|e| miette::miette!("'{name_str}' is not a valid virtual package name: {e}"))?;
    let version = parts
        .next()
        .map(|v| {
            Version::from_str(v)
                .into_diagnostic()
                .map_err(|e| miette::miette!("'{v}' is not a valid virtual package version: {e}"))
        })
        .transpose()?
        .unwrap_or_else(zero_version);
    let build_string = parts.next().unwrap_or("").to_string();
    Ok(GenericVirtualPackage {
        name,
        version,
        build_string,
    })
}

/// Parse a positional add argument. Accepts either a bare subdir
/// (`linux-64`) or `<name>=<subdir>` (`gpu-linux=linux-64`).
fn parse_add_positional(input: &str) -> miette::Result<(PixiPlatformName, Platform)> {
    if let Some((name, subdir)) = input.split_once('=') {
        let name = PixiPlatformName::try_from(name)
            .into_diagnostic()
            .map_err(|e| miette::miette!("invalid platform name '{name}': {e}"))?;
        let subdir = Platform::from_str(subdir)
            .into_diagnostic()
            .map_err(|e| miette::miette!("'{subdir}' is not a valid conda subdir: {e}"))?;
        Ok((name, subdir))
    } else {
        let subdir = Platform::from_str(input)
            .into_diagnostic()
            .map_err(|e| miette::miette!("'{input}' is not a valid conda subdir: {e}"))?;
        Ok((subdir.into(), subdir))
    }
}

#[derive(Parser, Debug, Default)]
pub struct AddArgs {
    /// Platforms to add, optionally followed by raw virtual-package specs.
    ///
    /// Each non-`__`-prefixed entry is either a bare conda subdir
    /// (`linux-64`) or `<name>=<subdir>` for a custom-named platform
    /// (`gpu-linux=linux-64`).
    ///
    /// Each `__`-prefixed entry is a raw virtual-package spec
    /// (`__name[=version[=build_string]]`) and is attached to the
    /// (single) custom-named platform in the same invocation. This mirrors
    /// the `__name = "..."` raw-key escape hatch in pixi.toml for virtual
    /// packages without a friendly flag (`--cuda`, `--archspec`, ...).
    ///
    /// When any virtual-package (friendly flag or raw spec) is set, exactly
    /// one platform may be given.
    #[clap(
        required = true,
        num_args=1..,
        value_name = "PLATFORM|NAME=PLATFORM|__NAME[=VERSION[=BUILD]]",
    )]
    pub platform: Vec<String>,

    #[clap(flatten)]
    pub virtual_packages: VirtualPackageArgs,

    /// Don't update the environment, only add changed packages to the
    /// lock file.
    #[clap(long, env = "PIXI_NO_INSTALL")]
    pub no_install: bool,

    /// The name of the feature to add the platform to.
    #[clap(long, short)]
    pub feature: Option<String>,
}

#[derive(Parser, Debug)]
pub struct EditArgs {
    /// Name of the platform to edit.
    pub name: PixiPlatformName,

    /// Raw virtual-package specs (`__name[=version[=build_string]]`) to
    /// declare or update on this platform. Use the friendly flags
    /// (`--cuda`, `--archspec`, ...) for virtual packages that have one;
    /// this trailing positional list is the escape hatch for everything
    /// else, mirroring the `__name = "..."` raw keys accepted in pixi.toml.
    #[clap(value_name = "__NAME[=VERSION[=BUILD]]")]
    pub raw_virtual_packages: Vec<String>,

    /// Set a new conda subdir for this platform.
    #[clap(long, value_name = "SUBDIR")]
    pub subdir: Option<Platform>,

    #[clap(flatten)]
    pub virtual_packages: VirtualPackageArgs,

    /// Remove the named virtual package from this platform. Can be repeated.
    #[clap(long = "remove-virtual-package", value_name = "NAME", num_args = 1)]
    pub remove_virtual_packages: Vec<String>,

    /// Clear all virtual packages before applying any add/upsert operations.
    #[clap(long)]
    pub clear_virtual_packages: bool,

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
    pub feature: Option<String>,
}

#[derive(Parser, Debug, Default)]
pub struct ListArgs {
    /// Emit machine-readable JSON instead of the human view.
    #[clap(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Adds a platform(s) to the workspace file and updates the lock file.
    #[clap(visible_alias = "a")]
    Add(AddArgs),
    /// Edit an existing workspace platform's subdir and/or virtual packages.
    #[clap(visible_alias = "e")]
    Edit(EditArgs),
    /// List every workspace platform with full detail, preceded by the
    /// auto-detected host as a separate entry.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove platform(s) from the workspace file and updates the lock file.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace.clone());

    match args.command {
        Command::Add(args) => execute_add(&workspace_ctx, args).await,
        Command::Edit(args) => execute_edit(&workspace_ctx, args).await,
        Command::List(args) => execute_list(&workspace_ctx, args).await,
        Command::Remove(args) => execute_remove(&workspace, &workspace_ctx, args).await,
    }
}

async fn execute_add(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: AddArgs,
) -> miette::Result<()> {
    // Positionals beginning with `__` are raw virtual-package specs; the rest
    // are platform entries. The split mirrors the TOML's `__name = "..."`
    // raw-key form.
    let (raw_specs, platform_entries): (Vec<String>, Vec<String>) =
        args.platform.into_iter().partition(|s| s.starts_with("__"));
    let virtual_packages_present = !args.virtual_packages.is_empty() || !raw_specs.is_empty();

    if virtual_packages_present && platform_entries.len() != 1 {
        miette::bail!(
            "virtual-package flags or `__name=value` positionals require exactly one platform argument; got {}",
            platform_entries.len()
        );
    }

    let parsed: Vec<(PixiPlatformName, Platform)> = platform_entries
        .iter()
        .map(|raw| parse_add_positional(raw))
        .collect::<miette::Result<_>>()?;

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
        let specs = args.virtual_packages.into_specs(subdir, &raw_specs)?;
        platforms.push(PixiPlatform::new_with_defaults(name, subdir, specs).into_diagnostic()?);
    } else {
        for (name, subdir) in parsed {
            platforms
                .push(PixiPlatform::new_with_defaults(name, subdir, Vec::new()).into_diagnostic()?);
        }
    }

    workspace_ctx
        .add_platforms(platforms, args.no_install, args.feature)
        .await
}

async fn execute_edit(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: EditArgs,
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
        .into_specs(subdir, &args.raw_virtual_packages)?;

    let remove_virtual_packages: Vec<PackageName> = args
        .remove_virtual_packages
        .iter()
        .map(|raw| {
            PackageName::try_from(raw.as_str())
                .into_diagnostic()
                .map_err(|e| {
                    miette::miette!("--remove-virtual-package: '{raw}' is not a valid name: {e}")
                })
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
            "nothing to do: pass at least one of --subdir, a virtual-package flag (--cuda, --archspec, --libc, --linux, --macos, --windows), a `__name=value` positional, --remove-virtual-package, or --clear-virtual-packages"
        );
    }

    workspace_ctx
        .edit_platform(args.name, edit, args.no_install)
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

    if args.json {
        let mut platforms: Vec<serde_json::Value> =
            Vec::with_capacity(workspace_platforms.len() + 1);
        platforms.push(autodetected_to_json());
        for p in &workspace_platforms {
            let users = environments_and_features_using(workspace, p);
            platforms.push(show_to_json(p, &users));
        }

        let value = serde_json::json!({
            "current_subdir": Platform::current().as_str(),
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
    print_autodetected_host();

    if !workspace_platforms.is_empty() {
        let _ = writeln!(stdout, "\n{}", console::style("Platforms:").bold().bright());
    }
    let machine = HostMachine::detect(workspace);
    let reachability = MachineReachability::compute(workspace, &machine);
    let multiple_environments = workspace.environments().len() > 1;
    for p in workspace_platforms.iter() {
        let users = environments_and_features_using(workspace, p);
        print_workspace_platform_row(p, &machine, &users, &reachability, multiple_environments);
    }

    Ok(())
}

async fn execute_remove(
    workspace: &pixi_core::Workspace,
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: RemoveArgs,
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
        .remove_platforms(platforms, args.no_install, args.feature)
        .await
}

/// Resolve the subdir `list` should treat as "current": the host's real
/// subdir unless the user has set `PIXI_OVERRIDE_PLATFORM`. Pixi's other
/// CLI paths use the same env var to cross-target a different platform
/// (see `pixi_core::workspace::environment::current_platform_with_override`),
/// and `list` should agree with them.
fn current_platform_with_override() -> Platform {
    std::env::var(pixi_consts::consts::PIXI_OVERRIDE_PLATFORM)
        .ok()
        .and_then(|val| val.parse::<Platform>().ok())
        .unwrap_or_else(Platform::current)
}

/// Pretty-print rattler's host detection as a "diagnostic" header rather
/// than another `<name>:` row -- the host has no manifest-side identity, so
/// labelling it `current:` was misleading. The body is the same
/// `platform=...[, ...]` payload the workspace rows use; subdir defaults
/// filter out so it only mentions where the host diverges from pixi's
/// baseline. Both `PIXI_OVERRIDE_PLATFORM` and the `CONDA_OVERRIDE_*`
/// virtual-package overrides are respected here so the header agrees
/// with what the workspace rows are matched against.
fn print_autodetected_host() {
    let subdir = current_platform_with_override();
    let detected: Vec<GenericVirtualPackage> =
        VirtualPackages::detect_for_platform(subdir, &VirtualPackageOverrides::from_env())
            .map(|d| d.into_generic_virtual_packages().collect())
            .unwrap_or_default();
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "Your current machine was detected as:");
    let _ = writeln!(stdout, "    {}", inline_entry_body(subdir, &detected));
}

/// Walk all environments + features in the workspace and collect the names of
/// those that reference `platform` by name. Used by the `show` output so the
/// user can see what would break if they remove the entry.
fn environments_and_features_using(
    workspace: &pixi_core::Workspace,
    platform: &PixiPlatform,
) -> PlatformUsers {
    let mut features = Vec::new();
    let mut environments = Vec::new();
    let manifest = workspace.workspace_manifest();
    let name = platform.name();

    for (feature_name, feature) in &manifest.features {
        if let Some(platforms) = &feature.platforms
            && platforms.contains(name)
        {
            features.push(feature_name.to_string());
        }
    }

    for env in workspace.environments() {
        if env.platforms().contains(name) {
            environments.push(env.name().to_string());
        }
    }

    PlatformUsers {
        features,
        environments,
    }
}

struct PlatformUsers {
    features: Vec<String>,
    environments: Vec<String>,
}

/// Snapshot of the local machine used to colour platform rows in `list`:
/// which subdirs we can run packages from (current + arch fallbacks) and
/// which virtual packages rattler detected on the host.
struct HostMachine {
    candidate_subdirs: Vec<Platform>,
    detected: Vec<GenericVirtualPackage>,
}

impl HostMachine {
    fn detect(workspace: &pixi_core::Workspace) -> Self {
        // Honor `PIXI_OVERRIDE_PLATFORM` so `list` reflects what the user
        // is targeting, not just the literal machine they're typing on.
        // Mirrors `pixi_core::workspace::environment::current_platform_with_override`.
        let current = current_platform_with_override();
        let candidate_subdirs = workspace
            .workspace_manifest()
            .workspace
            .candidate_subdirs(current);
        // `VirtualPackageOverrides::from_env()` reads the `CONDA_OVERRIDE_*`
        // family (`CUDA`, `GLIBC`, `OSX`, `WIN`, `ARCHSPEC`, ...). Calling
        // rattler's detector directly here is the same path
        // `detect_system_virtual_packages` takes in `pixi_core`; we can't
        // route through `PixiPlatform::virtual_packages()` because that
        // method only takes the platform's manifest-declared overrides.
        let detected =
            VirtualPackages::detect_for_platform(current, &VirtualPackageOverrides::from_env())
                .map(|d| d.into_generic_virtual_packages().collect::<Vec<_>>())
                .unwrap_or_default();
        HostMachine {
            candidate_subdirs,
            detected,
        }
    }

    /// `true` when a platform with this subdir can actually run on the
    /// current host -- includes architecture fallbacks (`Win64` → `Win32`,
    /// `Osx*` → `Osx64`).
    fn covers_subdir(&self, subdir: Platform) -> bool {
        self.candidate_subdirs.contains(&subdir)
    }

    /// `true` when the host advertises a virtual package whose version is
    /// at least the declared one (conda virtual-package semantics).
    fn satisfies(&self, declared: &GenericVirtualPackage) -> bool {
        self.detected
            .iter()
            .find(|h| h.name == declared.name)
            .is_some_and(|h| h.version >= declared.version)
    }

    /// Does the current machine support running this platform? Combines
    /// the subdir check with the per-VP satisfaction check on the user-
    /// customised virtual packages (subdir defaults are pixi's baseline
    /// and not considered host requirements). Used to colour both the
    /// row itself and the env/feature names that reference it.
    fn supports(&self, platform: &PixiPlatform) -> bool {
        let subdir = platform.subdir();
        if !self.covers_subdir(subdir) {
            return false;
        }
        pixi_manifest::toml::inline_virtual_package_specs(
            subdir,
            platform.declared_virtual_packages(),
        )
        .iter()
        .all(|spec| self.satisfies(&spec.package))
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
            .features
            .iter()
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
/// environment) and `Used in features    :`, each emitted only when the
/// manifest references the platform, with unreachable names dimmed.
fn print_workspace_platform_row(
    platform: &PixiPlatform,
    machine: &HostMachine,
    users: &PlatformUsers,
    reachability: &MachineReachability,
    multiple_environments: bool,
) {
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
        subdir,
        platform.declared_virtual_packages(),
    ) {
        let satisfied = machine.satisfies(&spec.package);
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
    let mut stdout = std::io::stdout();
    let _ = writeln!(
        stdout,
        "{name_styled}: {body}{suffix}",
        body = parts.join(", "),
    );
    // Indented usage lines. The labels are padded so the two colons line
    // up when both are emitted; either is omitted if nothing references
    // the platform from that side. Names of environments/features that
    // have no reachable platform on this machine are dimmed so users can
    // see at a glance which references they can act on locally.
    if multiple_environments && !users.environments.is_empty() {
        let _ = writeln!(
            stdout,
            "    Used in environments: {}",
            format_user_names(&users.environments, &reachability.unreachable_environments),
        );
    }
    if !users.features.is_empty() {
        let _ = writeln!(
            stdout,
            "    Used in features    : {}",
            format_user_names(&users.features, &reachability.unreachable_features),
        );
    }
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
fn inline_entry_body(subdir: Platform, declared: &[GenericVirtualPackage]) -> String {
    let mut parts = vec![format!("platform={}", subdir.as_str())];
    parts.extend(
        pixi_manifest::toml::inline_virtual_package_specs(subdir, declared)
            .into_iter()
            .map(|s| s.rendered),
    );
    parts.join(", ")
}

/// Same compact form pixi_manifest writes to pixi.toml: bare name when version
/// and build are zero, `name=version` when only the build is zero, otherwise
/// the full conda spec. Used by the JSON renderers so machine-readable
/// output keeps the raw `__name` form for consumers that want to round-trip.
fn format_virtual_package_short(gvp: &GenericVirtualPackage) -> String {
    let name = gvp.name.as_normalized();
    let version_is_zero = gvp.version.to_string() == "0";
    let build_is_zero = gvp.build_string.is_empty() || gvp.build_string == "0";
    if version_is_zero && build_is_zero {
        name.to_string()
    } else if build_is_zero {
        format!("{}={}", name, gvp.version)
    } else {
        gvp.to_string()
    }
}

fn show_to_json(platform: &PixiPlatform, users: &PlatformUsers) -> serde_json::Value {
    let detected: Vec<String> = match platform.virtual_packages() {
        Ok(detected) => detected
            .into_generic_virtual_packages()
            .map(|gvp| format_virtual_package_short(&gvp))
            .collect(),
        Err(_) => Vec::new(),
    };
    serde_json::json!({
        "name": platform.name().as_str(),
        "subdir": platform.subdir().as_str(),
        "virtual_packages": platform
            .declared_virtual_packages()
            .iter()
            .map(format_virtual_package_short)
            .collect::<Vec<_>>(),
        "detected_virtual_packages": detected,
        "features": users.features,
        "environments": users.environments,
    })
}

/// JSON counterpart to [`print_autodetected_host`]. Carries the same data
/// shape as a real platform entry plus an `is_autodetected: true` marker so
/// downstream tooling can tell synthetic rows apart from declared ones.
fn autodetected_to_json() -> serde_json::Value {
    let host = PixiPlatform::auto_detected(Platform::current());
    let detected: Vec<String> = match host.virtual_packages() {
        Ok(d) => d
            .into_generic_virtual_packages()
            .map(|gvp| format_virtual_package_short(&gvp))
            .collect(),
        Err(_) => Vec::new(),
    };
    serde_json::json!({
        "name": "current",
        "subdir": Platform::current().as_str(),
        "virtual_packages": Vec::<String>::new(),
        "detected_virtual_packages": detected,
        "features": Vec::<String>::new(),
        "environments": Vec::<String>::new(),
        "is_current": true,
        "is_autodetected": true,
    })
}
