use miette::IntoDiagnostic;
use pixi_api::Interface;

#[derive(Default)]
pub struct CliInterface {}

impl Interface for CliInterface {
    async fn is_cli(&self) -> bool {
        true
    }

    async fn confirm(&self, msg: &str) -> miette::Result<bool> {
        dialoguer::Confirm::new()
            .with_prompt(msg)
            .default(false)
            .show_default(true)
            .interact()
            .into_diagnostic()
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
