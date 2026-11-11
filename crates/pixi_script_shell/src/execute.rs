use std::{
    collections::HashMap,
    ffi::OsString,
    future::Future,
    path::PathBuf,
    pin::Pin,
    process::{ExitStatus, Stdio},
};

use deno_task_shell::KillSignal;
use miette::Diagnostic;
use thiserror::Error;
use tokio::process::Child;

use crate::{Command, CommandSequence, QuotedPart, Word, WordPart};

/// Everything an entrypoint runs against: the substitution variables, the
/// activated environment, the working directory and the signal handle.
#[derive(Debug, Clone)]
pub struct ShellContext {
    /// The `${VAR}` substitution values; an entrypoint defines `SCRIPT` and
    /// `CACHE`.
    pub variables: HashMap<String, String>,
    /// The full environment the commands run in, including the activation
    /// variables of the solved environment.
    pub env: HashMap<OsString, OsString>,
    /// The working directory, left at the directory the tool was invoked
    /// from.
    pub cwd: PathBuf,
    /// The handle the caller forwards signals through. Every running command
    /// receives them, and dropping the execution kills the command outright.
    pub kill_signal: KillSignal,
}

/// A failure while evaluating or running an entrypoint.
#[derive(Debug, Error, Diagnostic)]
pub enum ShellExecutionError {
    #[error("the entrypoint refers to the undefined variable `${{{name}}}`")]
    #[diagnostic(help("a conda-script entrypoint defines `${{SCRIPT}}` and `${{CACHE}}`"))]
    UndefinedVariable { name: String },

    #[error("`{argument}` looks like an environment variable assignment")]
    #[diagnostic(help(
        "a conda-script entrypoint cannot set environment variables; it only runs commands"
    ))]
    EnvAssignment { argument: String },

    #[error("failed to run `{command}`")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    #[error("`{command}` inside `$(...)` exited with code {code}")]
    #[diagnostic(help("a failing command inside `$(...)` aborts the entrypoint"))]
    SubstitutionFailed { command: String, code: i32 },

    #[error("the output of `{command}` inside `$(...)` is not valid UTF-8")]
    SubstitutionNotUtf8 { command: String },
}

/// Runs a command sequence, appending `extra_args` to the last command.
///
/// Returns the exit code of the sequence: `0` when every command succeeded,
/// otherwise the code of the command that failed and aborted it.
pub async fn execute_sequence(
    sequence: &CommandSequence,
    extra_args: &[String],
    context: &ShellContext,
) -> Result<i32, ShellExecutionError> {
    let last = sequence.commands.len().saturating_sub(1);
    for (index, command) in sequence.commands.iter().enumerate() {
        let mut argv = evaluate_command(command, context).await?;
        let Some(program) = argv.first().cloned() else {
            // A command whose words all evaluated to nothing, such as a lone
            // `$(...)` with empty output, runs nothing and succeeds. The
            // appended arguments vanish with it; they must never become the
            // program themselves.
            continue;
        };
        if index == last {
            argv.extend(extra_args.iter().cloned());
        }
        let arguments = &argv[1..];
        reject_env_assignment(&program)?;

        let mut child = tokio::process::Command::new(resolve_program(&program, context))
            .args(arguments)
            .env_clear()
            .envs(&context.env)
            .current_dir(&context.cwd)
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| ShellExecutionError::Spawn {
                command: program.clone(),
                source,
            })?;
        let status = wait_forwarding_signals(&mut child, &context.kill_signal, &program).await?;
        let code = exit_code(&status);
        if code != 0 {
            return Ok(code);
        }
    }
    Ok(0)
}

/// Resolves `program` through the `PATH` of the environment the command runs
/// in.
///
/// Spawning only tries `.exe` on Windows, while conda packages often install
/// `.bat` or `.cmd` wrappers, so the lookup goes through `which`, which
/// honours `PATHEXT`. An unresolved name is passed on as is, so the spawn
/// error still names the missing program.
fn resolve_program(program: &str, context: &ShellContext) -> OsString {
    let path = context
        .env
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.clone());
    which::which_in(program, path, &context.cwd)
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|_| OsString::from(program))
}

