use std::collections::HashSet;
use std::io::Write;
use std::str::FromStr;

use clap::Parser;
use fancy_display::FancyDisplay;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::WorkspaceLocator;
use pixi_manifest::{
    FeaturesExt, HasWorkspaceManifest, PixiPlatform, PixiPlatformName, PlatformEdit,
};
use rattler_conda_types::{GenericVirtualPackage, PackageName, Platform, Version};

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
    #[clap(long, value_name = "VERSION")]
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
pub struct ShowArgs {
    /// Name of the platform to inspect. Mutually exclusive with `--all` and
    /// `--current`.
    pub name: Option<PixiPlatformName>,

    /// Show every workspace platform. When combined with `--current`, the
    /// platforms matching the auto-detected current subdir come first,
    /// followed by a separator, then the rest.
    #[clap(long)]
    pub all: bool,

    /// Show platforms matching the auto-detected current subdir (the one
    /// best describing this machine). When combined with `--all`, the
    /// current-subdir entries appear first; on their own, only those
    /// entries are printed.
    #[clap(long)]
    pub current: bool,

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
    /// List the platforms in the workspace file.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove platform(s) from the workspace file and updates the lock file.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
    /// Show full detail for a single workspace platform.
    Show(ShowArgs),
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
        Command::Show(args) => execute_show(&workspace_ctx, args).await,
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

async fn execute_list(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: ListArgs,
) -> miette::Result<()> {
    let platforms = workspace_ctx.list_platforms().await;

    if args.json {
        let value = list_to_json(&platforms);
        let _ = writeln!(
            std::io::stdout(),
            "{}",
            serde_json::to_string_pretty(&value).into_diagnostic()?
        );
        return Ok(());
    }

    // Pull the rich PixiPlatform entries from the workspace so the human
    // listing can show subdir/virtual-package hints for non-trivial entries.
    let workspace = workspace_ctx.workspace();
    let workspace_platforms = workspace.workspace_manifest().workspace.platforms.clone();

    for (env_name, env_platforms) in platforms {
        let _ = writeln!(
            std::io::stdout(),
            "{} {}",
            console::style("Environment:").bold().bright(),
            env_name.fancy_display()
        )
        .inspect_err(|e| {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
        });

        for platform in env_platforms {
            let workspace_platform = workspace_platforms.iter().find(|p| p.name() == &platform);
            let hint = workspace_platform.and_then(rich_hint);
            let name_str = platform.as_str();
            let line = match hint {
                Some(hint) => format!("- {}  {}", name_str, console::style(hint).dim()),
                None => format!("- {name_str}"),
            };
            let _ = writeln!(std::io::stdout(), "{line}").inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
            });
        }
    }

    Ok(())
}

