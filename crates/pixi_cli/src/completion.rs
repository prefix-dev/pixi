use crate::Args as CommandArgs;
use clap::{Arg, Command, CommandFactory, Parser, ValueEnum};
use clap_complete::{
    Generator,
    engine::{ArgValueCompleter, CompletionCandidate, ValueCompleter},
    shells,
};
use clap_complete_nushell::Nushell;
use miette::IntoDiagnostic;
use pixi_core::WorkspaceLocator;
use regex::{Captures, Regex};
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::io::Write;
use std::iter;
use std::sync::LazyLock;

/// `pixi` subcommand emitting the space-delimited workspace environment names.
const ENVIRONMENT_LIST: &str = "workspace environment list --machine-readable";
/// `pixi` subcommand emitting the space-delimited workspace platform names.
const PLATFORM_LIST: &str = "workspace platform list --machine-readable";
/// `pixi` subcommand emitting the space-delimited workspace feature names.
const FEATURE_LIST: &str = "workspace feature list --machine-readable";
/// `pixi` subcommand emitting the space-delimited task names.
const TASK_LIST: &str = "task list --machine-readable";

/// `pixi global` keeps its own environment namespace and installs for conda
/// subdirs, so workspace names must never be offered below it.
const GLOBAL_SUBCOMMAND: &str = "global";

/// Line clap_complete emits before the completion function, used to anchor the
/// helper functions the rewritten value specs call.
const ZSH_PREAMBLE: &str = "autoload -U is-at-least\n";

/// clap value names marking an option as taking a name a `pixi` subcommand can
/// list, paired with that subcommand.
///
/// Matched on this rather than on the flag, so a completable option has to
/// declare its value name. `test_value_names_are_normalised` holds it to that.
const VALUE_SOURCES: [(&str, &str); 4] = [
    ("ENVIRONMENT", ENVIRONMENT_LIST),
    ("PLATFORM", PLATFORM_LIST),
    ("FEATURE", FEATURE_LIST),
    ("TASK", TASK_LIST),
];

/// Compile one of this file's own patterns, none of which come from user input.
fn compile_regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("should be able to compile the regex")
}

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

/// Handle native, runtime completion requests made through `PIXI_COMPLETE`.
pub fn complete() {
    clap_complete::CompleteEnv::with_factory(dynamic_completion_command)
        .var("PIXI_COMPLETE")
        .bin("pixi")
        .complete();
}

fn dynamic_completion_command() -> Command {
    dynamic_completion_command_with_tasks(current_workspace_task_names())
}

fn dynamic_completion_command_with_tasks(task_names: Vec<String>) -> Command {
    CommandArgs::command().mut_subcommand("run", move |command| {
        command.trailing_var_arg(false).mut_arg("task", move |arg| {
            // The runtime parser stores the task and its arguments in one
            // trailing Vec. Narrow only the completion model to its first
            // value, otherwise clap offers task names for task arguments too.
            arg.action(clap::ArgAction::Set)
                .num_args(1)
                .trailing_var_arg(false)
                .add(ArgValueCompleter::new(TaskNameCompleter { task_names }))
        })
    })
}

