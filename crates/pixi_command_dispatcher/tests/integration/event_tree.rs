//! Implements [`EventTree`] which enables outputting the hierarchy of the
//! operations that took place for a
//! [`pixi_command_dispatcher::CommandDispatcher`].
//!
//! The hierarchy is reconstructed by walking each operation's parent
//! chain via the shared [`OperationRegistry`].

use std::{collections::HashMap, fmt::Display, sync::Arc};

use event_reporter::Event;
use pixi_compute_reporters::{OperationId, OperationRegistry};
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

impl EventTree {
    pub fn from_store(store: &EventStore, registry: &Arc<OperationRegistry>) -> Self {
        Self::new(store.0.lock().unwrap().iter(), registry)
    }

    pub fn new<'i>(
        events: impl IntoIterator<Item = &'i Event>,
        registry: &Arc<OperationRegistry>,
    ) -> Self {
        let mut builder = EventTreeBuilder::new(registry);

        for event in events {
            match event {
                Event::CondaSolveQueued { id, .. } => builder.queue(*id),
                Event::CondaSolveStarted { id } => {
                    builder.alloc_node(*id, format!("Conda solve #{}", id.0));
                }
                Event::CondaSolveFinished { .. } => {}
                Event::PixiSolveQueued { id, spec } => {
                    builder.queue_with_label(*id, format!("Pixi solve ({})", spec.name));
                }
                Event::PixiSolveStarted { id } => builder.alloc_pending(*id),
                Event::PixiSolveFinished { .. } => {}
                Event::PixiInstallQueued { id, spec } => {
                    builder.queue_with_label(*id, format!("Pixi install ({})", spec.name));
                }
                Event::PixiInstallStarted { id } => builder.alloc_pending(*id),
                Event::PixiInstallFinished { .. } => {}
                Event::GitCheckoutQueued { id, reference } => {
                    builder.queue_with_label(
                        *id,
                        format!(
                            "Git Checkout ({}@{})",
                            reference.url.as_url(),
                            reference.reference
                        ),
                    );
                }
                Event::GitCheckoutStarted { id } => builder.alloc_pending(*id),
                Event::GitCheckoutFinished { .. } => {}
                Event::SourceMetadataQueued { id, spec } => {
                    builder.queue_with_label(
                        *id,
                        format!(
                            "Source metadata ({} @ {})",
                            spec.package.as_source(),
                            spec.backend_metadata.manifest_source
                        ),
                    );
                }
                Event::SourceMetadataStarted { id } => builder.alloc_pending(*id),
                Event::SourceMetadataFinished { .. } => {}
                Event::SourceRecordQueued { id, spec } => {
                    builder.queue_with_label(
                        *id,
                        format!(
                            "Source record ({} @ {})",
                            spec.package.as_source(),
                            spec.backend_metadata.manifest_source
                        ),
                    );
                }
                Event::SourceRecordStarted { id } => builder.alloc_pending(*id),
                Event::SourceRecordFinished { .. } => {}
                Event::BuildBackendMetadataQueued { id, spec } => {
                    builder.queue_with_label(
                        *id,
                        format!("Build backend metadata ({})", spec.manifest_source),
                    );
                }
                Event::BuildBackendMetadataStarted { id } => builder.alloc_pending(*id),
                Event::BuildBackendMetadataFinished { .. } => {}
                Event::BackendSourceBuildQueued { id, package } => {
                    builder.queue_with_label(*id, format!("Backend source build ({package})"));
                }
                Event::BackendSourceBuildStarted { id } => builder.alloc_pending(*id),
                Event::BackendSourceBuildFinished { .. } => {}
                Event::InstantiateBackendQueued { id, spec } => {
                    builder.queue_with_label(*id, format!("Instantiate backend ({})", spec.name));
                }
                Event::InstantiateBackendStarted { id } => builder.alloc_pending(*id),
                Event::InstantiateBackendFinished { .. } => {}
                Event::UrlCheckoutQueued { .. }
                | Event::UrlCheckoutStarted { .. }
                | Event::UrlCheckoutFinished { .. } => {
                    // URL checkouts don't participate in the tree display.
                }
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
struct EventTreeBuilder<'r> {
    registry: &'r Arc<OperationRegistry>,
    nodes: SlotMap<NodeId, Node>,
    rootes: Vec<NodeId>,
    /// Labels for queued ops not yet alloc'd; alloc-on-started picks these up.
    pending_labels: HashMap<OperationId, String>,
    /// Allocated node per operation id.
    op_nodes: HashMap<OperationId, NodeId>,
}

impl<'r> EventTreeBuilder<'r> {
    fn new(registry: &'r Arc<OperationRegistry>) -> Self {
        Self {
            registry,
            nodes: SlotMap::with_key(),
            rootes: Vec::new(),
            pending_labels: HashMap::new(),
            op_nodes: HashMap::new(),
        }
    }

    /// Record the existence of a queued op without a label (used for
    /// conda-solve, which builds its label from the started event).
    fn queue(&mut self, _id: OperationId) {}

    fn queue_with_label(&mut self, id: OperationId, label: String) {
        self.pending_labels.insert(id, label);
    }

    /// Allocate a tree node for `op_id` whose label was previously
    /// recorded via [`queue_with_label`]. Walks parents via the
    /// registry to attach the node to the correct slot.
    fn alloc_pending(&mut self, op_id: OperationId) {
        let label = self
            .pending_labels
            .remove(&op_id)
            .unwrap_or_else(|| format!("op#{}", op_id.0));
        self.alloc_node(op_id, label);
    }

    /// Allocate a tree node directly with the given label (used by
    /// CondaSolveStarted which doesn't go through `queue_with_label`).
    fn alloc_node(&mut self, op_id: OperationId, label: String) -> NodeId {
        let id = self.nodes.insert(Node {
            label,
            children: Vec::new(),
        });

        let parent = self
            .registry
            .ancestors(op_id)
            .find_map(|ancestor| self.op_nodes.get(&ancestor).copied());
        match parent {
            Some(parent) => self.nodes[parent].children.push(id),
            None => self.rootes.push(id),
        }

        self.op_nodes.insert(op_id, id);
        id
    }

    fn finish(self) -> EventTree {
        EventTree {
            rootes: self.rootes,
            nodes: self.nodes,
        }
    }
}
