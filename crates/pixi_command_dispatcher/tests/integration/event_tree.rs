//! Implements [`EventTree`] which enables outputting the hierarchy of the
//! operations that took place for a
//! [`pixi_command_dispatcher::CommandDispatcher`].
//!
//! For example, this could be the output:
//!
//! ```
//! Pixi solve (boost-check)
//! ├── Source metadata ({ git = "https://github.com/wolfv/pixi-build-examples.git", rev = "a4c27e86a4a5395759486552abb3df8a47d50172", subdirectory = "boost-check" })
//! │   ├── Git Checkout (https://github.com/wolfv/pixi-build-examples@a4c27e86a4a5395759486552abb3df8a47d50172)
//! │   └── Instantiate tool environment (pixi-build-cmake)
//! │       ├── Pixi solve (pixi-build-cmake)
//! │       │   └── Conda solve #0
//! │       └── Pixi install #0
//! └── Conda solve #1
//! ```

use std::{collections::HashMap, fmt::Display};

use event_reporter::Event;
use itertools::Itertools;
use pixi_command_dispatcher::{
    ReporterContext,
    reporter::{
        BackendSourceBuildId, BuildBackendMetadataId, CondaSolveId, GitCheckoutId,
        InstantiateToolEnvId, PixiInstallId, PixiSolveId, SourceBuildId, SourceMetadataId,
    },
};
use rattler_conda_types::PackageName;
use slotmap::SlotMap;
use text_trees::{FormatCharacters, StringTreeNode, TreeFormatting};

use crate::event_reporter;
use crate::event_reporter::EventStore;

/// An [`EventTree`] is a hierarchical representation of the events that
/// occurred in a [`pixi_command_dispatcher::CommandDispatcher`].
pub struct EventTree {
    rootes: Vec<NodeId>,
    nodes: slotmap::SlotMap<NodeId, Node>,
}

slotmap::new_key_type! {
    pub struct NodeId;
}

struct Node {
    label: String,
    children: Vec<NodeId>,
}

impl From<EventStore> for EventTree {
    fn from(store: EventStore) -> Self {
        Self::new(store.0.lock().unwrap().iter())
    }
}

impl EventTree {
    pub fn new<'i>(events: impl IntoIterator<Item = &'i Event>) -> Self {
        let mut builder = EventTreeBuilder::default();

        let mut checkout_label = HashMap::new();
        let mut pixi_solve_label = HashMap::new();
        let mut pixi_install_label = HashMap::new();
        let mut build_backend_metadata_label = HashMap::new();
        let mut source_metadata_label = HashMap::new();
        let mut source_build_label = HashMap::new();
        let mut backend_source_build_labels = HashMap::new();
        let mut instantiate_tool_env_label = HashMap::new();

        for event in events {
            match event {
                Event::CondaSolveQueued { id, context, .. } => {
                    builder.set_event_parent((*id).into(), *context);
                }
                Event::CondaSolveStarted { id } => {
                    builder.alloc_node((*id).into(), format!("Conda solve #{}", id.0));
                }
                Event::CondaSolveFinished { .. } => {}
                Event::PixiSolveQueued { id, context, spec } => {
                    pixi_solve_label.insert(
                        *id,
                        spec.dependencies
                            .names()
                            .map(PackageName::as_source)
                            .format(", ")
                            .to_string(),
                    );
                    builder.set_event_parent((*id).into(), *context);
                }
                Event::PixiSolveStarted { id } => {
                    builder.alloc_node(
                        (*id).into(),
                        format!("Pixi solve ({})", pixi_solve_label.get(id).unwrap()),
                    );
                }
                Event::PixiSolveFinished { .. } => {}
                Event::PixiInstallQueued { id, context, spec } => {
                    pixi_install_label.insert(*id, &spec.name);
                    builder.set_event_parent((*id).into(), *context);
                }
                Event::PixiInstallStarted { id } => {
                    builder.alloc_node(
                        (*id).into(),
                        format!("Pixi install ({})", pixi_install_label[id]),
                    );
                }
                Event::PixiInstallFinished { .. } => {}
                Event::GitCheckoutQueued {
                    id,
                    context,
                    reference,
                } => {
                    checkout_label.insert(
                        *id,
                        format!("{}@{}", reference.url.as_url(), reference.reference),
                    );
                    builder.set_event_parent((*id).into(), *context);
                }
                Event::GitCheckoutStarted { id } => {
                    builder.alloc_node(
                        (*id).into(),
                        format!("Git Checkout ({})", checkout_label.get(id).unwrap()),
                    );
                }
                Event::GitCheckoutFinished { .. } => {}
                Event::SourceMetadataQueued { id, context, spec } => {
                    source_metadata_label.insert(
                        *id,
                        format!(
                            "{} @ {}",
                            &spec.package.as_source(),
                            spec.backend_metadata.source
                        ),
                    );
                    builder.set_event_parent((*id).into(), *context);
                }
                Event::SourceMetadataStarted { id } => {
                    builder.alloc_node(
                        (*id).into(),
                        format!(
                            "Source metadata ({})",
                            source_metadata_label.get(id).unwrap()
                        ),
                    );
                }
                Event::SourceMetadataFinished { .. } => {}
                Event::BuildBackendMetadataQueued { id, context, spec } => {
                    build_backend_metadata_label.insert(*id, spec.source.to_string());
                    builder.set_event_parent((*id).into(), *context);
                }
                Event::BuildBackendMetadataStarted { id } => {
                    builder.alloc_node(
                        (*id).into(),
                        format!(
                            "Build backend metadata ({})",
                            build_backend_metadata_label.get(id).unwrap()
                        ),
                    );
                }
                Event::BuildBackendMetadataFinished { .. } => {}
                Event::SourceBuildQueued { id, context, spec } => {
                    source_build_label.insert(
                        *id,
                        format!("{} @ {}", spec.package.name.as_source(), spec.source),
                    );
                    builder.set_event_parent((*id).into(), *context);
                }
                Event::SourceBuildStarted { id } => {
                    builder.alloc_node(
                        (*id).into(),
                        format!("Source build ({})", source_build_label.get(id).unwrap()),
                    );
                }
                Event::SourceBuildFinished { .. } => {}
                Event::BackendSourceBuildQueued {
                    id,
                    package,
                    context,
                } => {
                    backend_source_build_labels.insert(*id, package.name.as_source().to_owned());
                    builder.set_event_parent((*id).into(), *context);
                }
                Event::BackendSourceBuildStarted { id } => {
                    builder.alloc_node(
                        (*id).into(),
                        format!(
                            "Backend source build ({})",
                            backend_source_build_labels.get(id).unwrap()
                        ),
                    );
                }
                Event::BackendSourceBuildFinished { .. } => {}
                Event::InstantiateToolEnvQueued { id, context, spec } => {
                    instantiate_tool_env_label
                        .insert(*id, spec.requirement.0.as_source().to_string());
                    builder.set_event_parent((*id).into(), *context);
                }
                Event::InstantiateToolEnvStarted { id } => {
                    builder.alloc_node(
                        (*id).into(),
                        format!(
                            "Instantiate tool environment ({})",
                            instantiate_tool_env_label.get(id).unwrap()
                        ),
                    );
                }
                Event::InstantiateToolEnvFinished { .. } => {}
            }
        }

        builder.finish()
    }
}