fn current_workspace_task_names() -> Vec<String> {
    // Workspace-aware candidates are best-effort. A missing or invalid
    // workspace must not disable clap's normal command and option completion.
    let Ok(workspace) = WorkspaceLocator::for_cli()
        .with_emit_warnings(false)
        .locate()
    else {
        return Vec::new();
    };

    workspace
        .environments()
        .into_iter()
        .flat_map(|environment| environment.get_filtered_tasks())
        .map(|task| task.as_str().to_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

struct TaskNameCompleter {
    task_names: Vec<String>,
}

impl TaskNameCompleter {
    fn candidates(&self, current: &OsStr) -> Vec<CompletionCandidate> {
        let Some(prefix) = current.to_str() else {
            return Vec::new();
        };

        self.task_names
            .iter()
            .filter(|task| task.starts_with(prefix))
            .map(CompletionCandidate::new)
            .collect()
    }
}

impl ValueCompleter for TaskNameCompleter {
    fn complete(&self, current: &OsStr) -> Vec<CompletionCandidate> {
        self.candidates(current)
    }
}

/// Generate completions for the pixi cli, and print those to the stdout
pub fn execute(args: Args) -> miette::Result<()> {
    let cli = Cli::new();

    // For supported shells, modify the script to include more context sensitive completions.
    let script = cli.generate(args.shell);
    let script = match args.shell {
        Shell::Bash => replace_bash_completion(&cli, &script),
        Shell::Zsh => replace_zsh_completion(&cli, &script),
        Shell::Fish => replace_fish_completion(&cli, &script),
        Shell::Nushell => replace_nushell_completion(&cli, &script),
        _ => script,
    };

    // Write the result to the standard output
    std::io::stdout()
        .write_all(script.as_bytes())
        .into_diagnostic()?;

    Ok(())
}

/// What every shell's rewrite is driven by: how `pixi` was invoked, its command
/// tree, and the options whose values a `pixi` subcommand can list.
///
/// Built once per invocation, because walking the tree is not free and all four
/// shells need the same answers from it.
struct Cli {
    /// Name `pixi` was invoked as.
    bin_name: &'static str,
    /// The clap command tree, before the generators build it.
    root: Command,
    /// Every option a shell should complete from a `pixi` subcommand.
    targets: Vec<OptionTarget>,
    /// Name of the shell function bash and zsh route every rewritten site
    /// through. Both take the `pixi` subcommand to run as arguments, so one
    /// definition serves every candidate source.
    helper: String,
}

impl Cli {
    fn new() -> Self {
        Self::with_bin_name(pixi_utils::executable_name())
    }

    /// The CLI as it looks when `pixi` is invoked as `bin_name`, which decides
    /// both the completion function names and the command every rewritten site
    /// shells out to.
    fn with_bin_name(bin_name: &'static str) -> Self {
        let root = CommandArgs::command();
        let targets = option_targets(&root);
        Self {
            bin_name,
            root,
            targets,
            helper: format!("_{}_complete_values", bin_name.replace('-', "_")),
        }
    }

    /// Generate the completion script using clap_complete for a specified shell.
    fn generate(&self, shell: Shell) -> String {
        let mut buf = vec![];
        clap_complete::generate(shell, &mut CommandArgs::command(), self.bin_name, &mut buf);
        String::from_utf8(buf).expect("clap_complete did not generate a valid UTF8 script")
    }

    fn helper(&self) -> &str {
        &self.helper
    }
}

/// An option of one subcommand whose values a shell should complete from a
/// `pixi` subcommand's output instead of from file paths.
#[derive(Debug)]
struct OptionTarget {
    /// Subcommand path below the binary, e.g. `["workspace", "export",
    /// "conda-environment"]`.
    path: Vec<String>,
    /// Long flag name, without the leading dashes.
    long: String,
    /// Short flag, when the option has one.
    short: Option<char>,
    /// The clap value name that opted this option in.
    value_name: &'static str,
    /// `pixi` subcommand emitting the candidates.
    command: &'static str,
}

/// Visit every argument below `cmd`, together with the subcommand path it sits
/// on. The path is empty for the arguments of `cmd` itself.
fn walk_arguments<'a>(cmd: &'a Command, visit: &mut impl FnMut(&[&'a str], &'a Arg)) {
    fn walk<'a>(
        cmd: &'a Command,
        path: &mut Vec<&'a str>,
        visit: &mut impl FnMut(&[&'a str], &'a Arg),
    ) {
        for arg in cmd.get_arguments() {
            visit(path, arg);
        }
        for sub in cmd.get_subcommands() {
            path.push(sub.get_name());
            walk(sub, path, visit);
            path.pop();
        }
    }

    walk(cmd, &mut Vec::new(), visit);
}

/// The candidate source `arg` opted into through the clap value name it
/// declares, or `None` when it does not name something a `pixi` subcommand can
/// list.
fn value_source(arg: &Arg) -> Option<(&'static str, &'static str)> {
    let declared = arg.get_value_names()?.first()?;
    VALUE_SOURCES
        .iter()
        .find(|(value_name, _)| *value_name == declared.as_str())
        .copied()
}

/// Every option below `root` whose values a `pixi` subcommand can list,
/// recognised by the clap value name it declares.
fn option_targets(root: &Command) -> Vec<OptionTarget> {
    let mut targets = Vec::new();
    walk_arguments(root, &mut |path, arg| {
        // `pixi help <cmd>` mirrors the tree without carrying any options, and
        // `pixi global` names things from its own namespaces.
        if path
            .iter()
            .any(|segment| *segment == "help" || *segment == GLOBAL_SUBCOMMAND)
        {
            return;
        }
        let (Some(long), Some((value_name, command))) = (arg.get_long(), value_source(arg)) else {
            return;
        };
        targets.push(OptionTarget {
            path: path.iter().map(|segment| (*segment).to_string()).collect(),
            long: long.to_string(),
            short: arg.get_short(),
            value_name,
            command,
        });
    });
    targets
}

/// Every spelling a fish predicate can use for a top-level subcommand: its name
/// and its visible aliases.
fn subcommand_spellings(root: &Command) -> HashMap<&str, Vec<&str>> {
    root.get_subcommands()
        .map(|sub| {
            let spellings = iter::once(sub.get_name())
                .chain(sub.get_visible_aliases())
                .collect();
            (sub.get_name(), spellings)
        })
        .collect()
}

/// Candidate sources keyed by `(subcommand spelling, long flag)`.
///
/// A fish predicate names only an option's first path segment, and clap_complete
/// emits no line at all three deep, so that segment is all there is to match on.
/// `test_fish_first_segment_is_unambiguous` and
/// `test_fish_predicates_are_at_most_two_deep` hold both halves.
fn fish_option_sources(
    root: &Command,
    targets: &[OptionTarget],
) -> HashMap<(String, String), &'static str> {
    let spellings = subcommand_spellings(root);
    targets
        .iter()
        .filter_map(|target| {
            let first = target.path.first()?;
            Some((spellings.get(first.as_str())?, target))
        })
        .flat_map(|(spellings, target)| {
            spellings.iter().map(|spelling| {
                (
                    ((*spelling).to_string(), target.long.clone()),
                    target.command,
                )
            })
        })
        .collect()
}

/// Replace the parts of the bash completion script that need different functionality.
fn replace_bash_completion(cli: &Cli, script: &str) -> String {
    let script = replace_bash_option_values(cli, script);
    let script = replace_bash_run_tasks(cli, &script);
    format!("{}{script}", bash_helper_definition(cli.helper()))
}

/// Complete from the output of a `pixi` subcommand, falling back to file paths
/// when it yields nothing (for instance outside a workspace).
fn bash_helper_definition(helper: &str) -> String {
    format!(
        r#"{helper}() {{
    local current="$1"
    shift
    local candidates
    if candidates=$("$@" 2> /dev/null) && [[ -n "${{candidates}}" ]]; then
        COMPREPLY=( $(compgen -W "${{candidates}}" -- "${{current}}") )
    else
        COMPREPLY=( $(compgen -f "${{current}}") )
    fi
}}

"#
    )
}

/// The bash call that offers what `command` lists.
fn bash_candidates(cli: &Cli, command: &str) -> String {
    format!(r#"{} "${{cur}}" {} {command}"#, cli.helper(), cli.bin_name)
}

/// clap's bash generator mangles the command path into the variable it matches
/// on: segments are joined with `__`, every `-` becomes `__`, and every `__`
/// then becomes `__subcmd__`.
fn bash_arm_name(bin_name: &str, path: &[String]) -> String {
    iter::once(bin_name)
        .chain(path.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("__")
        .replace('-', "__")
        .replace("__", "__subcmd__")
}

/// clap_complete indents the subcommand labels of the dispatch `case` by eight
/// spaces, which is what separates them from the nested option arms.
static BASH_ARM_LABEL: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"(?m)^ {8}(?P<arm>[A-Za-z0-9_]+)\)$"));