/// Build the inline parenthetical hint shown next to a rich platform in the
/// human `list` output. Returns `None` for plain subdir-platforms with no
/// declared virtual packages.
fn rich_hint(platform: &PixiPlatform) -> Option<String> {
    let custom_name = !platform.is_subdir_platform();
    let vp_count = platform.declared_virtual_packages().len();
    if !custom_name && vp_count == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if custom_name {
        parts.push(platform.subdir().to_string());
    }
    if vp_count > 0 {
        parts.push(format!(
            "{vp_count} virtual package{}",
            if vp_count == 1 { "" } else { "s" }
        ));
    }
    Some(format!("({})", parts.join(", ")))
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

async fn execute_show(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: ShowArgs,
) -> miette::Result<()> {
    match (args.name, args.all, args.current) {
        (Some(_), true, _) | (Some(_), _, true) => {
            miette::bail!("a positional NAME cannot be combined with --all or --current");
        }
        (None, false, false) => {
            miette::bail!(
                "missing platform name; pass a name, `--all`, or `--current` to use the auto-detected subdir"
            );
        }
        (Some(name), false, false) => execute_show_one(workspace_ctx, name, args.json).await,
        (None, all, current) => execute_show_multi(workspace_ctx, all, current, args.json).await,
    }
}

async fn execute_show_one(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    name: PixiPlatformName,
    json: bool,
) -> miette::Result<()> {
    let platform = workspace_ctx
        .get_workspace_platform(&name)
        .await
        .ok_or_else(|| {
            miette::miette!(
                "workspace does not define a platform named '{}'",
                name.as_str()
            )
        })?;

    let users = environments_and_features_using(workspace_ctx.workspace(), &platform);

    if json {
        let value = show_to_json(&platform, &users);
        let _ = writeln!(
            std::io::stdout(),
            "{}",
            serde_json::to_string_pretty(&value).into_diagnostic()?
        );
        return Ok(());
    }

    print_show_human(&platform, &users);
    Ok(())
}

/// Multi-platform variant of `show`. The two flags compose:
///   * `--all` alone: every platform, declaration order.
///   * `--current` alone: only platforms whose subdir matches
///     `Platform::current()`.
///   * `--all --current`: every platform, with current-subdir entries first
///     and the rest after a `---` separator.
async fn execute_show_multi(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    all: bool,
    current_flag: bool,
    json: bool,
) -> miette::Result<()> {
    let workspace = workspace_ctx.workspace();
    let workspace_platforms: Vec<PixiPlatform> = workspace
        .workspace_manifest()
        .workspace
        .platforms
        .iter()
        .cloned()
        .collect();

    if all && workspace_platforms.is_empty() {
        miette::bail!("workspace declares no platforms");
    }

    if json {
        let to_json_entry = |p: &PixiPlatform| {
            let users = environments_and_features_using(workspace, p);
            show_to_json(p, &users)
        };

        // `--current` contributes the synthetic auto-detected entry; `--all`
        // contributes every declared workspace platform. When both are set,
        // the auto-detected entry comes first.
        let mut platforms: Vec<serde_json::Value> = Vec::new();
        if current_flag {
            platforms.push(autodetected_to_json());
        }
        if all {
            platforms.extend(workspace_platforms.iter().map(to_json_entry));
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

    if current_flag {
        print_autodetected_host();
    }

    if all {
        if current_flag {
            let _ = writeln!(stdout, "\n{}", console::style("---").dim());
        }
        for (i, p) in workspace_platforms.iter().enumerate() {
            if i > 0 {
                let _ = writeln!(stdout);
            }
            let users = environments_and_features_using(workspace, p);
            print_show_human(p, &users);
        }
    }

    Ok(())
}

/// Pretty-print a synthetic "what the current host looks like" entry. Same
/// shape as a real platform's show block but with a distinct header so the
/// reader doesn't mistake it for a workspace declaration. No `Used by` lines
/// because nothing in the manifest points at this entry.
fn print_autodetected_host() {
    let host = PixiPlatform::auto_detected(Platform::current());
    let mut stdout = std::io::stdout();

    let _ = writeln!(
        stdout,
        "{} current",
        console::style("Platform:").bold().bright(),
    );
    let _ = writeln!(
        stdout,
        "  Subdir:   {}",
        styled_subdir_for_current_host(Platform::current())
    );

    let detected_str = match host.virtual_packages() {
        Ok(d) => {
            let specs: Vec<GenericVirtualPackage> = d.into_generic_virtual_packages().collect();
            if specs.is_empty() {
                "(none)".to_string()
            } else {
                specs
                    .iter()
                    .map(format_virtual_package_short)
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }
        Err(_) => "(none)".to_string(),
    };
    let _ = writeln!(stdout, "  Packages: {detected_str}");
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

/// Colour a subdir string based on whether it matches the current host's
/// subdir: green when this platform can actually run here, red otherwise.
fn styled_subdir_for_current_host(subdir: Platform) -> String {
    let raw = subdir.as_str();
    if subdir == Platform::current() {
        console::style(raw).green().to_string()
    } else {
        console::style(raw).red().to_string()
    }
}

fn print_show_human(platform: &PixiPlatform, users: &PlatformUsers) {
    let mut stdout = std::io::stdout();

    let _ = writeln!(
        stdout,
        "{} {}",
        console::style("Platform:").bold().bright(),
        platform.name().as_str(),
    );
    let _ = writeln!(
        stdout,
        "  Subdir:   {}",
        styled_subdir_for_current_host(platform.subdir())
    );

    let declared = platform.declared_virtual_packages();
    let declared_str = if declared.is_empty() {
        "(none)".to_string()
    } else {
        declared
            .iter()
            .map(format_virtual_package_short)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(stdout, "  Packages: {declared_str}");

    if !users.features.is_empty() {
        let _ = writeln!(stdout, "  Features: {}", users.features.join(", "));
    }
    if !users.environments.is_empty() {
        let _ = writeln!(stdout, "  Used by:  {}", users.environments.join(", "));
    }
}

/// Same compact form pixi_manifest writes to pixi.toml: bare name when version
/// and build are zero, `name=version` when only the build is zero, otherwise
/// the full conda spec.
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

fn list_to_json(
    platforms: &std::collections::HashMap<pixi_manifest::EnvironmentName, Vec<PixiPlatformName>>,
) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = platforms
        .iter()
        .map(|(env, names)| {
            (
                env.to_string(),
                serde_json::Value::Array(
                    names
                        .iter()
                        .map(|n| serde_json::Value::String(n.to_string()))
                        .collect(),
                ),
            )
        })
        .collect();
    serde_json::Value::Object(map)
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
