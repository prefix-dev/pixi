use background::DispatcherBackgroundTask;
use pixi_record::PixiRecord;
use rattler_repodata_gateway::Gateway;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::{CondaEnvironmentSpec, SolveCondaEnvironmentError};

mod background;

/// The dispatcher is responsible for synchronizing requests between different
/// conda environments.
pub struct Dispatcher {
    channel: DispatchChannel,
    context: Option<DispatcherContext>,
    inner: Arc<DispatchInner>,
}

struct DispatchInner {
    /// The gateway to use to query conda repodata.
    gateway: rattler_repodata_gateway::Gateway,
}

/// A channel through which to send any messages to the dispatcher. Some
/// dispatchers are constructed by the dispatcher itself. To avoid a
/// cyclic dependency, these "sub"-dispatchers use a weak reference to the
/// sender.
enum DispatchChannel {
    Strong(mpsc::UnboundedSender<DispatchMessage>),
    Weak(mpsc::WeakUnboundedSender<DispatchMessage>),
}

impl DispatchChannel {
    /// Returns an owned channel that can be used to send messages to the
    /// background task, or `None` if the background task has been dropped.
    pub fn sender(&self) -> Option<mpsc::UnboundedSender<DispatchMessage>> {
        match self {
            DispatchChannel::Strong(sender) => Some(sender.clone()),
            DispatchChannel::Weak(sender) => sender.upgrade(),
        }
    }
}

/// The context in which this particular dispatcher is running. This is used to
/// track dependencies.
#[derive(Debug, Copy, Clone)]
enum DispatcherContext {
    SolveCondaEnvironment(SolveCondaEnvironmentId),
}

slotmap::new_key_type! {
    /// An id that unique identifies a conda environment that is being solved.
    struct SolveCondaEnvironmentId;
}

/// Wraps an error that might have occurred during the processing of a task.
#[derive(Debug, Clone, Error)]
pub enum DispatchError<E> {
    Cancelled,

    #[error(transparent)]
    Failed(#[from] E),
}

/// A message send to the dispatch task.
enum DispatchMessage {
    SolveCondaEnvironment(SolveCondaEnvironmentTask),
}

/// A message that is send to the background task to start solving a particular
/// conda environment.
struct SolveCondaEnvironmentTask {
    env: CondaEnvironmentSpec,
    context: Option<DispatcherContext>,
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolveCondaEnvironmentError>>,
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Dispatcher {
    /// Constructs a new default constructed instance.
    pub fn new() -> Self {
        Self::builder().finish()
    }

    /// Constructs a new builder for the dispatcher.
    pub fn builder() -> DispatchBuilder {
        DispatchBuilder::default()
    }

    /// Returns the gateway used to query conda repodata.
    pub fn gateway(&self) -> &Gateway {
        &self.inner.gateway
    }

    /// Solves a particular requirement.
    pub async fn solve_conda_environment(
        &self,
        env: CondaEnvironmentSpec,
    ) -> Result<Vec<PixiRecord>, DispatchError<SolveCondaEnvironmentError>> {
        let Some(sender) = self.channel.sender() else {
            // If this fails, it means the dispatcher was dropped and the task is
            // immediately canceled.
            return Err(DispatchError::Cancelled);
        };

        let (tx, rx) = oneshot::channel();
        sender
            .send(DispatchMessage::SolveCondaEnvironment(
                SolveCondaEnvironmentTask {
                    env,
                    context: self.context,
                    tx,
                },
            ))
            .map_err(|_| DispatchError::Cancelled)?;
        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(DispatchError::Failed(err)),
            Err(_) => Err(DispatchError::Cancelled),
        }
    }
}

#[derive(Default)]
pub struct DispatchBuilder {
    gateway: Option<rattler_repodata_gateway::Gateway>,
}

impl DispatchBuilder {
    pub fn finish(self) -> Dispatcher {
        let gateway = self.gateway.unwrap_or_default();

        let inner = Arc::new(DispatchInner { gateway });

        let sender = DispatcherBackgroundTask::spawn(inner.clone());
        Dispatcher {
            channel: DispatchChannel::Strong(sender),
            context: None,
            inner,
        }
    }
}
