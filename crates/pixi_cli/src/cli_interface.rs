use miette::IntoDiagnostic;
use pixi_api::{interface::Interface, styled_text::StyledText};

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

    fn styled(&self, text: StyledText) -> String {
        let mut styled = console::style(text.text());
        if text.bold {
            styled = styled.bold();
        }
        if text.green {
            styled = styled.green();
        }
        styled.to_string()
    }
}
