use pixi_api::{GlobalContext, Interface, InterfaceError, WorkspaceContext, global::Project};
use pixi_core::Workspace;

/// Builds the [`GlobalContext`] every `pixi global` command works through.
pub fn cli_global_context(project: Project) -> GlobalContext<CliInterface> {
    GlobalContext::new(CliInterface {}, project)
}

/// Builds the [`WorkspaceContext`] every CLI command works through.
///
/// Attaching the progress reporter here rather than at each call site is what
/// keeps commands like `pixi add` from silently downloading gigabytes: every
/// context method that solves reports through it without the command having to
/// opt in. `search` is the exception, because it builds its own gateway rather
/// than going through the command dispatcher.
pub fn cli_workspace_context(workspace: Workspace) -> WorkspaceContext<CliInterface> {
    WorkspaceContext::new(CliInterface {}, workspace)
        .with_progress(pixi_reporters::TopLevelProgress::from_global())
}

#[derive(Default)]
pub struct CliInterface {}

impl Interface for CliInterface {
    async fn is_cli(&self) -> bool {
        true
    }

    async fn confirm(&self, msg: &str) -> Result<bool, InterfaceError> {
        dialoguer::Confirm::new()
            .with_prompt(msg)
            .default(false)
            .show_default(true)
            .interact()
            .map_err(InterfaceError::new)
    }

    async fn select(&self, msg: &str, options: &[String]) -> Result<Option<usize>, InterfaceError> {
        // The prompt draws on stderr, so without a terminal nobody can
        // answer, and no choice is made.
        if !console::Term::stderr().is_term() {
            return Ok(None);
        }

        let theme = dialoguer::theme::ColorfulTheme {
            active_item_style: console::Style::new().for_stderr().magenta(),
            ..dialoguer::theme::ColorfulTheme::default()
        };

        match dialoguer::Select::with_theme(&theme)
            .with_prompt(msg)
            .report(false)
            .items(options)
            .default(0)
            .interact_opt()
        {
            // Ctrl-C cancels the prompt, like Esc.
            Err(dialoguer::Error::IO(err)) if err.kind() == std::io::ErrorKind::Interrupted => {
                Ok(None)
            }
            result => result.map_err(InterfaceError::new),
        }
    }

    async fn info(&self, msg: &str) {
        eprintln!("{msg}");
    }

    async fn success(&self, msg: &str) {
        eprintln!("{}{msg}", console::style(console::Emoji("✔ ", "")).green());
    }

    async fn warning(&self, msg: &str) {
        eprintln!(
            "{}{msg}",
            console::style(console::Emoji("⚠️ ", "")).yellow(),
        );
    }

    async fn error(&self, msg: &str) {
        eprintln!(
            "{}{msg}",
            console::style(console::Emoji("❌ ", "")).yellow(),
        );
    }
}
