use crate::Args as CommandArgs;
use clap::{CommandFactory, Parser, ValueEnum};
use clap_complete::{Generator, shells};
use clap_complete_nushell::Nushell;
use miette::IntoDiagnostic;
use regex::{Captures, Regex};
use std::borrow::Cow;
use std::io::Write;

/// `pixi` subcommand emitting the space-delimited workspace environment names
/// used for `run` option-value completion in bash, zsh, and fish.
const ENVIRONMENT_LIST: &str = "workspace environment list --machine-readable";
/// `pixi` subcommand emitting the space-delimited workspace platform names
/// used for `run` option-value completion in bash, zsh, and fish.
const PLATFORM_LIST: &str = "workspace platform list --machine-readable";

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
pub fn execute(args: Args) -> miette::Result<()> {
    // Generate the original completion script.
    let script = get_completion_script(args.shell);

    // For supported shells, modify the script to include more context sensitive completions.
    let script = match args.shell {
        Shell::Bash => replace_bash_completion(&script, pixi_utils::executable_name()),
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
    let bin_name: &str = pixi_utils::executable_name();
    clap_complete::generate(shell, &mut CommandArgs::command(), bin_name, &mut buf);
    String::from_utf8(buf).expect("clap_complete did not generate a valid UTF8 script")
}

/// Replace the parts of the bash completion script that need different functionality.
fn replace_bash_completion<'a>(script: &'a str, bin_name: &str) -> Cow<'a, str> {
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    // Replace the '-' with '__' since that's what clap's generator does as well for Bash Shell completion.
    let clap_name = bin_name.replace("-", "__");
    // clap_complete >=4.6.2 separates each segment of the bin name from each
    // subcommand with an explicit `__subcmd__` marker, so the function for the
    // `run` subcommand of `pixi` is `pixi__subcmd__run`.
    let func_prefix = clap_name.replace("__", "__subcmd__");
    // Keep the generated `case "${prev}"` block (option-value completion) and
    // run task completion after it, so tasks complete at any position.
    let pattern = format!(
        r#"(?s){}__subcmd__run\).*?opts="(?P<opts>.*?)".*?if.*?fi\s*(?P<case>case.*?esac)"#,
        func_prefix
    );
    let re = Regex::new(pattern.as_str()).expect("should be able to compile the regex");
    re.replace(script, |caps: &Captures| {
        let opts = &caps["opts"];
        // Complete environment and platform names after their options instead
        // of files.
        let case = complete_bash_option(
            &caps["case"],
            bin_name,
            &BashOptionCompletion {
                labels: "--environment|-e",
                var: "environments",
                command: ENVIRONMENT_LIST,
            },
        );
        let case = complete_bash_option(
            &case,
            bin_name,
            &BashOptionCompletion {
                labels: "--platform|-p",
                var: "platforms",
                command: PLATFORM_LIST,
            },
        );
        format!(
            r#"{func_prefix}__subcmd__run)
            opts="{opts}"
            if [[ ${{cur}} == -* ]] ; then
                COMPREPLY=( $(compgen -W "${{opts}}" -- "${{cur}}") )
                return 0
            fi
            {case}
            local tasks
            if tasks=$({bin_name} task list --machine-readable 2> /dev/null) && [[ -n "${{tasks}}" ]]; then
                COMPREPLY=( $(compgen -W "${{tasks}}" -- "${{cur}}") )
                return 0
            fi"#
        )
    })
}

/// A `run` option whose value bash should complete from the space-delimited
/// output of a `pixi` subcommand instead of file paths.
struct BashOptionCompletion<'a> {
    /// Regex alternation matching the option's `case` labels, e.g.
    /// `--environment|-e`.
    labels: &'a str,
    /// Shell variable holding the candidate list.
    var: &'a str,
    /// `pixi` subcommand emitting the candidates.
    command: &'a str,
}

/// Complete an option's value in the bash `case "${prev}"` block from a `pixi`
/// subcommand's output, falling back to file completion when it yields nothing.
fn complete_bash_option(case_block: &str, bin_name: &str, opt: &BashOptionCompletion) -> String {
    let pattern = format!(
        r#"(?m)^(?P<indent>\s*)(?P<label>{})\)\n\s*COMPREPLY=\(\$\(compgen -f "\$\{{cur\}}"\)\)"#,
        opt.labels
    );
    let re = Regex::new(&pattern).expect("should be able to compile the regex");
    let (var, command) = (opt.var, opt.command);
    re.replace_all(case_block, |caps: &Captures| {
        let indent = &caps["indent"];
        let label = &caps["label"];
        format!(
            r#"{indent}{label})
{indent}    local {var}
{indent}    if {var}=$({bin_name} {command} 2> /dev/null) && [[ -n "${{{var}}}" ]]; then
{indent}        COMPREPLY=( $(compgen -W "${{{var}}}" -- "${{cur}}") )
{indent}    else
{indent}        COMPREPLY=($(compgen -f "${{cur}}"))
{indent}    fi"#
        )
    })
    .into_owned()
}

