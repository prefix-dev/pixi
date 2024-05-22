use crate::cli::Args as CommandArgs;
use clap::{CommandFactory, Parser};
use miette::IntoDiagnostic;
use regex::Regex;
use std::borrow::Cow;
use std::io::Write;

/// Generates a completion script for a shell.
#[derive(Parser, Debug)]
pub struct Args {
    /// The shell to generate a completion script for (defaults to 'bash').
    #[arg(short, long)]
    shell: Option<clap_complete::Shell>,
}

/// Generate completions for the pixi cli, and print those to the stdout
pub(crate) fn execute(args: Args) -> miette::Result<()> {
    let clap_shell = args
        .shell
        .or(clap_complete::Shell::from_env())
        .unwrap_or(clap_complete::Shell::Bash);

    // Generate the original completion script.
    let script = get_completion_script(clap_shell);

    // For supported shells, modify the script to include more context sensitive completions.
    let script = match clap_shell {
        clap_complete::Shell::Bash => replace_bash_completion(&script),
        clap_complete::Shell::Zsh => replace_zsh_completion(&script),
        _ => Cow::Owned(script),
    };

    // Write the result to the standard output
    std::io::stdout()
        .write_all(script.as_bytes())
        .into_diagnostic()?;

    Ok(())
}

/// Generate the completion script using clap_complete for a specified shell.
fn get_completion_script(shell: clap_complete::Shell) -> String {
    let mut buf = vec![];
    clap_complete::generate(shell, &mut CommandArgs::command(), "pixi", &mut buf);
    String::from_utf8(buf).expect("clap_complete did not generate a valid UTF8 script")
}

/// Replace the parts of the bash completion script that need different functionality.
fn replace_bash_completion(script: &str) -> Cow<str> {
    let pattern = r#"(?s)pixi__run\).*?opts="(.*?)".*?(if.*?fi)"#;
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    let replacement = r#"pixi__run)
            opts="$1"
            if [[ $${cur} == -* ]] ; then
               COMPREPLY=( $$(compgen -W "$${opts}" -- "$${cur}") )
               return 0
            elif [[ $${COMP_CWORD} -eq 2 ]]; then
               local tasks=$$(pixi task list --summary 2> /dev/null)
               if [[ $$? -eq 0 ]]; then
                   COMPREPLY=( $$(compgen -W "$${tasks}" -- "$${cur}") )
                   return 0
               fi
            fi"#;
    let re = Regex::new(pattern).unwrap();
    re.replace(script, replacement)
}

/// Replace the parts of the bash completion script that need different functionality.
fn replace_zsh_completion(script: &str) -> Cow<str> {
    let pattern = r"(?ms)(\(run\))(?:.*?)(_arguments.*?)(\*::task)";
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    let zsh_replacement = r#"$1
local tasks
tasks=("$${(@s/ /)$$(pixi task list --summary 2> /dev/null)}")

if [[ -n "$$tasks" ]]; then
    _values 'task' "$${tasks[@]}"
else
    return 1
fi
$2::task"#;

    let re = Regex::new(pattern).unwrap();
    re.replace(script, zsh_replacement)
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
    pub fn test_bash_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(clap_complete::Shell::Bash);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(replace_bash_completion(&script), script);
    }

    #[test]
    pub fn test_zsh_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(clap_complete::Shell::Zsh);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(replace_zsh_completion(&script), script);
    }
}
