use std::{collections::HashMap, fmt::Display};

use itertools::Itertools;
use pixi_command_dispatcher::{
    ReporterContext,
    reporter::{
        CondaSolveId, GitCheckoutId, InstantiateToolEnvId, PixiInstallId, PixiSolveId,
        SourceMetadataId,
    },
};
use rattler_conda_types::PackageName;
use slotmap::SlotMap;
use text_trees::{FormatCharacters, StringTreeNode, TreeFormatting};

use crate::event_reporter::Event;

slotmap::new_key_type! {
    pub struct NodeId;
}

pub struct Node {
    label: String,
    children: Vec<NodeId>,
}

pub struct EventTree {
    rootes: Vec<NodeId>,
    nodes: slotmap::SlotMap<NodeId, Node>,
}

impl EventTree {
    pub fn new<'i>(events: impl IntoIterator<Item = &'i Event>) -> Self {
        let mut builder = EventTreeBuilder::new();

        let mut checkout_name = HashMap::new();
        let mut pixi_solve_name = HashMap::new();
        let mut source_metadata_name = HashMap::new();
        let mut instantiate_tool_env_name = HashMap::new();

        for event in events {
            match event {
                Event::CondaSolveQueued { id, context, .. } => {
                    builder.set_event_context((*id).into(), *context);
                }
                Event::CondaSolveStarted { id } => {
                    let node = builder.alloc_node(format!("Conda solve #{}", id.0));
                    builder.set_context_node((*id).into(), node);
                }
                Event::CondaSolveFinished { .. } => {}
                Event::PixiSolveQueued { id, context, spec } => {
                    pixi_solve_name.insert(
                        *id,
                        spec.dependencies
                            .names()
                            .map(PackageName::as_source)
                            .format(", ")
                            .to_string(),
                    );
                    builder.set_event_context((*id).into(), *context);
                }
                Event::PixiSolveStarted { id } => {
                    let node = builder
                        .alloc_node(format!("Pixi solve ({})", pixi_solve_name.get(id).unwrap()));
                    builder.set_context_node((*id).into(), node);
                }
                Event::PixiSolveFinished { .. } => {}
                Event::PixiInstallQueued { id, context, .. } => {
                    builder.set_event_context((*id).into(), *context);
                }
                Event::PixiInstallStarted { id } => {
                    let node = builder.alloc_node(format!("Pixi install #{}", id.0));
                    builder.set_context_node((*id).into(), node);
                }
                Event::PixiInstallFinished { .. } => {}
                Event::GitCheckoutQueued {
                    id,
                    context,
                    reference,
                } => {
                    checkout_name.insert(
                        *id,
                        format!("{}@{}", reference.url.as_url(), reference.reference),
                    );
                    builder.set_event_context((*id).into(), *context);
                }
                Event::GitCheckoutStarted { id } => {
                    let node = builder
                        .alloc_node(format!("Git Checkout ({})", checkout_name.get(id).unwrap()));
                    builder.set_context_node((*id).into(), node);
                }
                Event::GitCheckoutFinished { .. } => {}
                Event::SourceMetadataQueued { id, context, spec } => {
                    source_metadata_name.insert(*id, spec.source_spec.to_toml_value().to_string());
                    builder.set_event_context((*id).into(), *context);
                }
                Event::SourceMetadataStarted { id } => {
                    let node = builder.alloc_node(format!(
                        "Source metadata ({})",
                        source_metadata_name.get(id).unwrap()
                    ));
                    builder.set_context_node((*id).into(), node);
                }
                Event::SourceMetadataFinished { .. } => {}
                Event::InstantiateToolEnvQueued { id, context, spec } => {
                    instantiate_tool_env_name
                        .insert(*id, spec.requirement.0.as_source().to_string());
                    builder.set_event_context((*id).into(), *context);
                }
                Event::InstantiateToolEnvStarted { id } => {
                    let node = builder.alloc_node(format!(
                        "Instantiate tool environment ({})",
                        instantiate_tool_env_name.get(id).unwrap()
                    ));
                    builder.set_context_node((*id).into(), node);
                }
                Event::InstantiateToolEnvFinished { .. } => {}
            }
        }

        builder.finish()
    }
}

impl Display for EventTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn make_node(nodes: &slotmap::SlotMap<NodeId, Node>, id: NodeId) -> StringTreeNode {
            let node = &nodes[id];
            let mut tree = StringTreeNode::new(node.label.clone());
            for child in &node.children {
                tree.push_node(make_node(nodes, *child))
            }
            tree
        }

        let format = TreeFormatting::dir_tree(FormatCharacters::box_chars());
        for root in &self.rootes {
            write!(
                f,
                "{}",
                make_node(&self.nodes, *root)
                    .to_string_with_format(&format)
                    .unwrap()
            )?;
        }

        Ok(())
    }
}

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
    InstantiateToolEnv(InstantiateToolEnvId),
}

impl From<ReporterContext> for EventId {
    fn from(context: ReporterContext) -> Self {
        match context {
            ReporterContext::SolvePixi(id) => Self::PixiSolve(id),
            ReporterContext::SolveConda(id) => Self::CondaSolve(id),
            ReporterContext::InstallPixi(id) => Self::PixiInstall(id),
            ReporterContext::SourceMetadata(id) => Self::SourceMetadata(id),
            ReporterContext::InstantiateToolEnv(id) => Self::InstantiateToolEnv(id),
        }
    }
}

impl EventTreeBuilder {
    fn new() -> Self {
        Self {
            nodes: SlotMap::default(),
            rootes: Vec::new(),
            event_parent_nodes: HashMap::new(),
            event_nodes: Default::default(),
        }
    }

    fn alloc_node(&mut self, label: String) -> NodeId {
        self.nodes.insert(Node {
            label,
            children: Vec::new(),
        })
    }

    fn set_event_context(&mut self, id: EventId, context: Option<ReporterContext>) {
        if let Some(context) = context
            .and_then(|context| self.event_nodes.get(&context.into()))
            .copied()
        {
            self.event_parent_nodes.insert(id, context);
        }
    }

    fn set_context_node(&mut self, id: EventId, node: NodeId) {
        self.event_nodes.insert(id, node);
        if let Some(parent) = self.event_parent_nodes.get(&id) {
            self.nodes[*parent].children.push(node);
        } else {
            self.rootes.push(node);
        }
    }

    fn finish(self) -> EventTree {
        EventTree {
            rootes: self.rootes,
            nodes: self.nodes,
        }
    }
}
