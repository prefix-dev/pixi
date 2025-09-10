use miette::IntoDiagnostic;
use pixi_api::interface::Interface;

#[derive(Default)]
pub struct CliInterface {}

impl Interface for CliInterface {
    fn is_cli(&self) -> bool {
        true
    }

    fn confirm(&self, msg: &str) -> miette::Result<bool> {
        dialoguer::Confirm::new()
            .with_prompt(msg)
            .default(false)
            .show_default(true)
            .interact()
            .into_diagnostic()
    }

    fn message(&self, msg: &str) {
        eprintln!("{msg}",);
    }

    fn success(&self, msg: &str) {
        eprintln!("{}{msg}", console::style(console::Emoji("✔ ", "")).green(),);
    }

    fn warning(&self, msg: &str) {
        eprintln!(
            "{}{msg}",
            console::style(console::Emoji("⚠️ ", "")).yellow(),
        );
    }

    fn error(&self, msg: &str) {
        eprintln!(
            "{}{msg}",
            console::style(console::Emoji("❌ ", "")).yellow(),
        );
    }
}
