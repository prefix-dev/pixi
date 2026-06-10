use crate::Args as CommandArgs;
use clap::{CommandFactory, Parser, ValueEnum};
use clap_complete::{Generator, shells};
use clap_complete_nushell::Nushell;
use miette::IntoDiagnostic;
use regex::{Captures, Regex};
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
pub fn execute(args: Args) -> miette::Result<()> {
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
    let bin_name: &str = pixi_utils::executable_name();
    clap_complete::generate(shell, &mut CommandArgs::command(), bin_name, &mut buf);
    String::from_utf8(buf).expect("clap_complete did not generate a valid UTF8 script")
}

/// Replace the parts of the bash completion script that need different functionality.
fn replace_bash_completion(script: &str) -> Cow<'_, str> {
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    // Replace the '-' with '__' since that's what clap's generator does as well for Bash Shell completion.
    let bin_name: &str = pixi_utils::executable_name();
    let clap_name = bin_name.replace("-", "__");
    // clap_complete >=4.6.2 separates each segment of the bin name from each
    // subcommand with an explicit `__subcmd__` marker, so the function for the
    // `run` subcommand of `pixi` is `pixi__subcmd__run`.
    let func_prefix = clap_name.replace("__", "__subcmd__");
    let pattern = format!(
        r#"(?s){}__subcmd__run\).*?opts="(.*?)".*?(if.*?fi)"#,
        func_prefix
    );
    let replacement = r#"FUNC_PREFIX__subcmd__run)
            opts="$1"
            if [[ $${cur} == -* ]] ; then
               COMPREPLY=( $$(compgen -W "$${opts}" -- "$${cur}") )
               return 0
            elif [[ $${COMP_CWORD} -eq 2 ]]; then
               local tasks=$$(BIN_NAME task list --machine-readable 2> /dev/null)
               if [[ $$? -eq 0 ]]; then
                   COMPREPLY=( $$(compgen -W "$${tasks}" -- "$${cur}") )
                   return 0
               fi
            fi"#;
    let re = Regex::new(pattern.as_str()).expect("should be able to compile the regex");
    re.replace(
        script,
        replacement
            .replace("BIN_NAME", bin_name)
            .replace("FUNC_PREFIX", &func_prefix),
    )
}

/// Replace the parts of the zsh completion script that need different functionality.
fn replace_zsh_completion(script: &str) -> Cow<'_, str> {
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND
    let pattern = r"(?ms)(\(run\))(?:.*?)(_arguments.*?)(\*::task)";
    let bin_name: &str = pixi_utils::executable_name();
    let replacement = r#"$1
local tasks
tasks=("$${(@s/ /)$$(BIN_NAME task list --machine-readable 2> /dev/null)}")

if [[ -n "$$tasks" ]]; then
    _values 'task' "$${tasks[@]}"
else
    return 1
fi
$2::task"#;

    let re = Regex::new(pattern).expect("should be able to compile the regex");
    re.replace(script, replacement.replace("BIN_NAME", bin_name))
}

