use pixi_core::{Workspace, environment::LockFileUsage};

use crate::interface::Interface;
use crate::workspace::{init::InitOptions, reinstall::ReinstallOptions};

pub struct WorkspaceContext<I: Interface> {
    interface: I,
    workspace: Workspace,
}

impl<I: Interface> WorkspaceContext<I> {
    pub fn new(interface: I, workspace: Workspace) -> Self {
        Self {
            interface,
            workspace,
        }
    }

    pub async fn init(interface: I, options: InitOptions) -> miette::Result<Workspace> {
        crate::workspace::init::init(&interface, options).await
    }

    pub async fn name(&self) -> String {
        crate::workspace::config::name::get(self.workspace.clone()).await
    }

    pub async fn set_name(&self, name: &str) -> miette::Result<()> {
        crate::workspace::config::name::set(&self.interface, self.workspace.clone(), name).await
    }

    pub async fn reinstall(
        &self,
        options: ReinstallOptions,
        lock_file_usage: LockFileUsage,
    ) -> miette::Result<()> {
        crate::workspace::reinstall::reinstall(
            &self.interface,
            options,
            self.workspace.clone(),
            lock_file_usage,
        )
        .await
    }
}
