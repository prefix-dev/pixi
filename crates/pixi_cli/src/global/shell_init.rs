use std::path::{Path, PathBuf};

use clap::Parser;
use miette::{Context, IntoDiagnostic};
use rattler_shell::shell::ShellEnum;

/// Configure your shell to include the pixi global bin directory on PATH.
///
/// Detects your shell and appends the appropriate configuration to your shell's
/// config file. Use `--print` to just print the snippet without modifying any files.
#[derive(Parser, Debug)]
pub struct Args {
    /// The shell to configure. Detected automatically if not specified,
    /// using the parent process, then $SHELL, then the system default.
    #[arg(short, long, value_parser = parse_shell)]
    shell: Option<ShellEnum>,

    /// Print the shell snippet to stdout instead of modifying config files.
    #[arg(long)]
    print: bool,

    /// Overwrite an existing pixi PATH configuration.
    /// Useful if the bin directory has changed.
    #[arg(long)]
    force: bool,
}

/// Parse a shell name string into a [`ShellEnum`].
fn parse_shell(s: &str) -> Result<ShellEnum, String> {
    match s.to_lowercase().as_str() {
        "bash" => Ok(ShellEnum::Bash(Default::default())),
        "zsh" => Ok(ShellEnum::Zsh(Default::default())),
        "fish" => Ok(ShellEnum::Fish(Default::default())),
        "nushell" | "nu" => Ok(ShellEnum::NuShell(Default::default())),
        "powershell" | "pwsh" => Ok(ShellEnum::PowerShell(Default::default())),
        "xonsh" => Ok(ShellEnum::Xonsh(Default::default())),
        "cmd" | "cmdexe" => Ok(ShellEnum::CmdExe(Default::default())),
        other => Err(format!(
            "unsupported shell `{other}`. Supported: bash, zsh, fish, nushell, powershell, xonsh, cmd"
        )),
    }
}

/// Detect the current shell using the same cascade as the rest of pixi:
/// parent process -> $SHELL env var -> system default.
fn detect_shell() -> miette::Result<ShellEnum> {
    ShellEnum::from_parent_process()
        .or_else(ShellEnum::from_env)
        .ok_or_else(|| {
            miette::miette!(
                "Could not detect your shell. Please specify one with `--shell`."
            )
        })
}

/// Human-readable name for a [`ShellEnum`] variant.
fn shell_name(shell: &ShellEnum) -> &'static str {
    match shell {
        ShellEnum::Bash(_) => "bash",
        ShellEnum::Zsh(_) => "zsh",
        ShellEnum::Fish(_) => "fish",
        ShellEnum::NuShell(_) => "nushell",
        ShellEnum::PowerShell(_) => "powershell",
        ShellEnum::Xonsh(_) => "xonsh",
        ShellEnum::CmdExe(_) => "cmd",
    }
}

/// Return the shell-appropriate snippet that adds `bin_dir` to PATH.
fn path_snippet(shell: &ShellEnum, bin_dir: &Path) -> miette::Result<String> {
    let dir = bin_dir.display();
    match shell {
        ShellEnum::Bash(_) | ShellEnum::Zsh(_) | ShellEnum::Xonsh(_) => {
            Ok(format!(
                "\n# pixi global bin directory\nexport PATH=\"{dir}:$PATH\"\n"
            ))
        }
        ShellEnum::Fish(_) => {
            // Use `set -gx` rather than `fish_add_path` which sets a universal
            // variable and persists across sessions on its own — having it in
            // config.fish would be redundant and subtly different.
            Ok(format!(
                "\n# pixi global bin directory\nset -gx PATH \"{dir}\" $PATH\n"
            ))
        }
        ShellEnum::NuShell(_) => {
            // Use `$env.PATH = (...)` syntax which works in nushell >= 0.80.
            // Using `split row` to handle the list conversion consistently.
            Ok(format!(
                "\n# pixi global bin directory\n\
                 $env.PATH = ($env.PATH | split row (char esep) | prepend \"{dir}\")\n"
            ))
        }
        ShellEnum::PowerShell(_) => {
            Ok(format!(
                "\n# pixi global bin directory\n\
                 $env:PATH = \"{dir}\" + [System.IO.Path]::PathSeparator + $env:PATH\n"
            ))
        }
        ShellEnum::CmdExe(_) => {
            Err(miette::miette!(
                "cmd.exe does not support persistent PATH modification via config files.\n\
                 To add the pixi bin directory to your PATH, run:\n\n  \
                 setx PATH \"{dir};%PATH%\""
            ))
        }
    }
}