fn replace_fish_completion(script: &str) -> Cow<'_, str> {
    // Adds tab completion to the pixi run command.
    let bin_name = pixi_utils::executable_name();
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

    /// Replace the `export extern "<command>" [...]` block with a
    /// `def --wrapped` wrapper that forwards all arguments verbatim to the
    /// external command.
    ///
    /// Nushell parses calls to known externs against their declared
    /// signature: it intercepts `--help`/`-h` (showing its own help page
    /// instead of running the command) and canonicalizes declared short
    /// flags to their long form (e.g. `-v` becomes `--verbose`). For
    /// commands like `pixi run` and `pixi exec` that forward trailing
    /// arguments to another command, this corrupts the forwarded arguments.
    /// A `def --wrapped` passes every token through untouched.
    pub fn replace_extern_with_wrapped_def(
        input: &str,
        command: &str,
        arg_name: &str,
        completer: Option<&str>,
    ) -> String {
        let cmd_escaped = regex::escape(command);

        // Matches the whole block:
        //   export extern "<command>" [
        //     ...
        //   ]
        let pattern =
            format!(r#"(?ms)^(?P<indent>[ \t]*)export extern "{cmd_escaped}" \[[^\]]*\]"#);

        let re = Regex::new(&pattern).expect("static regex must be valid");

        re.replace_all(input, |caps: &Captures| {
            let indent = &caps["indent"];
            let completion = completer
                .map(|completer| format!("@\"{completer}\""))
                .unwrap_or_default();
            format!(
                "{indent}export def --wrapped \"{command}\" [...{arg_name}: string{completion}] {{\n\
                 {indent}  ^{command} ...${arg_name}\n\
                 {indent}}}"
            )
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

    // Replace the externs for commands that forward their trailing arguments
    // to another command with `def --wrapped` wrappers, so that nushell
    // passes flags like `--help` through verbatim instead of consuming them
    // (see https://github.com/prefix-dev/pixi/issues/4430).
    let script = replace_extern_with_wrapped_def(
        &script,
        &format!("{bin_name} run"),
        "task",
        Some(&format!("nu-complete {bin_name} run")),
    );
    let script =
        replace_extern_with_wrapped_def(&script, &format!("{bin_name} exec"), "args", None);

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
        let replacement = format!("{} task list", pixi_utils::executable_name());
        insta::with_settings!({filters => vec![
            (replacement.as_str(), "pixi task list"),
        ]}, {
            insta::assert_snapshot!(result);
        });
    }

    #[test]
    pub(crate) fn test_bash_completion() {
        // NOTE THIS IS FORMATTED BY HAND!
        let script = r#"
        pixi__project__help__help)
            opts=""
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pixi__run)
            opts="-v -q -h --manifest-path --locked --frozen --verbose --quiet --color --help [TASK]..."
            if [[ ${cur} == -* ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            elif [[ ${COMP_CWORD} -eq 2 ]]; then
               local tasks=$(pixi task list --machine-readable 2> /dev/null)
               if [[ $? -eq 0 ]]; then
                   COMPREPLY=( $(compgen -W "${tasks}" -- "${cur}") )
                   return 0
               fi
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
        let replacement = format!("{} task list", pixi_utils::executable_name());
        let zsh_arg_name = format!("{}__", pixi_utils::executable_name().replace("-", "__"));
        println!("{result}");
        insta::with_settings!({filters => vec![
            (replacement.as_str(), "pixi task list"),
            (zsh_arg_name.as_str(), "[PIXI COMMAND]"),
        ]}, {
            insta::assert_snapshot!(result);
        });
    }

    #[test]
    pub(crate) fn test_nushell_completion() {
        // NOTE THIS IS FORMATTED BY HAND!
        // In tests the binary name is the name of the test executable, so the
        // input script is templated with the actual binary name to make the
        // replacements match, and the snapshot is filtered back to `pixi`.
        let script = r#"module completions {
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
  ]

  # Run a command and install it in a temporary environment
  export extern "pixi exec" [
    ...command: string        # The executable to run, followed by any arguments
    --spec(-s): string        # Matchspecs of package to install. If this is not provided, the package is guessed from the command
    --help(-h)                # Print help (see more with '--help')
  ]
}
"#
        .replace("pixi ", &format!("{} ", pixi_utils::executable_name()));
        let result = replace_nushell_completion(&script);
        let bin_name = regex::escape(pixi_utils::executable_name());
        println!("{result}");
        insta::with_settings!({filters => vec![
            (bin_name.as_str(), "pixi"),
        ]}, {
            insta::assert_snapshot!(result);
        });
    }

    #[test]
    pub(crate) fn test_bash_completion_working_regex() {
        // Generate the original completion script.
        let script = get_completion_script(Shell::Bash);
        // Test if there was a replacement done on the clap generated completions
        assert_ne!(replace_bash_completion(&script), script);
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
        let replaced = replace_nushell_completion(&script);
        if replaced == script {
            panic!(
                "Completion replacement did not work as expected\n\n======================\n= Original script\n======================\n{script}"
            );
        }

        // The externs for the commands that forward their trailing arguments
        // must have been replaced by `def --wrapped` wrappers.
        let bin_name = pixi_utils::executable_name();
        for subcommand in ["run", "exec"] {
            assert!(
                replaced.contains(&format!("export def --wrapped \"{bin_name} {subcommand}\"")),
                "expected a wrapped def for `{bin_name} {subcommand}`"
            );
            assert!(
                !replaced.contains(&format!("export extern \"{bin_name} {subcommand}\"")),
                "the extern for `{bin_name} {subcommand}` should have been replaced"
            );
        }
    }
}