/// Waits for `child`, forwarding every signal sent through `kill_signal` to
/// it.
///
/// Windows cannot deliver a signal to a process, so there a signal that
/// aborts the command kills the child instead.
async fn wait_forwarding_signals(
    child: &mut Child,
    kill_signal: &KillSignal,
    program: &str,
) -> Result<ExitStatus, ShellExecutionError> {
    loop {
        tokio::select! {
            status = child.wait() => {
                return status.map_err(|source| ShellExecutionError::Spawn {
                    command: program.to_owned(),
                    source,
                });
            }
            signal = kill_signal.wait_any() => {
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    forward_signal(pid, signal);
                }
                #[cfg(not(unix))]
                if signal.causes_abort() {
                    let _ = child.start_kill();
                }
            }
        }
    }
}

#[cfg(unix)]
fn forward_signal(pid: u32, signal: deno_task_shell::SignalKind) {
    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };

    let signo: i32 = signal.into();
    if let Ok(signal) = Signal::try_from(signo) {
        // A child that exited in the meantime is reaped by `wait`; nothing
        // to do about a failed delivery here.
        let _ = kill(Pid::from_raw(pid as i32), signal);
    }
}

/// The exit code of a finished process, with a signal death encoded as
/// `128 + signal_number` like a POSIX shell reports it.
fn exit_code(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

/// Runs a substituted command sequence, capturing its stdout.
///
/// The commands share stderr with the entrypoint; their stdout is
/// concatenated and a single trailing newline is stripped.
fn run_substitution<'a>(
    sequence: &'a CommandSequence,
    context: &'a ShellContext,
) -> Pin<Box<dyn Future<Output = Result<String, ShellExecutionError>> + 'a>> {
    Box::pin(async move {
        let mut output = Vec::new();
        for command in &sequence.commands {
            let argv = evaluate_command(command, context).await?;
            let Some((program, arguments)) = argv.split_first() else {
                continue;
            };
            reject_env_assignment(program)?;

            // `output()` would force stderr into a pipe and swallow it; the
            // diagnostics of a failing command belong on the terminal.
            let mut child = tokio::process::Command::new(resolve_program(program, context))
                .args(arguments)
                .env_clear()
                .envs(&context.env)
                .current_dir(&context.cwd)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|source| ShellExecutionError::Spawn {
                    command: program.clone(),
                    source,
                })?;
            let mut stdout = child.stdout.take().expect("stdout was configured as piped");
            let mut chunk = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut chunk)
                .await
                .map_err(|source| ShellExecutionError::Spawn {
                    command: program.clone(),
                    source,
                })?;
            let status = wait_forwarding_signals(&mut child, &context.kill_signal, program).await?;
            if !status.success() {
                return Err(ShellExecutionError::SubstitutionFailed {
                    command: program.clone(),
                    code: exit_code(&status),
                });
            }
            output.extend_from_slice(&chunk);
        }

        let mut output =
            String::from_utf8(output).map_err(|_| ShellExecutionError::SubstitutionNotUtf8 {
                command: display_sequence(sequence),
            })?;
        if output.ends_with('\n') {
            output.pop();
            if output.ends_with('\r') {
                output.pop();
            }
        }
        Ok(output)
    })
}

async fn evaluate_command(
    command: &Command,
    context: &ShellContext,
) -> Result<Vec<String>, ShellExecutionError> {
    let mut argv = Vec::new();
    for word in &command.words {
        argv.extend(evaluate_word(word, context).await?);
    }
    Ok(argv)
}

/// Evaluates one word into arguments.
///
/// Variable values and quoted content never split; only the output of an
/// unquoted `$(...)` splits on whitespace. A word built solely from
/// substitutions with empty output vanishes, while a quoted empty string
/// stays an argument.
async fn evaluate_word(
    word: &Word,
    context: &ShellContext,
) -> Result<Vec<String>, ShellExecutionError> {
    let mut arguments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for part in &word.parts {
        match part {
            WordPart::Literal(text) => current.push_str(text),
            WordPart::SingleQuoted(text) => {
                current.push_str(text);
                quoted = true;
            }
            WordPart::Variable(name) => current.push_str(variable(name, context)?),
            WordPart::DoubleQuoted(parts) => {
                quoted = true;
                for part in parts {
                    match part {
                        QuotedPart::Literal(text) => current.push_str(text),
                        QuotedPart::Variable(name) => current.push_str(variable(name, context)?),
                        QuotedPart::Substitution(sequence) => {
                            current.push_str(&run_substitution(sequence, context).await?);
                        }
                    }
                }
            }
            WordPart::Substitution(sequence) => {
                let output = run_substitution(sequence, context).await?;
                if output.starts_with(char::is_whitespace) && !current.is_empty() {
                    arguments.push(std::mem::take(&mut current));
                }
                let mut pieces = output.split_whitespace();
                if let Some(first) = pieces.next() {
                    current.push_str(first);
                    for piece in pieces {
                        arguments.push(std::mem::take(&mut current));
                        current.push_str(piece);
                    }
                }
                if output.ends_with(char::is_whitespace) && !current.is_empty() {
                    arguments.push(std::mem::take(&mut current));
                }
            }
        }
    }
    if quoted || !current.is_empty() {
        arguments.push(current);
    }
    Ok(arguments)
}

