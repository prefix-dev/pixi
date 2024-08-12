use crate::{protocol::Protocol, tool::Tool, Metadata};

/// A statful object to communicate with a build backend and perform tasks.
#[derive(Debug)]
pub struct Builder {
    /// The protocol to communicate with the backend tool.
    protocol: Protocol,

    /// The tool to use to build the package.
    tool: Tool,
}

impl Builder {
    /// Construct a new build from a protocol and backend tool.
    pub(crate) fn new(protocol: Protocol, tool: Tool) -> Self {
        Self { protocol, tool }
    }

    /// Builds the package by invoking the tool and communicating with it
    /// through the backend protocol.
    pub fn get_metadata(&self) -> miette::Result<Metadata> {
        self.protocol.get_metadata(&self.tool)
    }
}
