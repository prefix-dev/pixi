use pixi_core::{Workspace, environment::LockFileUsage};

use crate::{init::InitOptions, interface::Interface, reinstall::ReinstallOptions};

pub struct ApiContext<I: Interface> {
    interface: I,
}

impl<I: Interface> ApiContext<I> {
    pub fn new(interface: I) -> Self {
        Self { interface }
    }

    pub async fn init(&self, options: InitOptions) -> miette::Result<()> {
        crate::init::init(&self.interface, options).await
    }

    pub async fn reinstall(
        &self,
        options: ReinstallOptions,
        workspace: Workspace,
        lock_file_usage: LockFileUsage,
    ) -> miette::Result<()> {
        crate::reinstall::reinstall(&self.interface, options, workspace, lock_file_usage).await
    }
}
