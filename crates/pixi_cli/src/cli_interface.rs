use pixi_api::{Interface, InterfaceError, WorkspaceContext};
use pixi_core::Workspace;

/// Builds the [`WorkspaceContext`] every CLI command works through.
///
/// Attaching the progress reporter here rather than at each call site is what
/// keeps commands like `pixi add` from silently downloading gigabytes: every
/// context method that solves reports through it without the command having to
/// opt in. `search` is the exception, because it builds its own gateway rather
/// than going through the command dispatcher.
pub fn cli_context(workspace: Workspace) -> WorkspaceContext<CliInterface> {
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
        let theme = dialoguer::theme::ColorfulTheme {
            active_item_style: console::Style::new().for_stderr().magenta(),
            ..dialoguer::theme::ColorfulTheme::default()
        };

        Ok(dialoguer::Select::with_theme(&theme)
            .with_prompt(msg)
            .report(false)
            .items(options)
            .default(0)
            .interact_opt()
            .unwrap_or(None))
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
