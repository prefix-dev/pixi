use crate::cli::Args as CommandArgs;
use clap::{CommandFactory, Parser, ValueEnum};
use clap_complete::{shells, Generator};
use clap_complete_nushell::Nushell;
use miette::IntoDiagnostic;
use regex::Regex;
use std::borrow::Cow;
use std::io::Write;

/// Generates a completion script for a shell.
#[derive(Parser, Debug)]
pub struct Args {
    /// The shell to generate a completion script for
    #[arg(short, long)]
    shell: Shell,
}

/// Defines the shells for which we can provide completions
#[allow(clippy::enum_variant_names)]
#[derive(ValueEnum, Clone, Debug, Copy, Eq, Hash, PartialEq)]
enum Shell {
    /// Bourne Again SHell (bash)
    Bash,
    /// Elvish shell
    Elvish,
    /// Friendly Interactive SHell (fish)
    Fish,
    /// Nushell
    Nushell,
    /// PowerShell
    Powershell,
    /// Z SHell (zsh)
    Zsh,
}

impl Generator for Shell {
    fn file_name(&self, name: &str) -> String {
        match self {
            Shell::Bash => shells::Bash.file_name(name),
            Shell::Elvish => shells::Elvish.file_name(name),
            Shell::Fish => shells::Fish.file_name(name),
            Shell::Nushell => Nushell.file_name(name),
            Shell::Powershell => shells::PowerShell.file_name(name),
            Shell::Zsh => shells::Zsh.file_name(name),
        }
    }

    fn generate(&self, cmd: &clap::Command, buf: &mut dyn std::io::Write) {
        match self {
            Shell::Bash => shells::Bash.generate(cmd, buf),
            Shell::Elvish => shells::Elvish.generate(cmd, buf),
            Shell::Fish => shells::Fish.generate(cmd, buf),
            Shell::Nushell => Nushell.generate(cmd, buf),
            Shell::Powershell => shells::PowerShell.generate(cmd, buf),
            Shell::Zsh => shells::Zsh.generate(cmd, buf),
        }
    }
}

/// Generate completions for the pixi cli, and print those to the stdout
pub(crate) fn execute(args: Args) -> miette::Result<()> {
    // Generate the original completion script.
    let script = get_completion_script(args.shell);

    // For supported shells, modify the script to include more context sensitive completions.
    let script = match args.shell {
        Shell::Bash => replace_bash_completion(&script),
        Shell::Zsh => replace_zsh_completion(&script),
        Shell::Fish => replace_fish_completion(&script),
        Shell::Nushell => replace_nushell_completion(&script),
        _ => Cow::Owned(script),
    };

    // Write the result to the standard output
    std::io::stdout()
        .write_all(script.as_bytes())
        .into_diagnostic()?;

    Ok(())
}

/// Generate the completion script using clap_complete for a specified shell.
fn get_completion_script(shell: Shell) -> String {
    let mut buf = vec![];
    clap_complete::generate(shell, &mut CommandArgs::command(), "pixi", &mut buf);
    String::from_utf8(buf).expect("clap_complete did not generate a valid UTF8 script")
}

/// Replace the parts of the bash completion script that need different functionality.
fn replace_bash_completion(script: &str) -> Cow<str> {
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    let pattern = r#"(?s)pixi__run\).*?opts="(.*?)".*?(if.*?fi)"#;
    let replacement = r#"pixi__run)
            opts="$1"
            if [[ $${cur} == -* ]] ; then
               COMPREPLY=( $$(compgen -W "$${opts}" -- "$${cur}") )
               return 0
            elif [[ $${COMP_CWORD} -eq 2 ]]; then
               local tasks=$$(pixi task list --machine-readable 2> /dev/null)
               if [[ $$? -eq 0 ]]; then
                   COMPREPLY=( $$(compgen -W "$${tasks}" -- "$${cur}") )
                   return 0
               fi
            fi"#;
    let re = Regex::new(pattern).unwrap();
    re.replace(script, replacement)
}

/// Replace the parts of the zsh completion script that need different functionality.
fn replace_zsh_completion(script: &str) -> Cow<str> {
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    let pattern = r"(?ms)(\(run\))(?:.*?)(_arguments.*?)(\*::task)";
    let replacement = r#"$1
local tasks
tasks=("$${(@s/ /)$$(pixi task list --machine-readable 2> /dev/null)}")

if [[ -n "$$tasks" ]]; then
    _values 'task' "$${tasks[@]}"
else
    return 1
fi
$2::task"#;

    let re = Regex::new(pattern).unwrap();
    re.replace(script, replacement)
}

fn replace_fish_completion(script: &str) -> Cow<str> {
    // Adds tab completion to the pixi run command.
    let addition = "complete -c pixi -f -n \"__fish_seen_subcommand_from run\" -a \"(string split ' ' (pixi task list --machine-readable  2> /dev/null))\"";
    format!("{}{}\n", script, addition).into()
}

