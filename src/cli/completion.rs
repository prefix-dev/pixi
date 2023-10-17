use crate::cli::{Args, CompletionCommand};
use clap::CommandFactory;
use miette::IntoDiagnostic;
use regex::Regex;
use std::borrow::Cow;
use std::io::Write;
use std::str::from_utf8_mut;

pub(crate) fn execute(args: CompletionCommand) -> miette::Result<()> {
    let clap_shell = args
        .shell
        .or(clap_complete::Shell::from_env())
        .unwrap_or(clap_complete::Shell::Bash);

    let mut script = vec![];

    // Generate the original completion script.
    clap_complete::generate(
        clap_shell,
        &mut Args::command(),
        "pixi",
        &mut script, // &mut std::io::stdout(),
    );

    match clap_shell {
        clap_complete::Shell::Bash => {
            let script = replace_bash_completion(from_utf8_mut(&mut script).into_diagnostic()?);
            std::io::stdout()
                .write_all(script.as_ref().as_ref())
                .into_diagnostic()?;
        }
        clap_complete::Shell::Zsh => {
            let script = replace_zsh_completion(from_utf8_mut(&mut script).into_diagnostic()?);
            std::io::stdout()
                .write_all(script.as_ref().as_ref())
                .into_diagnostic()?;
        }
        _ => {
            // If no replacements needed write original script to stdout
            std::io::stdout().write_all(&script).into_diagnostic()?;
        }
    }

    Ok(())
}

fn replace_bash_completion(script: &str) -> Cow<str> {
    let pattern = r#"(?s)pixi__run\).*?opts="(.*?)".*?(if.*?fi)"#;

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

fn replace_zsh_completion(script: &str) -> Cow<str> {
    let pattern = r#"(?s)pixi__run\).*?opts="(.*?)".*?(if.*?fi)"#;

    let zsh_replacement = r#"pixi__run)
            opts="$1"
            if [[ $${CURRENT} -eq 2 ]]; then
                local tasks=$$(pixi task list --summary 2> /dev/null)
                if [[ $$? -eq 0 ]]; then
                    compadd "$${tasks}"
                    return 1
                fi
            elif [[ $${cur} == -* ]]; then
                compadd -- "$${opts}"
                return 1
            fi"#;

    let re = Regex::new(pattern).unwrap();
    re.replace(script, zsh_replacement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn test_completion() {
        let mut script = r#"
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
        let pattern = r#"(?s)pixi__run\).*?opts="(.*?)".*?(if.*?fi)"#;

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
        let script = re.replace(&mut script, replacement);
        insta::assert_snapshot!(script);
        println!("{}", script)
    }
}