/// Replace the parts of the zsh completion script that need different functionality.
fn replace_zsh_completion(script: &str) -> Cow<'_, str> {
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    let pattern = r"(?ms)(?P<run>\(run\))(?:.*?)(?P<args>_arguments.*?)(\*::task)";
    let bin_name: &str = pixi_utils::executable_name();
    let re = Regex::new(pattern).expect("should be able to compile the regex");
    re.replace(script, |caps: &Captures| {
        let run = &caps["run"];
        // Complete environment and platform names after their options instead
        // of files.
        let args = caps["args"]
            .replace(
                ":ENVIRONMENT:_default",
                &format!(":ENVIRONMENT:($({bin_name} {ENVIRONMENT_LIST} 2> /dev/null))"),
            )
            .replace(
                ":PLATFORM:_default",
                &format!(":PLATFORM:($({bin_name} {PLATFORM_LIST} 2> /dev/null))"),
            );
        format!(
            r#"{run}
local tasks
tasks=("${{(@s/ /)$({bin_name} task list --machine-readable 2> /dev/null)}}")

if [[ -n "$tasks" ]]; then
    _values 'task' "${{tasks[@]}}"
else
    return 1
fi
{args}::task"#
        )
    })
}

fn replace_fish_completion(script: &str) -> Cow<'_, str> {
    // Adds tab completion to the pixi run command.
    let bin_name = pixi_utils::executable_name();

    // Complete environment and platform names for the corresponding options of
    // `run` (and its `r` alias), which clap otherwise completes as file paths.
    let script =
        fish_complete_run_option(script, bin_name, "-s e -l environment", ENVIRONMENT_LIST);
    let script = fish_complete_run_option(&script, bin_name, "-s p -l platform", PLATFORM_LIST);

    let addition = format!(
        "complete -c {bin_name} -n \"__fish_seen_subcommand_from run\" -f -a \"(string split ' ' ({bin_name} task list --machine-readable  2> /dev/null))\""
    );
    let new_script = format!("{script}{addition}\n");
    let pattern = r#"-n "__fish_seen_subcommand_from run""#;
    let replacement = r#"-n "__fish_seen_subcommand_from run; or __fish_seen_subcommand_from r""#;
    let re = Regex::new(pattern).expect("should be able to compile the regex");
    let result = re.replace_all(&new_script, replacement);
    Cow::Owned(result.into_owned())
}