/// Rewrite the `case "${prev}"` arms of every completable option, in the
/// subcommand arm it belongs to.
///
/// Arms are delimited by their `<cmd>)` labels rather than by the `;;` that
/// closes them, because the body contains a nested `case` with `;;` of its own.
fn replace_bash_option_values(cli: &Cli, script: &str) -> String {
    let by_arm = cli.targets.iter().fold(
        HashMap::<String, Vec<&OptionTarget>>::new(),
        |mut acc, target| {
            acc.entry(bash_arm_name(cli.bin_name, &target.path))
                .or_default()
                .push(target);
            acc
        },
    );

    let labels: Vec<Captures> = BASH_ARM_LABEL.captures_iter(script).collect();
    let mut result = String::with_capacity(script.len());
    let mut cursor = 0;
    for (index, caps) in labels.iter().enumerate() {
        let label = caps.get(0).expect("group 0 always participates");
        let body_end = labels.get(index + 1).map_or(script.len(), |next| {
            next.get(0).expect("group 0 always participates").start()
        });
        result.push_str(&script[cursor..label.end()]);
        let body = &script[label.end()..body_end];
        match by_arm.get(&caps["arm"]) {
            Some(arm_targets) => {
                let patched = arm_targets.iter().fold(body.to_string(), |body, target| {
                    complete_bash_option(cli, &body, target)
                });
                result.push_str(&patched);
            }
            None => result.push_str(body),
        }
        cursor = body_end;
    }
    result.push_str(&script[cursor..]);
    result
}

/// Point one option's `case "${prev}"` arms at the shared helper.
fn complete_bash_option(cli: &Cli, body: &str, target: &OptionTarget) -> String {
    let long = regex::escape(&target.long);
    let labels = match target.short {
        Some(short) => format!("--{long}|-{}", regex::escape(&short.to_string())),
        None => format!("--{long}"),
    };
    let pattern = format!(
        r#"(?m)^(?P<indent>\s*)(?P<label>{labels})\)\n\s*COMPREPLY=\(\$\(compgen -f "\$\{{cur\}}"\)\)"#
    );
    let candidates = bash_candidates(cli, target.command);
    compile_regex(&pattern)
        .replace_all(body, |caps: &Captures| {
            let indent = &caps["indent"];
            let label = &caps["label"];
            format!(
                r#"{indent}{label})
{indent}    {candidates}"#
            )
        })
        .into_owned()
}

/// Complete task names for `pixi run` at any position on the command line.
fn replace_bash_run_tasks(cli: &Cli, script: &str) -> String {
    // NOTE THIS IS FORMATTED BY HAND
    let arm = bash_arm_name(cli.bin_name, &["run".to_string()]);
    // Keep the generated `case "${prev}"` block (option-value completion) and
    // run task completion after it, so tasks complete at any position.
    let pattern =
        format!(r#"(?s){arm}\).*?opts="(?P<opts>.*?)".*?if.*?fi\s*(?P<case>case.*?esac)"#);
    compile_regex(&pattern)
        .replace(script, |caps: &Captures| {
            let opts = &caps["opts"];
            let case = &caps["case"];
            // Not `bash_candidates`: with no tasks to offer this falls through to
            // the generated tail, which lists the flags, rather than to file paths.
            format!(
                r#"{arm})
            opts="{opts}"
            if [[ ${{cur}} == -* ]] ; then
                COMPREPLY=( $(compgen -W "${{opts}}" -- "${{cur}}") )
                return 0
            fi
            {case}
            local tasks
            if tasks=$({bin_name} {TASK_LIST} 2> /dev/null) && [[ -n "${{tasks}}" ]]; then
                COMPREPLY=( $(compgen -W "${{tasks}}" -- "${{cur}}") )
                return 0
            fi"#,
                bin_name = cli.bin_name
            )
        })
        .into_owned()
}

/// Replace the parts of the zsh completion script that need different functionality.
fn replace_zsh_completion(cli: &Cli, script: &str) -> String {
    // zsh spec lines carry no subcommand path, but they do carry the clap value
    // name, which identifies the option. `pixi global` is ruled out inside the
    // helper instead, from the command line being completed.
    let script = VALUE_SOURCES
        .iter()
        .fold(script.to_string(), |script, (value_name, command)| {
            script.replace(
                &format!(":{value_name}:_default"),
                &format!(":{value_name}:{}", zsh_candidates(cli, command)),
            )
        });

    // Anchor on the preamble rather than on `_<bin_name>()`, whose spelling
    // depends on the binary name.
    let script = script.replace(
        ZSH_PREAMBLE,
        &format!("{ZSH_PREAMBLE}\n{}", zsh_helper_definition(cli)),
    );

    replace_zsh_run_tasks(cli, &script)
}

/// The zsh action that offers what `command` lists.
///
/// Wrapped in braces, which `_arguments` evaluates verbatim. A bare word list
/// would also be run as a command, but `_arguments` prepends its own `compadd`
/// options to it (`-J -default- …`), and those would reach `pixi` as arguments.
fn zsh_candidates(cli: &Cli, command: &str) -> String {
    format!("{{{} {command}}}", cli.helper())
}

/// Complete from the output of a `pixi` subcommand, deferring to the default
/// completion below `pixi global`, which has its own namespaces.
///
/// The generated script rewrites `curcontext` into the canonical subcommand
/// path, so it identifies the subtree even behind an alias or an option value.
fn zsh_helper_definition(cli: &Cli) -> String {
    format!(
        r#"{helper}() {{
    if [[ $curcontext == *:{bin_name}-{GLOBAL_SUBCOMMAND}-* ]]; then
        _default
        return
    fi
    local -a candidates
    candidates=(${{=$({bin_name} "$@" 2> /dev/null)}})
    if (( ${{#candidates}} )); then
        compadd -a candidates
    else
        _default
    fi
}}

"#,
        helper = cli.helper(),
        bin_name = cli.bin_name
    )
}

/// The `*::task` rest-argument spec of `pixi run`, whose action clap leaves at
/// the default. There is one per spelling of `run`, the name and its alias.
static ZSH_RUN_POSITIONAL: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"(?m)^(?P<spec>'\*::task[^\n]*?):_default(?P<tail>' \\)$"));

/// Complete task names for the rest arguments of `pixi run`.
///
/// Rewrites that spec's action rather than calling `_values` ahead of
/// `_arguments`, which offered tasks in every context (even as an option value),
/// skipped `_arguments` when there were no tasks, and missed the `r` alias arm.
fn replace_zsh_run_tasks(cli: &Cli, script: &str) -> String {
    ZSH_RUN_POSITIONAL
        .replace_all(script, |caps: &Captures| {
            format!(
                "{spec}:{action}{tail}",
                spec = &caps["spec"],
                action = zsh_candidates(cli, TASK_LIST),
                tail = &caps["tail"]
            )
        })
        .into_owned()
}

/// Replace the parts of the fish completion script that need different functionality.
fn replace_fish_completion(cli: &Cli, script: &str) -> String {
    let script = complete_fish_options(cli, script, &fish_option_sources(&cli.root, &cli.targets));

    // Adds tab completion to the pixi run command, under both its spellings.
    let addition = format!(
        "complete -c {bin_name} -n \"__fish_seen_subcommand_from run; or __fish_seen_subcommand_from r\" {}\n",
        fish_candidates(cli, TASK_LIST),
        bin_name = cli.bin_name
    );
    format!("{script}{addition}")
}

/// The fish fragment that offers what `command` lists, and nothing else.
fn fish_candidates(cli: &Cli, command: &str) -> String {
    format!(
        r#"-f -a "(string split ' ' ({} {command} 2> /dev/null))""#,
        cli.bin_name
    )
}

/// The subcommand a fish `complete` line's predicate is gated on.
static FISH_SUBCOMMAND: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"_using_subcommand (?P<name>[^\s;]+)"));