/// Replace the parts of the nushell completion script that need different functionality.
fn replace_nushell_completion(script: &str) -> Cow<str> {
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    let pattern = r#"(#.*\n  export extern "pixi run".*\n.*...task: string)([^\]]*--environment\(-e\): string)"#;
    let replacement = r#"
  def "nu-complete pixi run" [] {
    ^pixi info --json | from json | get environments_info | get tasks | flatten | uniq
  }

  def "nu-complete pixi run environment" [] {
    ^pixi info --json | from json | get environments_info | get name
  }

  ${1}@"nu-complete pixi run"${2}@"nu-complete pixi run environment""#;

    let re = Regex::new(pattern).unwrap();
    re.replace(script, replacement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn test_zsh_completion() {
        let script = r#"
(add)
_arguments "${_arguments_options[@]}" \
'--manifest-path=[The path to '\''pixi.toml'\'']:MANIFEST_PATH:_files' \
'*::specs -- Specify the dependencies you wish to add to the project:' \
&& ret=0
;;
(run)
_arguments "${_arguments_options[@]}" \
'--manifest-path=[The path to '\''pixi.toml'\'']:MANIFEST_PATH:_files' \
'--color=[Whether the log needs to be colored]:COLOR:(always never auto)' \
'(--frozen)--locked[Require pixi.lock is up-to-date]' \
'(--locked)--frozen[Don'\''t check if pixi.lock is up-to-date, install as lockfile states]' \
'*-v[More output per occurrence]' \
'*--verbose[More output per occurrence]' \
'(-v --verbose)*-q[Less output per occurrence]' \
'(-v --verbose)*--quiet[Less output per occurrence]' \
'-h[Print help]' \
'--help[Print help]' \
'*::task -- The pixi task or a deno task shell command you want to run in the project's environment, which can be an executable in the environment's PATH.:' \
&& ret=0
;;
(add)
_arguments "${_arguments_options[@]}" \
&& ret=0
;;
(run)
_arguments "${_arguments_options[@]}" \
&& ret=0
;;
(shell)
_arguments "${_arguments_options[@]}" \
&& ret=0
;;

        "#;
        let result = replace_zsh_completion(script);
        insta::assert_snapshot!(result);
    }

    #[test]
    pub fn test_bash_completion() {
        // NOTE THIS IS FORMATTED BY HAND!
        let script = r#"
        pixi__project__help__help)
            opts=""
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pixi__run)
            opts="-v -q -h --manifest-path --locked --frozen --verbose --quiet --color --help [TASK]..."
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --manifest-path)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --color)
                    COMPREPLY=($(compgen -W "always never auto" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pixi__search)
            opts="-c -l -v -q -h --channel --color --help <PACKAGE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
            fi
            case "${prev}" in
                --channel)
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )

            ;;
        "#;
        let result = replace_bash_completion(script);
        insta::assert_snapshot!(result);
    }

    #[test]
    pub fn test_nushell_completion() {
        // NOTE THIS IS FORMATTED BY HAND!
        let script = r#"
  # Runs task in project
  export extern "pixi run" [
    ...task: string           # The pixi task or a task shell command you want to run in the project's environment, which can be an executable in the environment's PATH
    --manifest-path: string   # The path to 'pixi.toml' or 'pyproject.toml'
    --frozen                  # Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
    --locked                  # Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
    --environment(-e): string # The environment to run the task in
    --tls-no-verify           # Do not verify the TLS certificate of the server
    --auth-file: string       # Path to the file containing the authentication token
    --pypi-keyring-provider: string@"nu-complete pixi run pypi_keyring_provider" # Specifies if we want to use uv keyring provider
    --clean-env               # Use a clean environment to run the task
    --verbose(-v)             # Increase logging verbosity
    --quiet(-q)               # Decrease logging verbosity
    --color: string@"nu-complete pixi run color" # Whether the log needs to be colored
    --no-progress             # Hide all progress bars
    --help(-h)                # Print help (see more with '--help')
  ]"#;
        let result = replace_nushell_completion(script);
        insta::assert_snapshot!(result);
    }

    #[test]
    pub fn test_bash_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(Shell::Bash);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(replace_bash_completion(&script), script);
    }

    #[test]
    pub fn test_zsh_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(Shell::Zsh);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(replace_zsh_completion(&script), script);
    }

    #[test]
    pub fn test_fish_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(Shell::Fish);
        let replaced_script = replace_fish_completion(&script);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(replaced_script, script);
        assert!(replaced_script.contains(&script));
    }

    #[test]
    pub fn test_nushell_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(Shell::Nushell);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(replace_nushell_completion(&script), script);
    }
}
