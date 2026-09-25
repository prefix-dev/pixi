//! Asking whether detectors from a channel may run and remembering the decision.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use dialoguer::{Confirm, Select, theme::ColorfulTheme};
use pixi_config::{DetectorDecision, write_detector_decision, write_repository_detector_decision};
use pixi_progress::global_multi_progress;
use rattler_conda_types::ChannelUrl;

use crate::host::{Consent, ConsentRequest, DetectorConsent};

#[derive(Clone, Copy)]
enum DecisionStore {
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
        let has_repository = self.project_root.is_some();
        let answer = tokio::task::spawn_blocking(move || {
            global_multi_progress().suspend(|| ask(&channel, has_repository))
        })
        .await
        .ok()
        .flatten();
        let Some(answer) = answer else {
            decisions.insert(origin.clone(), Consent::Deny);
            return Consent::Deny;
        };

        let (path, flag, saved) = match answer.store {
            DecisionStore::Repository => {
                let root = self
                    .project_root
                    .as_deref()
                    .expect("repository scope has a root");
                (
                    root.join(".pixi/config.toml"),
                    "--local",
                    write_repository_detector_decision(root, origin, answer.decision),
                )
            }
            DecisionStore::Shared => {
                let path = pixi_config::shared_user_config_write_path();
                let saved = write_detector_decision(&path, origin, answer.decision);
                (path, "--shared", saved)
            }
        };
        match saved {
            Ok(()) => eprintln!(
                "Saved in {}. Change with `pixi config set {flag} \
                 'virtual-package-detectors.consent.{}' allow` or `deny`.",
                path.display(),
                toml_edit::Key::new(origin.as_str().trim_end_matches('/')),
            ),
            Err(error) => tracing::warn!(
                "Could not save the decision in {}. Pixi will ask again next time: {error:?}",
                path.display()
            ),
        }
        let decision = match answer.decision {
            DetectorDecision::Allow => Consent::Allow,
            DetectorDecision::Deny => Consent::Deny,
        };
        decisions.insert(origin.clone(), decision);
        decision
    }
}

fn ask(channel: &str, has_repository: bool) -> Option<Answer> {
    eprintln!(
        "\nDetectors inspect your system and run code on your machine.\n\
         Trust applies to current and future detectors from this channel."
    );
    let theme = ColorfulTheme::default();
    let allow = Confirm::with_theme(&theme)
        .with_prompt(format!("Trust detectors from {channel}?"))
        .default(false)
        .interact()
        .ok()?;
    let decision = if allow {
        DetectorDecision::Allow
    } else {
        DetectorDecision::Deny
    };
    let store = if has_repository {
        let selection = Select::with_theme(&theme)
            .with_prompt("Where should this decision apply?")
            .items(["This repository", "All repositories (shared configuration)"])
            .default(0)
            .interact()
            .ok()?;
        if selection == 0 {
            DecisionStore::Repository
        } else {
            DecisionStore::Shared
        }
    } else {
        DecisionStore::Shared
    };
    Some(Answer { decision, store })
}