/// Append `-f -a "..."` to every `complete` line whose subcommand and long flag
/// name something a `pixi` subcommand can list.
///
/// fish emits one line per subcommand and per visible alias, each gated by a
/// `__fish_..._using_subcommand` predicate, which is where the subcommand
/// comes from.
fn complete_fish_options(
    cli: &Cli,
    script: &str,
    sources: &HashMap<(String, String), &'static str>,
) -> String {
    // Longest first, so a flag that is a prefix of another cannot shadow it,
    // and so the pattern does not depend on `HashMap` iteration order.
    let mut longs = sources
        .keys()
        .map(|(_, long)| regex::escape(long))
        .collect::<Vec<_>>();
    longs.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    longs.dedup();
    let longs = longs.join("|");
    let pattern = format!(
        r#"(?m)^(?P<line>complete -c {bin} -n "(?P<predicate>[^"]*)"[^\n]*? -l (?P<long>{longs}) [^\n]*? -r)$"#,
        bin = regex::escape(cli.bin_name)
    );
    compile_regex(&pattern)
        .replace_all(script, |caps: &Captures| {
            let line = &caps["line"];
            let Some(name) = FISH_SUBCOMMAND
                .captures(&caps["predicate"])
                .map(|predicate| predicate["name"].to_string())
            else {
                return line.to_string();
            };
            let Some(command) = sources.get(&(name, caps["long"].to_string())) else {
                return line.to_string();
            };
            format!("{line} {}", fish_candidates(cli, command))
        })
        .into_owned()
}

/// Line opening the module every generated nushell definition lives in.
const NUSHELL_MODULE: &str = "module completions {";

/// Matches one argument line of a nushell `export extern` block, splitting the
/// declared type off so that a completer can be appended after it.
///
/// Boolean flags carry no `: type` and so do not match.
static NUSHELL_ARG_LINE: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r"^(?P<pre>\s*(?P<name>[^\s:]+)\s*:\s*)(?P<ty>[^\s#@]+)(?P<tail>.*)$")
});

/// Replace the parts of the nushell completion script that need different functionality.
fn replace_nushell_completion(cli: &Cli, script: &str) -> String {
    // Adds tab completion to the pixi run command.
    // NOTE THIS IS FORMATTED BY HAND

    // One definition per candidate source, plus the `r` alias the generator
    // does not emit for `run`. nushell completers take no arguments, so unlike
    // bash and zsh it needs one definition per source rather than one helper.
    let definitions = VALUE_SOURCES
        .iter()
        .map(|(value_name, command)| nushell_definition(cli, value_name, command))
        .collect::<String>();
    let script = script.replacen(
        NUSHELL_MODULE,
        &format!(
            "{NUSHELL_MODULE}{definitions}\n    export alias \"{bin_name} r\" = {bin_name} run\n    \n",
            bin_name = cli.bin_name
        ),
        1,
    );

    // Point each argument that names something listable at the definition for
    // its value name. The `...task` of `pixi run` names the same things as
    // `--depends-on`, so it shares the task definition.
    let run = format!("{} run", cli.bin_name);
    rewrite_nushell_blocks(&script, |command, line| {
        let caps = NUSHELL_ARG_LINE.captures(line)?;
        // A type the generator already gave a completer is not ours to touch.
        if caps["tail"].starts_with('@') {
            return None;
        }
        let name = &caps["name"];
        let completion = if name == "...task" && command == run {
            nushell_candidates(cli, "TASK")
        } else {
            let long = name.strip_prefix("--")?.split('(').next()?;
            nushell_completion(cli, command, long)?
        };
        Some(format!(
            "{pre}{ty}{completion}{tail}",
            pre = &caps["pre"],
            ty = &caps["ty"],
            tail = &caps["tail"]
        ))
    })
}

/// Header opening an `export extern` block. The bare binary is declared
/// unquoted, every subcommand in quotes.
static NUSHELL_BLOCK_HEADER: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r#"^\s*export extern "?(?P<command>[^"\[]+?)"? \[\s*$"#));