impl Display for EventTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn make_tree_node(nodes: &slotmap::SlotMap<NodeId, Node>, id: NodeId) -> StringTreeNode {
            let node = &nodes[id];
            let mut tree = StringTreeNode::new(node.label.clone());
            for child in &node.children {
                tree.push_node(make_tree_node(nodes, *child))
            }
            tree
        }

        let format = TreeFormatting::dir_tree(FormatCharacters::box_chars());
        for root in &self.rootes {
            write!(
                f,
                "{}",
                make_tree_node(&self.nodes, *root)
                    .to_string_with_format(&format)
                    .unwrap()
            )?;
        }

        Ok(())
    }
}

/// A helper struct that aids in the construction of an [`EventTree`].
#[derive(Default)]
struct EventTreeBuilder {
    nodes: SlotMap<NodeId, Node>,
    rootes: Vec<NodeId>,
    event_parent_nodes: HashMap<EventId, NodeId>,
    event_nodes: HashMap<EventId, NodeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::From)]
pub enum EventId {
    CondaSolve(CondaSolveId),
    PixiSolve(PixiSolveId),
    PixiInstall(PixiInstallId),
    GitCheckout(GitCheckoutId),
    SourceMetadata(SourceMetadataId),
    BuildBackendMetadata(BuildBackendMetadataId),
    InstantiateToolEnv(InstantiateToolEnvId),
    SourceBuild(SourceBuildId),
    BackendSourceBuild(BackendSourceBuildId),
}

impl From<ReporterContext> for EventId {
    fn from(context: ReporterContext) -> Self {
        match context {
            ReporterContext::SolvePixi(id) => Self::PixiSolve(id),
            ReporterContext::SolveConda(id) => Self::CondaSolve(id),
            ReporterContext::InstallPixi(id) => Self::PixiInstall(id),
            ReporterContext::BuildBackendMetadata(id) => Self::BuildBackendMetadata(id),
            ReporterContext::InstantiateToolEnv(id) => Self::InstantiateToolEnv(id),
            ReporterContext::SourceBuild(id) => Self::SourceBuild(id),
            ReporterContext::SourceMetadata(id) => Self::SourceMetadata(id),
            ReporterContext::BackendSourceBuild(id) => Self::BackendSourceBuild(id),
        }
    }
}

impl EventTreeBuilder {
    /// Allocate a node in the tree
    fn alloc_node(&mut self, event_id: EventId, label: String) -> NodeId {
        let id = self.nodes.insert(Node {
            label,
            children: Vec::new(),
        });

        if let Some(parent) = self.event_parent_nodes.get(&event_id) {
            self.nodes[*parent].children.push(id);
        } else {
            self.rootes.push(id);
        }

        self.event_nodes.insert(event_id, id);

        id
    }

    /// Set the parent for the node with the given [`EventId`].
    fn set_event_parent(&mut self, id: EventId, context: Option<ReporterContext>) {
        if let Some(context) = context
            .and_then(|context| self.event_nodes.get(&context.into()))
            .copied()
        {
            self.event_parent_nodes.insert(id, context);
        }
    }

    /// Finish the construction of the tree
    fn finish(self) -> EventTree {
        EventTree {
            rootes: self.rootes,
            nodes: self.nodes,
        }
    }
}