/// Complete a `run` option's value from a `pixi` subcommand's output by
/// appending `-f -a "..."` to the clap-generated `complete` line matched by
/// `flags` (e.g. `-s e -l environment`) for both `run` and its `r` alias.
fn fish_complete_run_option(script: &str, bin_name: &str, flags: &str, command: &str) -> String {
    let pattern = format!(
        r#"(?m)^(complete -c {bin_name} -n "__fish_pixi_using_subcommand r(?:un)?" {flags} [^\n]*? -r)$"#
    );
    let re = Regex::new(&pattern).expect("should be able to compile the regex");
    re.replace_all(
        script,
        format!(r#"$1 -f -a "(string split ' ' ({bin_name} {command} 2> /dev/null))""#).as_str(),
    )
    .into_owned()
}

/// Replace the parts of the nushell completion script that need different functionality.
fn replace_nushell_completion(script: &str) -> Cow<'_, str> {
    fn insert_after_module<'a>(input: &'a str, insert: &str) -> Cow<'a, str> {
        // Match the literal line
        let re = Regex::new(r"(?m)^module completions \{").expect("static regex must be valid");

        re.replace(input, |caps: &regex::Captures| {
            format!("{}{}\n", &caps[0], insert)
        })
    }

    /// For every occurrence of `flag` inside any `export extern "<cmd>" [...]` block:
    /// - Extract the command name.
    /// - Extract the type token and the rest of the line.
    /// - Call `modify(cmd, ty, tail)`
    ///     - If it returns `Some(extra)`, append `extra` after the type.
    ///     - If it returns `None`, leave the line unchanged.
    ///
    /// The closure gets clean values:
    ///     cmd  = "pixi run"
    ///     ty   = "string"  (or "path", etc.)
    ///     tail = " # comment"
    pub fn append_after_type<F>(input: &str, flag: &str, modify: F) -> String
    where
        F: Fn(&str, &str, &str) -> Option<String>,
    {
        let flag_escaped = regex::escape(flag);

        // Captures:
        //   cmd  = the command name in quotes
        //   pre  = prefix up to and including ": " on the flag line
        //   ty   = type token
        //   tail = rest of the line (comment + spacing)
        let pattern = format!(
            r#"(?ms)(export extern "(?P<cmd>[^"]+)" \[[^\]]*?^\s*{flag_escaped}\s*:\s*)(?P<ty>[^\s#@]+)(?P<tail>[^\n]*)"#,
        );

        let re = Regex::new(&pattern).expect("static regex must be valid");

        re.replace_all(input, |caps: &Captures| {
            let cmd = &caps["cmd"];
            let pre = &caps[1];
            let ty = &caps["ty"];
            let tail = &caps["tail"];

            match modify(cmd, ty, tail) {
                Some(extra) => format!("{pre}{ty}{extra}{tail}"),
                None => caps[0].to_string(),
            }
        })
        .into_owned()
    }

    /// Append `append` after the `string` type for the argument `...task: string`
    /// but only inside the `export extern "<command>" [...]` block.
    pub fn append_to_task_arg(input: &str, command: &str, arg: &str, append: &str) -> String {
        let cmd_escaped = regex::escape(command);

        // Matches:
        //   export extern "<command>" [ ... <newline>
        //   ...task: string[rest]
        //
        // Captures:
        //   1 = prefix up to and including "string"
        //   2 = rest of line (comments, whitespace)
        let pattern = format!(
            r#"(?ms)(export extern "{cmd_escaped}" \[[^\]]*?^\s*\.{{3}}{arg}:\s*string)([^\n]*)"#,
        );

        let re = Regex::new(&pattern).expect("static regex must be valid");

        re.replace_all(input, |caps: &Captures| {
            let prefix = &caps[1]; // "...task: string"
            let tail = &caps[2]; // comment / rest of line
            format!("{prefix}{append}{tail}")
        })
        .into_owned()
    }

    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    let bin_name = pixi_utils::executable_name();

    // Extra definitions we want to add to the completion module.
    let script = insert_after_module(
        script,
        &format!(
            r#"
    def "nu-complete {bin_name} run environment" [] {{
      ^{bin_name} info --json | from json | get environments_info | get name
    }}

    def "nu-complete {bin_name} run platform" [] {{
      ^{bin_name} info --json | from json | get environments_info | get platforms | flatten | get name | uniq
    }}

    def "nu-complete {bin_name} run" [] {{
      ^{bin_name} info --json | from json | get environments_info | get tasks | flatten | uniq
    }}

    export alias "{bin_name} r" = {bin_name} run
    "#
        ),
    );

    // Add completion for all `--environment(-e)` flags.
    let script = append_after_type(&script, "--environment(-e)", |cmd, _ty, _extra| {
        let cmd = cmd.strip_prefix(bin_name)?.trim_start();
        if cmd.starts_with("global") {
            // --environment means something else for pixi global
            None
        } else {
            Some(format!(r#"@"nu-complete {bin_name} run environment""#))
        }
    });

    // Add completion for `pixi run`'s `--platform(-p)` flag.
    let script = append_after_type(&script, "--platform(-p)", |cmd, _ty, _extra| {
        let cmd = cmd.strip_prefix(bin_name)?.trim_start();
        (cmd == "run").then(|| format!(r#"@"nu-complete {bin_name} run platform""#))
    });

    // Add completion for the `...task: string` argument in pixi run.
    let script = append_to_task_arg(
        &script,
        &format!("{bin_name} run"),
        "task",
        &format!("@\"nu-complete {bin_name} run\""),
    );

    script.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub(crate) fn test_zsh_completion() {
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
'-e+[The environment to run the task in]:ENVIRONMENT:_default' \
'--environment=[The environment to run the task in]:ENVIRONMENT:_default' \
'-p+[Install and run in the environment for the given platform]:PLATFORM:_default' \
'--platform=[Install and run in the environment for the given platform]:PLATFORM:_default' \
'--color=[Whether the log needs to be colored]:COLOR:(always never auto)' \
'(--frozen)--locked[Require pixi.lock is up-to-date]' \
'(--locked)--frozen[Don'\''t check if pixi.lock is up-to-date, install as lock file states]' \
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
        // Normalize the test binary name (e.g. `pixi_cli-<hash>`) to `pixi` so
        // the snapshot is stable across builds.
        insta::with_settings!({filters => vec![
            (pixi_utils::executable_name(), "pixi"),
        ]}, {
            insta::assert_snapshot!(result);
        });
    }

    #[test]
    pub(crate) fn test_bash_completion() {
        // Trimmed excerpt of what clap_complete >=4.6.2 generates for `pixi`.
        let script = r#"
        pixi__subcmd__project__subcmd__help__subcmd__help)
            opts=""
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pixi__subcmd__run)
            opts="-e -p -v -q -h --manifest-path --locked --frozen --environment --platform --verbose --quiet --color --help [TASK]..."
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --manifest-path)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --environment)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -e)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --platform)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -p)
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
        pixi__subcmd__search)
            opts="-c -l -v -q -h --channel --color --help <PACKAGE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --channel)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        "#;
        let result = replace_bash_completion(script, "pixi");
        assert_ne!(result, script);
        insta::assert_snapshot!(result);
    }

    #[test]
    pub(crate) fn test_nushell_completion() {
        // NOTE THIS IS FORMATTED BY HAND!
        let script = r#"
  # Runs task in project
  export extern "pixi run" [
    ...task: string@"nu-complete pixi run"           # The pixi task or a task shell command you want to run in the project's environment, which can be an executable in the environment's PATH
    --manifest-path: string   # The path to `pixi.toml`, `pyproject.toml`, or the project directory
    --no-lockfile-update      # Legacy flag, do not use, will be removed in subsequent version
    --frozen                  # Install the environment as defined in the lock file, doesn't update lock file if it isn't up-to-date with the manifest file
    --locked                  # Check if lock file is up-to-date before installing the environment, aborts when lock file isn't up-to-date with the manifest file
    --no-install              # Don't modify the environment, only modify the lock file
    --tls-no-verify           # Do not verify the TLS certificate of the server
    --auth-file: string       # Path to the file containing the authentication token
    --pypi-keyring-provider: string@"nu-complete pixi run pypi_keyring_provider" # Specifies if we want to use uv keyring provider
    --concurrent-solves: string # Max concurrent solves, default is the number of CPUs
    --concurrent-downloads: string # Max concurrent network requests, default is 50
    --force-activate          # Do not use the environment activation cache. (default: true except in experimental mode)
    --environment(-e): string@"nu-complete pixi run environment" # The environment to run the task in
    --clean-env               # Use a clean environment to run the task
    --skip-deps               # Don't run the dependencies of the task ('depends-on' field in the task definition)
    --verbose(-v)             # Increase logging verbosity
    --quiet(-q)               # Decrease logging verbosity
    --color: string@"nu-complete pixi run color" # Whether the log needs to be colored
    --no-progress             # Hide all progress bars, always turned on if stderr is not a terminal
    --help(-h)                # Print help (see more with '--help')
  ]"#;
        let result = replace_nushell_completion(script);
        let replacement = format!("{} run", pixi_utils::executable_name());
        let nu_complete_run = format!("nu-complete {} run", pixi_utils::executable_name());
        println!("{result}");
        insta::with_settings!({filters => vec![
            (replacement.as_str(), "[PIXI RUN]"),
            (nu_complete_run.as_str(), "[nu_complete_run PIXI COMMAND]"),
        ]}, {
            insta::assert_snapshot!(result);
        });
    }

    #[test]
    pub(crate) fn test_bash_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(Shell::Bash);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(
            replace_bash_completion(&script, pixi_utils::executable_name()),
            script
        );
    }

    #[test]
    pub(crate) fn test_zsh_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(Shell::Zsh);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(replace_zsh_completion(&script), script);
    }

    #[test]
    pub(crate) fn test_fish_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(Shell::Fish);
        let replaced_script = replace_fish_completion(&script);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(replaced_script, script);
    }

    #[test]
    pub(crate) fn test_nushell_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(Shell::Nushell);
        // Test if there was a replacement done on the clap generated completions
        if replace_nushell_completion(&script) == script {
            panic!(
                "Completion replacement did not work as expected\n\n======================\n= Original script\n======================\n{script}"
            );
        }
    }
}