/// Rewrite the argument lines of every `export extern "<cmd>" [ … ]` block,
/// replacing a line with whatever `rewrite(command, line)` returns.
///
/// Walked line by line, not matched as a bracket-delimited region: a `]` in an
/// option's help text would end such a match early and silently skip the rest
/// of the block. `pixi global install --package` has one.
fn rewrite_nushell_blocks(
    script: &str,
    mut rewrite: impl FnMut(&str, &str) -> Option<String>,
) -> String {
    let mut result = String::with_capacity(script.len());
    let mut block = None;
    for line in script.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if let Some(caps) = NUSHELL_BLOCK_HEADER.captures(content) {
            block = Some(caps["command"].to_string());
        } else if content.trim() == "]" {
            block = None;
        } else if let Some(replacement) = block
            .as_deref()
            .and_then(|command| rewrite(command, content))
        {
            result.push_str(&replacement);
            result.push_str(&line[content.len()..]);
            continue;
        }
        result.push_str(line);
    }
    result
}

/// The nushell reference to the definition for `value_name`.
fn nushell_candidates(cli: &Cli, value_name: &str) -> String {
    format!(
        r#"@"nu-complete {} {}""#,
        cli.bin_name,
        value_name.to_lowercase()
    )
}

/// A `nu-complete` definition listing the candidates emitted by `command`,
/// yielding an empty list when it fails, for instance outside a workspace.
fn nushell_definition(cli: &Cli, value_name: &str, command: &str) -> String {
    format!(
        r#"
    def "nu-complete {bin_name} {kind}" [] {{
      let result = (^{bin_name} {command} | complete)
      if $result.exit_code == 0 {{ $result.stdout | str trim | split row " " | where {{|it| $it != "" }} }} else {{ [] }}
    }}
"#,
        bin_name = cli.bin_name,
        kind = value_name.to_lowercase()
    )
}

