use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use miette::{Context, IntoDiagnostic};

/// Configure your shell to include the pixi global bin directory on PATH.
///
/// Detects your shell and appends the appropriate configuration to your shell's
/// config file. Use `--print` to just print the snippet without modifying any files.
#[derive(Parser, Debug)]
pub struct Args {
    /// The shell to generate the PATH init snippet for.
    /// If not specified, the shell is detected from the SHELL environment variable.
    #[arg(short, long)]
    shell: Option<ShellKind>,

    /// Print the shell snippet to stdout instead of modifying config files.
    #[arg(long)]
    print: bool,
}

#[derive(ValueEnum, Clone, Debug, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    Nushell,
    #[value(alias = "pwsh")]
    Powershell,
    Elvish,
}

impl std::fmt::Display for ShellKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellKind::Bash => write!(f, "bash"),
            ShellKind::Zsh => write!(f, "zsh"),
            ShellKind::Fish => write!(f, "fish"),
            ShellKind::Nushell => write!(f, "nushell"),
            ShellKind::Powershell => write!(f, "powershell"),
            ShellKind::Elvish => write!(f, "elvish"),
        }
    }
}

/// Detect the shell from the SHELL environment variable.
fn detect_shell() -> miette::Result<ShellKind> {
    let shell_env = std::env::var("SHELL").into_diagnostic().wrap_err(
        "Could not detect shell from $SHELL. Please specify a shell with --shell.",
    )?;
    let shell_name = Path::new(&shell_env)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    match shell_name {
        "bash" => Ok(ShellKind::Bash),
        "zsh" => Ok(ShellKind::Zsh),
        "fish" => Ok(ShellKind::Fish),
        "nu" | "nushell" => Ok(ShellKind::Nushell),
        "pwsh" | "powershell" => Ok(ShellKind::Powershell),
        "elvish" => Ok(ShellKind::Elvish),
        other => Err(miette::miette!(
            "Unsupported shell: `{other}`. Please specify one with --shell."
        )),
    }
}

/// Return the shell-appropriate snippet that adds `bin_dir` to PATH.
fn path_snippet(shell: ShellKind, bin_dir: &Path) -> String {
    let dir = bin_dir.display();
    match shell {
        ShellKind::Bash | ShellKind::Zsh => {
            format!("\n# pixi global bin directory\nexport PATH=\"{dir}:$PATH\"\n")
        }
        ShellKind::Fish => {
            format!("\n# pixi global bin directory\nfish_add_path {dir}\n")
        }
        ShellKind::Nushell => {
            format!(
                "\n# pixi global bin directory\n$env.PATH = ($env.PATH | prepend \"{dir}\")\n"
            )
        }
        ShellKind::Powershell => {
            format!("\n# pixi global bin directory\n$env:PATH = \"{dir}\" + [IO.Path]::PathSeparator + $env:PATH\n")
        }
        ShellKind::Elvish => {
            format!("\n# pixi global bin directory\nset paths = [{dir} $@paths]\n")
        }
    }
}

/// A marker we look for to detect whether the snippet is already present.
const MARKER: &str = "# pixi global bin directory";

/// Return the config file path(s) for the given shell.
fn config_files(shell: ShellKind) -> Vec<PathBuf> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    match shell {
        ShellKind::Bash => {
            // Prefer .bashrc, fall back to .bash_profile
            vec![home.join(".bashrc"), home.join(".bash_profile")]
        }
        ShellKind::Zsh => {
            // Respect ZDOTDIR if set
            let zdotdir = std::env::var("ZDOTDIR")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.clone());
            vec![zdotdir.join(".zshrc"), zdotdir.join(".zshenv")]
        }
        ShellKind::Fish => {
            let fish_dir = std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            vec![fish_dir.join("fish").join("config.fish")]
        }
        ShellKind::Nushell => {
            let nu_dir = std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            vec![nu_dir.join("nushell").join("env.nu")]
        }
        ShellKind::Powershell => {
            // PowerShell profile paths vary by platform
            #[cfg(windows)]
            {
                if let Some(docs) = dirs::document_dir() {
                    vec![
                        docs.join("PowerShell").join("Microsoft.PowerShell_profile.ps1"),
                        docs.join("WindowsPowerShell")
                            .join("Microsoft.PowerShell_profile.ps1"),
                    ]
                } else {
                    Vec::new()
                }
            }
            #[cfg(not(windows))]
            {
                let config = std::env::var("XDG_CONFIG_HOME")
                    .ok()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config"));
                vec![config.join("powershell").join("Microsoft.PowerShell_profile.ps1")]
            }
        }
        ShellKind::Elvish => {
            let config = std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            vec![config.join("elvish").join("rc.elv")]
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let shell = match args.shell {
        Some(s) => s,
        None => detect_shell()?,
    };

    let bin_dir = pixi_global::BinDir::from_env().await?;
    let snippet = path_snippet(shell, bin_dir.path());

    if args.print {
        print!("{snippet}");
        return Ok(());
    }

    // Find the first config file that either already exists or whose parent exists.
    let candidates = config_files(shell);
    let config_path = candidates
        .iter()
        .find(|p| p.exists())
        .or_else(|| {
            // None exist yet — pick the first whose parent directory exists.
            candidates
                .iter()
                .find(|p| p.parent().is_some_and(|d| d.is_dir()))
        })
        .ok_or_else(|| {
            miette::miette!(
                "Could not find a config file for {shell}. Tried: {}.\n\
                 Use `--print` to output the snippet to stdout instead.",
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

    // Check if the snippet is already present.
    let contents = if config_path.exists() {
        fs_err::read_to_string(config_path).into_diagnostic()?
    } else {
        String::new()
    };

    if contents.contains(MARKER) {
        eprintln!(
            "{}The pixi global bin directory is already configured in `{}`.",
            console::style(console::Emoji("✔ ", "")).green(),
            config_path.display(),
        );
        return Ok(());
    }

    // Append the snippet.
    fs_err::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(snippet.as_bytes())
        })
        .into_diagnostic()
        .wrap_err_with(|| format!("Failed to write to `{}`", config_path.display()))?;

    eprintln!(
        "{}Added pixi global bin directory to `{}`.\n  \
         Restart your shell or run `source {}` to apply.",
        console::style(console::Emoji("✔ ", "")).green(),
        config_path.display(),
        config_path.display(),
    );

    Ok(())
}
