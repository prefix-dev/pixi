//! Asking whether detectors from a channel may run and remembering the decision.

use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use dialoguer::{Select, theme::ColorfulTheme};
use pixi_config::{DetectorDecision, write_detector_decision, write_repository_detector_decision};
use pixi_progress::global_multi_progress;
use rattler_conda_types::ChannelUrl;
use shlex::try_quote;

use crate::host::{Consent, ConsentRequest, DetectorConsent};

#[derive(Clone, Copy)]
enum DecisionStore {
    Session,
    Repository,
    Shared,
}

struct Answer {
    decision: DetectorDecision,
    store: DecisionStore,
}

/// Prompts once per channel and remembers the answer for this process.
pub struct InteractiveConsent {
    project_root: Option<PathBuf>,
    decisions: tokio::sync::Mutex<HashMap<ChannelUrl, Consent>>,
}

impl InteractiveConsent {
    pub fn new(project_root: Option<&Path>) -> Self {
        Self {
            project_root: project_root.map(Path::to_owned),
            decisions: tokio::sync::Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl DetectorConsent for InteractiveConsent {
    async fn decide(&self, request: &ConsentRequest<'_>) -> Consent {
        let mut decisions = self.decisions.lock().await;
        let origin = &request.channel.base_url;
        if let Some(decision) = decisions.get(origin) {
            return *decision;
        }
        let channel = origin.to_string();
        let project_root = self.project_root.clone();
        let shared_path = pixi_config::shared_user_config_write_path();
        let answer = tokio::task::spawn_blocking(move || {
            global_multi_progress().suspend(|| ask(&channel, project_root.as_deref(), &shared_path))
        })
        .await
        .ok()
        .flatten();
        let Some(answer) = answer else {
            decisions.insert(origin.clone(), Consent::Deny);
            return Consent::Deny;
        };

        let decision = match answer.decision {
            DetectorDecision::Allow => Consent::Allow,
            DetectorDecision::Deny => Consent::Deny,
        };
        decisions.insert(origin.clone(), decision);
        let (path, flag, saved) = match answer.store {
            DecisionStore::Session => return decision,
            DecisionStore::Repository => {
                let root = self
                    .project_root
                    .as_deref()
                    .expect("repository scope has a root");
                (
                    root.join(".pixi/config.toml"),
                    Cow::Owned(format!(
                        "--local --manifest-path {}",
                        try_quote(&root.to_string_lossy()).expect("workspace paths contain no nul")
                    )),
                    write_repository_detector_decision(root, origin, answer.decision),
                )
            }
            DecisionStore::Shared => {
                let path = pixi_config::shared_user_config_write_path();
                let saved = write_detector_decision(&path, origin, answer.decision);
                (path, Cow::Borrowed("--shared"), saved)
            }
        };
        match saved {
            Ok(()) => eprintln!(
                "Saved in {}. To remove this decision, run:\n\n\
                 pixi config unset {flag} 'virtual-package-detectors.consent.{}'\n",
                path.display(),
                toml_edit::Key::new(origin.as_str().trim_end_matches('/')),
            ),
            Err(error) => tracing::warn!(
                "Could not save the decision in {}. Pixi will ask again next time: {error:?}",
                path.display()
            ),
        }
        decision
    }
}

fn ask(channel: &str, project_root: Option<&Path>, shared_path: &Path) -> Option<Answer> {
    eprintln!(
        "\nDetectors inspect your system to identify capabilities that packages depend on, \
         and run code on your machine.\n\
         Trust applies to current and future detectors from this channel."
    );
    let theme = ColorfulTheme::default();
    let selection = Select::with_theme(&theme)
        .with_prompt(format!("Trust detectors from {channel}?"))
        .items(["Don't trust", "Trust"])
        .default(0)
        .interact()
        .ok()?;
    let decision = if selection == 0 {
        DetectorDecision::Deny
    } else {
        DetectorDecision::Allow
    };
    let system = format!("System configuration ({})", shared_path.display());
    let workspace = project_root
        .map(|root| format!("Workspace ({})", root.join(".pixi/config.toml").display()));
    let options = [
        "Don't save",
        workspace.as_deref().unwrap_or(&system),
        system.as_str(),
    ];
    let selection = Select::with_theme(&theme)
        .with_prompt("Where should this decision be saved?")
        .items(&options[..if workspace.is_some() { 3 } else { 2 }])
        .default(0)
        .interact()
        .ok()?;
    let store = if selection == 0 {
        DecisionStore::Session
    } else if project_root.is_some() && selection == 1 {
        DecisionStore::Repository
    } else {
        DecisionStore::Shared
    };
    Some(Answer { decision, store })
}