/// A marker we look for to detect whether the snippet is already present.
const MARKER: &str = "# pixi global bin directory";

/// Return the config file path(s) for the given shell.
fn config_files(shell: &ShellEnum) -> Vec<PathBuf> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    match shell {
        ShellEnum::Bash(_) => {
            // Prefer .bashrc, fall back to .bash_profile
            vec![home.join(".bashrc"), home.join(".bash_profile")]
        }
        ShellEnum::Zsh(_) => {
            // Respect ZDOTDIR if set
            let zdotdir = std::env::var("ZDOTDIR")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.clone());
            vec![zdotdir.join(".zshrc"), zdotdir.join(".zshenv")]
        }
        ShellEnum::Fish(_) => {
            let config_dir = std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            vec![config_dir.join("fish").join("config.fish")]
        }
        ShellEnum::NuShell(_) => {
            let config_dir = std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            vec![config_dir.join("nushell").join("env.nu")]
        }
        ShellEnum::PowerShell(_) => {
            #[cfg(windows)]
            {
                if let Some(docs) = dirs::document_dir() {
                    vec![
                        docs.join("PowerShell")
                            .join("Microsoft.PowerShell_profile.ps1"),
                        docs.join("WindowsPowerShell")
                            .join("Microsoft.PowerShell_profile.ps1"),
                    ]
                } else {
                    Vec::new()
                }
            }
            #[cfg(not(windows))]
            {
                let config_dir = std::env::var("XDG_CONFIG_HOME")
                    .ok()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config"));
                vec![config_dir
                    .join("powershell")
                    .join("Microsoft.PowerShell_profile.ps1")]
            }
        }
        ShellEnum::Xonsh(_) => {
            vec![home.join(".xonshrc")]
        }
        ShellEnum::CmdExe(_) => Vec::new(),
    }
}

