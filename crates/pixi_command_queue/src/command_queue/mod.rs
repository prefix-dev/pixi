use processor::CommandQueueProcessor;
use pixi_record::PixiRecord;
use rattler_repodata_gateway::Gateway;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::{CondaEnvironmentSpec, SolveCondaEnvironmentError};
use crate::reporter::Reporter;

mod processor;

/// The dispatcher is responsible for synchronizing requests between different
/// conda environments.
pub struct CommandQueue {
    channel: CommandQueueChannel,
    context: Option<CommandQueueContext>,
    data: Arc<CommandQueueData>,
}

struct CommandQueueData {
    /// The gateway to use to query conda repodata.
    gateway: Gateway,
}

/// A channel through which to send any messages to the dispatcher. Some
/// dispatchers are constructed by the dispatcher itself. To avoid a
/// cyclic dependency, these "sub"-dispatchers use a weak reference to the
/// sender.
enum CommandQueueChannel {
    Strong(mpsc::UnboundedSender<ForegroundMessage>),
    Weak(mpsc::WeakUnboundedSender<ForegroundMessage>),
}

impl CommandQueueChannel {
    /// Returns an owned channel that can be used to send messages to the
    /// background task, or `None` if the background task has been dropped.
    pub fn sender(&self) -> Option<mpsc::UnboundedSender<ForegroundMessage>> {
        match self {
            CommandQueueChannel::Strong(sender) => Some(sender.clone()),
            CommandQueueChannel::Weak(sender) => sender.upgrade(),
        }
    }
}

/// The context in which this particular dispatcher is running. This is used to
/// track dependencies.
#[derive(Debug, Copy, Clone)]
enum CommandQueueContext {
    SolveCondaEnvironment(SolveCondaEnvironmentId),
}

slotmap::new_key_type! {
    /// An id that unique identifies a conda environment that is being solved.
    struct SolveCondaEnvironmentId;
}

/// Wraps an error that might have occurred during the processing of a task.
#[derive(Debug, Clone, Error)]
pub enum CommandQueueError<E> {
    Cancelled,

    #[error(transparent)]
    Failed(#[from] E),
}

/// A message send to the dispatch task.
enum ForegroundMessage {
    SolveCondaEnvironment(SolveCondaEnvironmentTask),
}

/// A message that is send to the background task to start solving a particular
/// conda environment.
struct SolveCondaEnvironmentTask {
    env: CondaEnvironmentSpec,
    context: Option<CommandQueueContext>,
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolveCondaEnvironmentError>>,
}

impl Default for CommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandQueue {
    /// Constructs a new default constructed instance.
    pub fn new() -> Self {
        Self::builder().finish()
    }

    /// Constructs a new builder for the dispatcher.
    pub fn builder() -> CommandQueueBuilder {
        CommandQueueBuilder::default()
    }

    /// Returns the gateway used to query conda repodata.
    pub fn gateway(&self) -> &Gateway {
        &self.data.gateway
    }

    /// Solves a particular requirement.
    pub async fn solve_conda_environment(
        &self,
        env: CondaEnvironmentSpec,
    ) -> Result<Vec<PixiRecord>, CommandQueueError<SolveCondaEnvironmentError>> {
        let Some(sender) = self.channel.sender() else {
            // If this fails, it means the dispatcher was dropped and the task is
            // immediately canceled.
            return Err(CommandQueueError::Cancelled);
        };

        let (tx, rx) = oneshot::channel();
        sender
            .send(ForegroundMessage::SolveCondaEnvironment(
                SolveCondaEnvironmentTask {
                    env,
                    context: self.context,
                    tx,
                },
            ))
            .map_err(|_| CommandQueueError::Cancelled)?;
        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(CommandQueueError::Failed(err)),
            Err(_) => Err(CommandQueueError::Cancelled),
        }
    }
}

#[derive(Default)]
pub struct CommandQueueBuilder {
    gateway: Option<Gateway>,
    reporter: Option<Box<dyn Reporter>>,
}

impl CommandQueueBuilder {
    /// Sets the reporter used by the [`CommandQueue`] to report progress.
    pub fn with_reporter<F: Reporter + 'static>(self, reporter: F) -> Self {
        Self {
            reporter: Some(Box::new(reporter)),
            ..self
        }
    }

    /// Finish building the [`CommandQueue`] and return it.
    pub fn finish(self) -> CommandQueue {
        let gateway = self.gateway.unwrap_or_default();

        let data = Arc::new(CommandQueueData { gateway });

        let sender = CommandQueueProcessor::spawn(data.clone(), self.reporter);
        CommandQueue {
            channel: CommandQueueChannel::Strong(sender),
            context: None,
            data,
        }
    }
}