fn variable<'a>(name: &str, context: &'a ShellContext) -> Result<&'a str, ShellExecutionError> {
    context
        .variables
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| ShellExecutionError::UndefinedVariable {
            name: name.to_owned(),
        })
}

/// Rejects a program that reads as `NAME=value`, the shell syntax for an
/// environment variable assignment the mini-shell deliberately lacks.
fn reject_env_assignment(program: &str) -> Result<(), ShellExecutionError> {
    let Some((name, _)) = program.split_once('=') else {
        return Ok(());
    };
    let mut chars = name.chars();
    let valid_name = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid_name {
        Err(ShellExecutionError::EnvAssignment {
            argument: program.to_owned(),
        })
    } else {
        Ok(())
    }
}

/// A short rendering of a sequence for error messages.
fn display_sequence(sequence: &CommandSequence) -> String {
    sequence
        .commands
        .iter()
        .flat_map(|command| command.words.first())
        .map(|word| {
            word.parts
                .iter()
                .map(|part| match part {
                    WordPart::Literal(text) | WordPart::SingleQuoted(text) => text.clone(),
                    WordPart::Variable(name) => format!("${{{name}}}"),
                    WordPart::DoubleQuoted(_) => "\"...\"".to_owned(),
                    WordPart::Substitution(_) => "$(...)".to_owned(),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" && ")
}

#[cfg(all(test, unix))]
mod tests {
    use std::{path::Path, time::Duration};

    use super::*;
    use crate::parse_sequence;

    fn context(cwd: &Path, variables: &[(&str, &str)]) -> ShellContext {
        ShellContext {
            variables: variables
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            env: std::env::vars_os().collect(),
            cwd: cwd.to_owned(),
            kill_signal: KillSignal::default(),
        }
    }

    #[tokio::test]
    async fn programs_resolve_through_the_environment_path() {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        fs_err::create_dir(&bin).unwrap();
        let hello = bin.join("hello");
        fs_err::write(&hello, "#!/bin/sh\n: > marker\n").unwrap();
        fs_err::set_permissions(&hello, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let mut context = context(directory.path(), &[]);
        context.env = HashMap::from([(OsString::from("PATH"), bin.into_os_string())]);

        let code = run("hello", &[], &context).await.unwrap();

        assert_eq!(code, 0);
        assert!(directory.path().join("marker").exists());
    }

    #[tokio::test]
    async fn a_forwarded_signal_reaches_the_running_command() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);
        let kill_signal = context.kill_signal.clone();

        let send_term = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            kill_signal.send(nix::libc::SIGTERM.into());
        };
        let (code, ()) = tokio::join!(run("sleep 30", &[], &context), send_term);

        assert_eq!(code.unwrap(), 143);
    }

    #[tokio::test]
    async fn dropping_the_execution_kills_the_running_command() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        // One child process that would leave a marker after a short sleep;
        // abandoning the execution while it sleeps must take it down.
        let abandoned = tokio::time::timeout(
            Duration::from_millis(200),
            run("sh -c 'sleep 0.5; touch marker'", &[], &context),
        )
        .await;
        assert!(abandoned.is_err());

        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(!directory.path().join("marker").exists());
    }

    async fn run(
        input: &str,
        extra_args: &[&str],
        context: &ShellContext,
    ) -> Result<i32, ShellExecutionError> {
        let sequence = parse_sequence(input).unwrap();
        let extra_args: Vec<String> = extra_args.iter().map(ToString::to_string).collect();
        execute_sequence(&sequence, &extra_args, context).await
    }

    #[tokio::test]
    async fn a_failing_command_aborts_the_sequence() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        let code = run("false && touch marker", &[], &context).await.unwrap();

        assert_eq!(code, 1);
        assert!(!directory.path().join("marker").exists());
    }

    #[tokio::test]
    async fn arguments_are_appended_to_the_last_command() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        let code = run("true && touch", &["marker"], &context).await.unwrap();

        assert_eq!(code, 0);
        assert!(directory.path().join("marker").exists());
    }

    #[tokio::test]
    async fn unquoted_substitution_output_splits_into_arguments() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        run("touch $(printf 'first  second')", &[], &context)
            .await
            .unwrap();

        assert!(directory.path().join("first").exists());
        assert!(directory.path().join("second").exists());
    }

    #[tokio::test]
    async fn quoted_substitution_output_stays_one_argument() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        run("touch \"$(printf 'first second')\"", &[], &context)
            .await
            .unwrap();

        assert!(directory.path().join("first second").exists());
    }

    #[tokio::test]
    async fn a_trailing_newline_is_stripped_from_substitution_output() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        run("touch prefix-$(printf 'suffix\\n')", &[], &context)
            .await
            .unwrap();

        assert!(directory.path().join("prefix-suffix").exists());
    }

    #[tokio::test]
    async fn substitutions_nest() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        run("touch $(printf '%s' $(printf 'inner'))", &[], &context)
            .await
            .unwrap();

        assert!(directory.path().join("inner").exists());
    }

    #[tokio::test]
    async fn variables_substitute_without_splitting() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("cache dir");
        fs_err::create_dir(&cache).unwrap();
        let context = context(directory.path(), &[("CACHE", cache.to_str().unwrap())]);

        run("touch ${CACHE}/artifact", &[], &context).await.unwrap();

        assert!(cache.join("artifact").exists());
    }

    #[tokio::test]
    async fn single_quotes_suppress_substitution() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[("CACHE", "unused")]);

        run("touch '${CACHE}'", &[], &context).await.unwrap();

        assert!(directory.path().join("${CACHE}").exists());
    }

    #[tokio::test]
    async fn an_undefined_variable_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        let error = run("touch ${MISSING}", &[], &context).await.unwrap_err();

        assert!(matches!(
            error,
            ShellExecutionError::UndefinedVariable { name } if name == "MISSING"
        ));
    }

    #[tokio::test]
    async fn a_failing_substitution_aborts_the_entrypoint() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        let error = run("touch $(false) marker", &[], &context)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ShellExecutionError::SubstitutionFailed { command, code: 1 } if command == "false"
        ));
        assert!(!directory.path().join("marker").exists());
    }

    #[tokio::test]
    async fn an_environment_assignment_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        let error = run("FOO=bar true", &[], &context).await.unwrap_err();

        assert!(matches!(
            error,
            ShellExecutionError::EnvAssignment { argument } if argument == "FOO=bar"
        ));
    }

    #[tokio::test]
    async fn whitespace_at_substitution_edges_starts_a_new_argument() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        // `pkg-config`-style output with trailing whitespace must not glue
        // onto an adjacent literal.
        run("touch first$(printf ' second ')third", &[], &context)
            .await
            .unwrap();

        assert!(directory.path().join("first").exists());
        assert!(directory.path().join("second").exists());
        assert!(directory.path().join("third").exists());
    }

    #[tokio::test]
    async fn a_signal_death_reports_the_conventional_exit_code() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        // `perl` kills itself with SIGKILL (9); single quotes keep `$$` away
        // from the mini-shell's substitution syntax.
        let code = run("perl -e 'kill 9, $$'", &[], &context).await.unwrap();

        assert_eq!(code, 137);
    }

    #[tokio::test]
    async fn appended_arguments_never_become_the_program() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        // The last command evaluates to nothing, so the appended arguments
        // vanish with it instead of being executed themselves.
        let code = run("$(true)", &["touch", "marker"], &context)
            .await
            .unwrap();

        assert_eq!(code, 0);
        assert!(!directory.path().join("marker").exists());
    }

    #[tokio::test]
    async fn a_missing_program_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path(), &[]);

        let error = run("definitely-not-a-real-program-42", &[], &context)
            .await
            .unwrap_err();

        assert!(matches!(error, ShellExecutionError::Spawn { .. }));
    }
}