/// Return the shell-appropriate command to reload the config file.
fn reload_hint(shell: &ShellEnum, config_path: &Path) -> String {
    let path = config_path.display();
    match shell {
        ShellEnum::Bash(_) | ShellEnum::Zsh(_) | ShellEnum::Xonsh(_) | ShellEnum::Fish(_) => {
            format!("source {path}")
        }
        ShellEnum::NuShell(_) => {
            format!("source {path}")
        }
        ShellEnum::PowerShell(_) => {
            format!(". {path}")
        }
        ShellEnum::CmdExe(_) => "restart your shell".to_string(),
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let shell = match args.shell {
        Some(s) => s,
        None => detect_shell()?,
    };

    let bin_dir = pixi_global::BinDir::from_env().await?;
    let snippet = path_snippet(&shell, bin_dir.path())?;

    if args.print {
        print!("{snippet}");
        return Ok(());
    }

    // Find the first config file that either already exists or whose parent exists.
    let candidates = config_files(&shell);
    let config_path = candidates
        .iter()
        .find(|p| p.exists())
        .or_else(|| {
            candidates
                .iter()
                .find(|p| p.parent().is_some_and(|d| d.is_dir()))
        })
        .ok_or_else(|| {
            miette::miette!(
                "Could not find a config file for {shell_name}. Tried: {}.\n\
                 Use `--print` to output the snippet to stdout instead.",
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                shell_name = shell_name(&shell),
            )
        })?;

    // Check if the snippet is already present.
    let contents = if config_path.exists() {
        fs_err::read_to_string(config_path).into_diagnostic()?
    } else {
        String::new()
    };

    if contents.contains(MARKER) {
        if !args.force {
            eprintln!(
                "{}The pixi global bin directory is already configured in `{}`.\n  \
                 Use `--force` to overwrite the existing configuration.",
                console::style(console::Emoji("✔ ", "")).green(),
                config_path.display(),
            );
            return Ok(());
        }

        // Remove the old snippet (marker line + the line after it) and append fresh.
        let mut new_lines: Vec<&str> = Vec::new();
        let mut lines = contents.lines().peekable();
        while let Some(line) = lines.next() {
            if line.trim() == MARKER {
                // Skip the marker line and the next non-empty line (the PATH export).
                if let Some(next) = lines.peek() {
                    if !next.trim().is_empty() {
                        lines.next();
                    }
                }
            } else {
                new_lines.push(line);
            }
        }

        // Trim trailing blank lines, then append the new snippet.
        while new_lines.last().is_some_and(|l| l.is_empty()) {
            new_lines.pop();
        }
        let mut new_contents = new_lines.join("\n");
        new_contents.push_str(&snippet);

        fs_err::write(config_path, new_contents).into_diagnostic()?;
    } else {
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
    }

    let hint = reload_hint(&shell, config_path);
    eprintln!(
        "{}Added pixi global bin directory to `{}`.\n  \
         Restart your shell or run `{hint}` to apply.",
        console::style(console::Emoji("✔ ", "")).green(),
        config_path.display(),
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_path_snippet_bash() {
        let shell = ShellEnum::Bash(Default::default());
        let snippet = path_snippet(&shell, Path::new("/home/user/.pixi/bin")).unwrap();
        assert!(snippet.contains("export PATH=\"/home/user/.pixi/bin:$PATH\""));
        assert!(snippet.contains(MARKER));
    }

    #[test]
    fn test_path_snippet_zsh() {
        let shell = ShellEnum::Zsh(Default::default());
        let snippet = path_snippet(&shell, Path::new("/home/user/.pixi/bin")).unwrap();
        assert!(snippet.contains("export PATH=\"/home/user/.pixi/bin:$PATH\""));
    }

    #[test]
    fn test_path_snippet_fish() {
        let shell = ShellEnum::Fish(Default::default());
        let snippet = path_snippet(&shell, Path::new("/home/user/.pixi/bin")).unwrap();
        assert!(snippet.contains("set -gx PATH"));
        assert!(snippet.contains("/home/user/.pixi/bin"));
        assert!(!snippet.contains("fish_add_path"));
    }

    #[test]
    fn test_path_snippet_nushell() {
        let shell = ShellEnum::NuShell(Default::default());
        let snippet = path_snippet(&shell, Path::new("/home/user/.pixi/bin")).unwrap();
        assert!(snippet.contains("$env.PATH"));
        assert!(snippet.contains("split row (char esep)"));
        assert!(snippet.contains("prepend"));
    }

    #[test]
    fn test_path_snippet_powershell() {
        let shell = ShellEnum::PowerShell(Default::default());
        let snippet = path_snippet(&shell, Path::new("/home/user/.pixi/bin")).unwrap();
        assert!(snippet.contains("$env:PATH"));
        assert!(snippet.contains("[System.IO.Path]::PathSeparator"));
    }

    #[test]
    fn test_path_snippet_xonsh() {
        let shell = ShellEnum::Xonsh(Default::default());
        let snippet = path_snippet(&shell, Path::new("/home/user/.pixi/bin")).unwrap();
        assert!(snippet.contains("export PATH=\"/home/user/.pixi/bin:$PATH\""));
    }

    #[test]
    fn test_path_snippet_cmd_errors() {
        let shell = ShellEnum::CmdExe(Default::default());
        assert!(path_snippet(&shell, Path::new("/home/user/.pixi/bin")).is_err());
    }

    #[test]
    fn test_config_files_bash() {
        let shell = ShellEnum::Bash(Default::default());
        let files = config_files(&shell);
        assert!(!files.is_empty());
        assert!(files[0].ends_with(".bashrc"));
    }

    #[test]
    fn test_config_files_zsh() {
        let shell = ShellEnum::Zsh(Default::default());
        let files = config_files(&shell);
        assert!(!files.is_empty());
        assert!(files[0].ends_with(".zshrc"));
    }

    #[test]
    fn test_config_files_fish() {
        let shell = ShellEnum::Fish(Default::default());
        let files = config_files(&shell);
        assert!(!files.is_empty());
        assert!(files[0].ends_with("config.fish"));
    }

    #[test]
    fn test_config_files_nushell() {
        let shell = ShellEnum::NuShell(Default::default());
        let files = config_files(&shell);
        assert!(!files.is_empty());
        assert!(files[0].ends_with("env.nu"));
    }

    #[test]
    fn test_config_files_xonsh() {
        let shell = ShellEnum::Xonsh(Default::default());
        let files = config_files(&shell);
        assert!(!files.is_empty());
        assert!(files[0].ends_with(".xonshrc"));
    }

    #[test]
    fn test_reload_hint_bash() {
        let shell = ShellEnum::Bash(Default::default());
        let hint = reload_hint(&shell, Path::new("/home/user/.bashrc"));
        assert_eq!(hint, "source /home/user/.bashrc");
    }

    #[test]
    fn test_reload_hint_powershell() {
        let shell = ShellEnum::PowerShell(Default::default());
        let hint = reload_hint(&shell, Path::new("/home/user/.config/powershell/profile.ps1"));
        assert!(hint.starts_with(". "));
    }

    #[test]
    fn test_parse_shell() {
        assert!(matches!(parse_shell("bash"), Ok(ShellEnum::Bash(_))));
        assert!(matches!(parse_shell("zsh"), Ok(ShellEnum::Zsh(_))));
        assert!(matches!(parse_shell("fish"), Ok(ShellEnum::Fish(_))));
        assert!(matches!(parse_shell("nushell"), Ok(ShellEnum::NuShell(_))));
        assert!(matches!(parse_shell("nu"), Ok(ShellEnum::NuShell(_))));
        assert!(matches!(
            parse_shell("powershell"),
            Ok(ShellEnum::PowerShell(_))
        ));
        assert!(matches!(parse_shell("pwsh"), Ok(ShellEnum::PowerShell(_))));
        assert!(matches!(parse_shell("xonsh"), Ok(ShellEnum::Xonsh(_))));
        assert!(matches!(parse_shell("cmd"), Ok(ShellEnum::CmdExe(_))));
        assert!(parse_shell("unknown_shell").is_err());
    }

    #[test]
    fn test_all_snippets_contain_marker() {
        let bin = Path::new("/test/bin");
        let shells: Vec<ShellEnum> = vec![
            ShellEnum::Bash(Default::default()),
            ShellEnum::Zsh(Default::default()),
            ShellEnum::Fish(Default::default()),
            ShellEnum::NuShell(Default::default()),
            ShellEnum::PowerShell(Default::default()),
            ShellEnum::Xonsh(Default::default()),
        ];
        for shell in &shells {
            let snippet = path_snippet(shell, bin).unwrap();
            assert!(
                snippet.contains(MARKER),
                "Snippet for {} is missing marker",
                shell_name(shell)
            );
        }
    }

    #[test]
    fn test_execute_with_force_replaces_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".bashrc");

        // Write initial content with an old snippet
        fs_err::write(
            &config_path,
            "# existing config\n\
             # pixi global bin directory\n\
             export PATH=\"/old/path:$PATH\"\n",
        )
        .unwrap();

        let contents = fs_err::read_to_string(&config_path).unwrap();
        assert!(contents.contains("/old/path"));

        // Simulate what --force does: remove old marker + line, append new snippet
        let mut new_lines: Vec<&str> = Vec::new();
        let mut lines = contents.lines().peekable();
        while let Some(line) = lines.next() {
            if line.trim() == MARKER {
                if let Some(next) = lines.peek() {
                    if !next.trim().is_empty() {
                        lines.next();
                    }
                }
            } else {
                new_lines.push(line);
            }
        }
        while new_lines.last().is_some_and(|l| l.is_empty()) {
            new_lines.pop();
        }
        let mut new_contents = new_lines.join("\n");
        let snippet = path_snippet(
            &ShellEnum::Bash(Default::default()),
            Path::new("/new/path"),
        )
        .unwrap();
        new_contents.push_str(&snippet);

        fs_err::write(&config_path, &new_contents).unwrap();

        let result = fs_err::read_to_string(&config_path).unwrap();
        assert!(result.contains("/new/path"));
        assert!(!result.contains("/old/path"));
        assert!(result.contains("# existing config"));
    }
}