/// The `nu-complete` reference for `long` under `cmd`, or `None` when that
/// command does not take a listable name there.
///
/// nushell names the whole command in each `export extern`, so the target is
/// matched on its full path.
fn nushell_completion(cli: &Cli, cmd: &str, long: &str) -> Option<String> {
    let path = cmd.strip_prefix(cli.bin_name)?.split_whitespace();
    let target = cli.targets.iter().find(|target| {
        target.long == long && target.path.iter().map(String::as_str).eq(path.clone())
    })?;
    Some(nushell_candidates(cli, target.value_name))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, ffi::OsString};

    use clap::ValueHint;

    use super::*;

    /// The real CLI, as every shell's rewrite sees it.
    fn real_cli() -> Cli {
        Cli::new()
    }

    /// The bash fixture spells the binary `pixi`, so the rewrite has to too.
    fn bash_fixture_cli() -> Cli {
        Cli::with_bin_name("pixi")
    }

    fn dynamic_candidates(args: &[&str], index: usize) -> HashSet<String> {
        let mut command = dynamic_completion_command_with_tasks(vec![
            "build-docs".to_owned(),
            "lint".to_owned(),
            "test".to_owned(),
        ]);
        clap_complete::engine::complete(
            &mut command,
            args.iter().map(OsString::from).collect(),
            index,
            None,
        )
        .expect("dynamic completion should succeed")
        .into_iter()
        .map(|candidate| candidate.get_value().to_string_lossy().into_owned())
        .collect()
    }

    #[test]
    fn test_dynamic_completion_uses_the_full_clap_command_tree() {
        let root = dynamic_candidates(&["pixi", "work"], 1);
        assert!(root.contains("workspace"));

        let nested = dynamic_candidates(&["pixi", "workspace", "plat"], 2);
        assert!(nested.contains("platform"));
    }

    #[test]
    fn test_dynamic_completion_adds_current_workspace_tasks_to_run() {
        let tasks = dynamic_candidates(&["pixi", "run", "bu"], 2);
        assert_eq!(tasks, HashSet::from(["build-docs".to_owned()]));

        let task_arguments = dynamic_candidates(&["pixi", "run", "build-docs", ""], 3);
        assert!(!task_arguments.contains("build-docs"));
        assert!(!task_arguments.contains("lint"));
        assert!(!task_arguments.contains("test"));
    }

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
'*::task -- The pixi task or a deno task shell command you want to run in the project's environment, which can be an executable in the environment's PATH.:_default' \
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
'-e+[The environment to activate in the shell]:ENVIRONMENT:_default' \
'--environment=[The environment to activate in the shell]:ENVIRONMENT:_default' \
&& ret=0
;;

        "#;
        let cli = real_cli();
        let result = replace_zsh_completion(&cli, script);
        // Normalize the test binary name (e.g. `pixi_cli-<hash>`) to `pixi` so
        // the snapshot is stable across builds. Helper function names spell it
        // with underscores, so both forms need filtering.
        let underscored = cli.bin_name.replace('-', "_");
        insta::with_settings!({filters => vec![
            (cli.bin_name, "pixi"),
            (underscored.as_str(), "pixi"),
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
            if [[ ${cur} == -* ]] ; then
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
        pixi__subcmd__shell)
            opts="-e -h --environment --help"
            if [[ ${cur} == -* ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --environment)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -e)
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
        pixi__subcmd__global__subcmd__install)
            opts="-e -h --environment --help"
            if [[ ${cur} == -* ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --environment)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -e)
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
        pixi__subcmd__search)
            opts="-c -l -v -q -h --channel --color --help <PACKAGE>"
            if [[ ${cur} == -* ]] ; then
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
        let result = replace_bash_completion(&bash_fixture_cli(), script);
        assert_ne!(result, script);
        insta::assert_snapshot!(result);
    }

    #[test]
    pub(crate) fn test_nushell_completion() {
        // The rewrites key off the binary name, which is the test binary here,
        // so the fixture has to be built with it rather than a literal `pixi`.
        let cli = real_cli();
        // NOTE THIS IS FORMATTED BY HAND!
        let script = format!(
            r#"
  # Runs task in project
  export extern "{bin_name} run" [
    ...task: string           # The pixi task or a task shell command you want to run in the project's environment, which can be an executable in the environment's PATH
    --manifest-path: string   # The path to `pixi.toml`, `pyproject.toml`, or the project directory
    --no-lockfile-update      # Legacy flag, do not use, will be removed in subsequent version
    --frozen                  # Install the environment as defined in the lock file, doesn't update lock file if it isn't up-to-date with the manifest file
    --locked                  # Check if lock file is up-to-date before installing the environment, aborts when lock file isn't up-to-date with the manifest file
    --no-install              # Don't modify the environment, only modify the lock file
    --tls-no-verify           # Do not verify the TLS certificate of the server
    --auth-file: string       # Path to the file containing the authentication token
    --pypi-keyring-provider: string # Specifies if we want to use uv keyring provider
    --concurrent-solves: string # Max concurrent solves, default is the number of CPUs
    --concurrent-downloads: string # Max concurrent network requests, default is 50
    --force-activate          # Do not use the environment activation cache. (default: true except in experimental mode)
    --environment(-e): string # The environment to run the task in
    --platform(-p): string    # Install and run in the environment for the given platform
    --clean-env               # Use a clean environment to run the task
    --skip-deps               # Don't run the dependencies of the task ('depends-on' field in the task definition)
    --verbose(-v)             # Increase logging verbosity
    --quiet(-q)               # Decrease logging verbosity
    --no-progress             # Hide all progress bars, always turned on if stderr is not a terminal
    --help(-h)                # Print help (see more with '--help')
  ]

  # Activate a shell
  export extern "{bin_name} shell" [
    --environment(-e): string # The environment to activate in the shell
  ]

  # Install a global tool
  export extern "{bin_name} global install" [
    --environment(-e): string # Ensures that all packages will be installed in the same environment
    --platform(-p): string    # The platform to install the packages for
  ]"#,
            bin_name = cli.bin_name
        );
        let result = replace_nushell_completion(&cli, &script);
        // Normalize the test binary name (e.g. `pixi_cli-<hash>`) to `pixi` so
        // the snapshot is stable across builds.
        insta::with_settings!({filters => vec![
            (cli.bin_name, "pixi"),
        ]}, {
            insta::assert_snapshot!(result);
        });
    }

    /// The tree walk must find the options the issue is about, and must leave
    /// `pixi global` alone.
    #[test]
    fn test_option_targets_cover_workspace_commands_only() {
        let cli = real_cli();
        let targets = &cli.targets;
        let completed = targets
            .iter()
            .map(|target| format!("{} --{}", target.path.join(" "), target.long))
            .collect::<Vec<_>>();

        for option in [
            "shell --environment",
            "shell-hook --environment",
            "install --environment",
            "add --environment",
            "task add --environment",
            "run --platform",
            "task add --feature",
            "task add --depends-on",
        ] {
            assert!(
                completed.iter().any(|entry| entry == option),
                "`pixi {option}` should complete workspace names"
            );
        }
        assert!(
            targets
                .iter()
                .all(|target| target.path.first().map(String::as_str) != Some(GLOBAL_SUBCOMMAND)),
            "`pixi global` has its own namespaces and must be left alone"
        );
    }

    /// The tree walk skips `pixi global` by name and the zsh helper matches
    /// `curcontext` against the same spelling. Renaming or emptying it would
    /// make both silently start offering workspace names, and would make every
    /// "global is excluded" assertion pass with nothing left to exclude.
    #[test]
    fn test_global_subcommand_is_worth_excluding() {
        /// Does `cmd` or anything below it take a name a shell would rewrite?
        fn completable(cmd: &Command) -> bool {
            let mut found = false;
            walk_arguments(cmd, &mut |_, arg| {
                found |= arg.get_long().is_some() && value_source(arg).is_some();
            });
            found
        }

        let cli = real_cli();
        let global = cli
            .root
            .get_subcommands()
            .find(|sub| sub.get_name() == GLOBAL_SUBCOMMAND)
            .expect("`pixi global` is gone, so the exclusions naming it are dead code");
        assert!(
            completable(global),
            "`pixi {GLOBAL_SUBCOMMAND}` no longer takes a name any shell would rewrite, so \
             excluding it has become a no-op"
        );
    }

    /// Options that name something the workspace does not have yet, and so must
    /// keep a value name outside [`VALUE_SOURCES`], paired with that name.
    ///
    /// `pixi init` and `pixi exec` resolve their platform without a workspace,
    /// and `pixi import` names the environment and feature it creates.
    const OPTED_OUT: [(&str, &str, &str); 4] = [
        ("exec", "platform", "SUBDIR"),
        ("import", "environment", "NEW_ENVIRONMENT"),
        ("import", "feature", "NEW_FEATURE"),
        ("init", "platform", "NEW_PLATFORM"),
    ];

    /// Options are matched on their clap value name, which clap derives from
    /// the field name unless told otherwise. Any completable option that lets it
    /// do so silently drops out of every completion, so hold the whole CLI to
    /// the normalised spelling — or, for the deliberate exceptions, to the
    /// spelling that keeps them out.
    #[test]
    fn test_value_names_are_normalised() {
        let mut wrong = Vec::new();
        walk_arguments(&CommandArgs::command(), &mut |path, arg| {
            let (Some(long), Some(value_name)) =
                (arg.get_long(), arg.get_value_names().and_then(<[_]>::first))
            else {
                return;
            };
            let opted_out = OPTED_OUT
                .iter()
                .find(|(command, flag, _)| [*command] == *path && *flag == long)
                .map(|(_, _, value_name)| *value_name);
            let expected = match (opted_out, long) {
                (Some(value_name), _) => value_name,
                (None, "environment" | "default-environment") => "ENVIRONMENT",
                (None, "platform") => "PLATFORM",
                (None, "feature") => "FEATURE",
                (None, "depends-on") => "TASK",
                (None, _) => return,
            };
            if value_name.as_str() != expected {
                wrong.push(format!(
                    "`pixi {} --{long}` should declare `value_name = \"{expected}\"`, not \
                     `{value_name}`",
                    path.join(" ")
                ));
            }
        });
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// The opted-out options must stay out of the tree walk, so that bash never
    /// addresses them and zsh never sees their value name.
    #[test]
    fn test_opted_out_options_are_not_targets() {
        let cli = real_cli();
        let targets = &cli.targets;
        let mut hints = HashMap::new();
        walk_arguments(&cli.root, &mut |path, arg| {
            if let Some(long) = arg.get_long() {
                hints.insert((path.to_vec(), long), arg.get_value_hint());
            }
        });

        for (command, flag, value_name) in OPTED_OUT {
            assert!(
                !targets
                    .iter()
                    .any(|target| target.path == [command] && target.long == flag),
                "`pixi {command} --{flag}` names something the workspace cannot list yet"
            );
            assert!(
                !VALUE_SOURCES.iter().any(|(name, _)| *name == value_name),
                "`{value_name}` is a candidate source, so it cannot opt an option out"
            );
            // Nothing can be offered for these, so they must not fall back to
            // the shell default either, which is file paths.
            assert_eq!(
                hints.get(&(vec![command], flag)).copied(),
                Some(ValueHint::Other),
                "`pixi {command} --{flag}` should declare `value_hint = ValueHint::Other`"
            );
        }
    }

    /// fish and nushell name the flag rather than the value name, so they are the
    /// two that could rewrite an opted-out option anyway.
    #[test]
    fn test_opted_out_options_are_not_rewritten() {
        let cli = real_cli();
        let fish_script = cli.generate(Shell::Fish);
        let nushell_script = cli.generate(Shell::Nushell);
        let fish = replace_fish_completion(&cli, &fish_script);
        let nushell = replace_nushell_completion(&cli, &nushell_script);

        // Each half locates the option first and asserts it was found, because
        // "nothing leaked" reads the same as "nothing was looked at" -- a
        // predicate or block header that stops matching would pass silently.
        for (command, flag, _) in OPTED_OUT {
            let addressed = fish
                .lines()
                .filter(|line| {
                    line.contains(&format!("_using_subcommand {command}\""))
                        && line.contains(&format!(" -l {flag} "))
                })
                .collect::<Vec<_>>();
            assert!(
                !addressed.is_empty(),
                "no fish line completes `pixi {command} --{flag}` at all any more"
            );
            let leaked = addressed
                .iter()
                .filter(|line| line.contains("--machine-readable"))
                .collect::<Vec<_>>();
            assert!(
                leaked.is_empty(),
                "fish still completes `pixi {command} --{flag}`: {leaked:#?}"
            );

            let block = format!("{} {command}", cli.bin_name);
            let mut addressed = Vec::new();
            rewrite_nushell_blocks(&nushell, |declared, line| {
                if declared == block && line.trim_start().starts_with(&format!("--{flag}")) {
                    addressed.push(line.to_string());
                }
                None
            });
            assert!(
                !addressed.is_empty(),
                "no `{block}` argument line names `--{flag}` any more"
            );
            let leaked = addressed
                .iter()
                .filter(|line| line.contains("nu-complete"))
                .collect::<Vec<_>>();
            assert!(
                leaked.is_empty(),
                "nushell still completes `pixi {command} --{flag}`: {leaked:#?}"
            );
        }
    }

    /// clap_complete gates a fish `complete` line on at most two subcommands, so
    /// anything deeper than that cannot be addressed in fish at all. Record the
    /// limit, and which targets it costs, so that a clap_complete bump which
    /// starts emitting deeper predicates shows up as a failure here.
    #[test]
    fn test_fish_predicates_are_at_most_two_deep() {
        let cli = real_cli();
        let script = cli.generate(Shell::Fish);
        let nested = Regex::new(r"_using_subcommand [^\n]*?_from [^\n]*?_from ")
            .expect("should be able to compile the regex");
        let unreachable = cli
            .targets
            .iter()
            .filter(|target| target.path.len() > 2)
            .map(|target| format!("pixi {} --{}", target.path.join(" "), target.long))
            .collect::<Vec<_>>();
        assert!(
            !nested.is_match(&script),
            "fish now addresses three subcommands, so `fish_option_sources` can stop \
             keying on the first segment alone and reach {unreachable:#?}"
        );
        assert!(
            !unreachable.is_empty(),
            "nothing is out of fish's reach any more, so drop this test and the note \
             on `fish_option_sources`"
        );
    }

    /// fish matches only the first segment of an option's path, so a subcommand
    /// must not mix completable and opted-out spellings of one long flag.
    #[test]
    fn test_fish_first_segment_is_unambiguous() {
        let cli = real_cli();
        let targets = &cli.targets;
        let completable = targets
            .iter()
            .map(|target| target.long.as_str())
            .collect::<HashSet<_>>();

        let root = CommandArgs::command();
        let mut found = Vec::new();
        walk_arguments(&root, &mut |path, arg| {
            if let Some(long) = arg.get_long()
                && !path.is_empty()
                && completable.contains(long)
            {
                found.push((path.to_vec(), long));
            }
        });

        let mut verdicts: HashMap<(&str, &str), HashSet<bool>> = HashMap::new();
        for (path, long) in &found {
            let is_target = targets.iter().any(|target| {
                target.long == *long
                    && target
                        .path
                        .iter()
                        .map(String::as_str)
                        .eq(path.iter().copied())
            });
            verdicts
                .entry((path[0], long))
                .or_default()
                .insert(is_target);
        }
        let mixed = verdicts
            .iter()
            .filter(|(_, verdicts)| verdicts.len() > 1)
            .map(|((command, long), _)| format!("pixi {command} … --{long}"))
            .collect::<Vec<_>>();
        assert!(
            mixed.is_empty(),
            "fish cannot tell these apart from the subcommand alone: {mixed:#?}"
        );

        // Two targets sharing a key must at least agree on what to offer:
        // `fish_option_sources` keeps whichever the walk inserted last.
        let mut sources: HashMap<(&str, &str), HashSet<&str>> = HashMap::new();
        for target in targets {
            if let Some(first) = target.path.first() {
                sources
                    .entry((first.as_str(), target.long.as_str()))
                    .or_default()
                    .insert(target.command);
            }
        }
        let disagreeing = sources
            .iter()
            .filter(|(_, commands)| commands.len() > 1)
            .map(|((command, long), _)| format!("pixi {command} … --{long}"))
            .collect::<Vec<_>>();
        assert!(
            disagreeing.is_empty(),
            "fish would silently pick one source for each of these: {disagreeing:#?}"
        );
    }

    /// Every targeted option must actually be rewritten; a clap_complete bump
    /// that changes the generated layout has to fail here rather than silently
    /// drop completions.
    #[test]
    fn test_bash_rewrites_every_target() {
        let cli = real_cli();
        let script = cli.generate(Shell::Bash);
        let patched = replace_bash_completion(&cli, &script);

        for target in &cli.targets {
            let labels = iter::once(format!("--{}", target.long))
                .chain(target.short.map(|short| format!("-{short}")));
            for label in labels {
                let expected = format!(
                    "{label})\n                    {helper} \"${{cur}}\" {bin_name} {command}",
                    helper = cli.helper(),
                    bin_name = cli.bin_name,
                    command = target.command
                );
                assert!(
                    patched.contains(&expected),
                    "`pixi {} {label}` still falls back to file completion",
                    target.path.join(" ")
                );
            }
        }
    }

    /// `pixi global` keeps the clap default of file completion.
    #[test]
    fn test_bash_leaves_global_alone() {
        let cli = real_cli();
        let script = cli.generate(Shell::Bash);
        let patched = replace_bash_completion(&cli, &script);
        let arm = format!(
            "{}__subcmd__install)",
            bash_arm_name(cli.bin_name, &[GLOBAL_SUBCOMMAND.to_string()])
        );
        let body = patched
            .split(&arm)
            .nth(1)
            .expect("the `pixi global install` arm should exist");
        let body = body.split_once("\n        ").map_or(body, |(body, _)| body);
        assert!(
            !body.contains(cli.helper()),
            "`pixi global install` must not complete workspace names"
        );
    }

    /// fish gets one `complete` line per subcommand and per visible alias. Each
    /// must be rewritten exactly when its subcommand and long flag name
    /// something a `pixi` subcommand can list — no more, no less.
    #[test]
    fn test_fish_rewrites_exactly_the_completable_lines() {
        let cli = real_cli();
        let sources = fish_option_sources(&cli.root, &cli.targets);
        let script = cli.generate(Shell::Fish);
        let patched = replace_fish_completion(&cli, &script);

        // Matched against the whole line here, so the quote closing the
        // predicate has to be excluded too.
        let subcommand = Regex::new(r#"_using_subcommand (?P<name>[^\s;"]+)"#)
            .expect("should be able to compile the regex");
        let flag =
            Regex::new(r" -l (?P<long>[\w-]+) ").expect("should be able to compile the regex");

        let wrong = patched
            .lines()
            .filter(|line| line.starts_with("complete -c"))
            .filter_map(|line| {
                let name = subcommand.captures(line)?["name"].to_string();
                let long = flag.captures(line)?["long"].to_string();
                let completable = sources.contains_key(&(name, long));
                let rewritten = line.contains("--machine-readable");
                (completable != rewritten).then_some((completable, line))
            })
            .map(|(completable, line)| {
                let verb = if completable { "should" } else { "must not" };
                format!("{verb} complete workspace names: {line}")
            })
            .collect::<Vec<_>>();
        assert!(wrong.is_empty(), "{wrong:#?}");

        assert!(
            patched.contains(&format!(
                "-l environment -d 'The environment to activate in the shell' -r -f -a \"(string split ' ' ({bin_name} {ENVIRONMENT_LIST} 2> /dev/null))\"",
                bin_name = cli.bin_name
            )),
            "`pixi shell --environment` should complete environment names"
        );
    }

    /// nushell is the one shell whose flags the generator names literally, so
    /// it is the one that can silently miss a source. Every flag has to carry a
    /// definition, and every definition it names has to exist.
    #[test]
    fn test_nushell_rewrites_exactly_the_completable_arguments() {
        let cli = real_cli();
        let script = cli.generate(Shell::Nushell);
        let patched = replace_nushell_completion(&cli, &script);
        let run = format!("{} run", cli.bin_name);

        let mut wrong = Vec::new();
        rewrite_nushell_blocks(&patched, |command, line| {
            let caps = NUSHELL_ARG_LINE.captures(line)?;
            let name = caps["name"].to_string();
            // The generator gives its own `nu-complete` definitions to every
            // `ValueEnum` flag, so only our four count as a rewrite.
            let completed = VALUE_SOURCES.iter().any(|(value_name, _)| {
                caps["tail"].starts_with(&nushell_candidates(&cli, value_name))
            });
            let expected = if name == "...task" && command == run {
                true
            } else {
                name.strip_prefix("--")
                    .and_then(|long| long.split('(').next())
                    .is_some_and(|long| nushell_completion(&cli, command, long).is_some())
            };
            if completed != expected {
                let verb = if expected { "should" } else { "must not" };
                wrong.push(format!("`{command} {name}` {verb} be completed"));
            }
            None
        });
        assert!(wrong.is_empty(), "{wrong:#?}");

        for kind in patched
            .split("string@\"nu-complete ")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
        {
            let definitions = patched
                .matches(&format!("def \"nu-complete {kind}\" []"))
                .count();
            assert_eq!(
                definitions, 1,
                "`nu-complete {kind}` should be defined exactly once, not {definitions} times"
            );
        }
    }

    /// zsh keys off the clap value name, so no `_default` action may survive on
    /// a completable spec, and every action the specs name has to be defined in
    /// the same script.
    #[test]
    fn test_zsh_rewrites_every_value_spec() {
        let cli = real_cli();
        let script = cli.generate(Shell::Zsh);
        let patched = replace_zsh_completion(&cli, &script);
        let helper = cli.helper();
        assert!(
            patched.contains(&format!("{helper}() {{")),
            "`{helper}` is called but never defined"
        );
        for (value_name, command) in VALUE_SOURCES {
            // Counted rather than merely searched for: one rewritten spec is no
            // evidence about the rest, and a spec clap emits with some other
            // action (a `value_hint` would do it) would slip through.
            let marker = format!(":{value_name}:");
            let action = format!("{marker}{}", zsh_candidates(&cli, command));
            let declared = patched.matches(marker.as_str()).count();
            assert!(declared > 0, "no spec declares `{value_name}` any more");
            assert_eq!(
                patched.matches(action.as_str()).count(),
                declared,
                "only some of the {declared} `{value_name}` specs call `{action}`"
            );
        }
    }
}
