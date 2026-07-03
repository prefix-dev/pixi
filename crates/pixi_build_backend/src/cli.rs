use std::sync::{Arc, atomic::AtomicBool};

use clap::{Parser, Subcommand};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use miette::IntoDiagnostic;
use pixi_build_types::{
    BackendCapabilities, FrontendCapabilities,
    procedures::log_message::{LOG_LEVEL_ENV, LogMessage},
    procedures::negotiate_capabilities::NegotiateCapabilitiesParams,
};
use rattler_build_core::console_utils::{LoggingOutputHandler, get_default_env_filter};
use tokio::sync::mpsc;
use tracing::Level;
use tracing_subscriber::{Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{log_message_layer::LogMessageLayer, protocol::ProtocolInstantiator, server::Server};

#[allow(missing_docs)]
#[derive(Parser)]
pub struct App {
    /// The subcommand to run.
    #[clap(subcommand)]
    command: Option<Commands>,

    /// The port to expose the json-rpc server on. If not specified will
    /// communicate with stdin/stdout.
    #[clap(long)]
    http_port: Option<u16>,

    /// Enable verbose logging.
    #[command(flatten)]
    verbose: Verbosity<InfoLevel>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Get the capabilities of the backend.
    Capabilities,
}

/// The log channel handed to the server when it runs over stdio.
type LogMessageChannel = (mpsc::UnboundedReceiver<LogMessage>, Arc<AtomicBool>);

/// Run the sever on the specified port or over stdin/stdout.
async fn run_server<T: ProtocolInstantiator>(
    port: Option<u16>,
    protocol: T,
    log_messages: Option<LogMessageChannel>,
) -> miette::Result<()> {
    let mut server = Server::new(protocol);
    if let Some(port) = port {
        server.run_over_http(port)
    } else {
        if let Some((receiver, enabled)) = log_messages {
            server = server.with_log_messages(receiver, enabled);
        }
        // running over stdin/stdout
        server.run().await
    }
}

/// The actual implementation of the main function that runs the CLI.
pub(crate) async fn main_impl<T: ProtocolInstantiator, F: FnOnce(LoggingOutputHandler) -> T>(
    factory: F,
    args: App,
) -> miette::Result<()> {
    // Setup logging
    let log_handler = LoggingOutputHandler::default();

    // The frontend can ask for more verbose logging than our own command line
    // requested; take the maximum of the two.
    let frontend_level = std::env::var(LOG_LEVEL_ENV)
        .ok()
        .and_then(|level| level.parse::<clap_verbosity_flag::log::LevelFilter>().ok());
    let cli_level = args.verbose.log_level_filter();
    let effective_level = frontend_level.map_or(cli_level, |level| level.max(cli_level));

    // `get_default_env_filter` only enables `rattler_build` and friends, which
    // silently drops events from the backend crates themselves (e.g. the
    // "`pypi-conda-map` is set but the mapping is disabled" warning). Add a
    // default directive so warnings from any target are surfaced — raised
    // further when the frontend asked for more.
    let default_directive = log_to_tracing_level_filter(effective_level)
        .max(tracing_subscriber::filter::LevelFilter::WARN);
    let registry = tracing_subscriber::registry().with(
        get_default_env_filter(effective_level)
            .into_diagnostic()?
            .add_directive(default_directive.into()),
    );

    // When we serve a frontend over stdio, structured log records can travel
    // to it as `log/message` notifications. The layer stays dormant until
    // the frontend advertises support during capability negotiation; until
    // then (and for INFO events — the plaintext build-output stream — always)
    // the human-readable handler keeps rendering to stderr.
    let serves_stdio = args.command.is_none() && args.http_port.is_none();
    let log_messages = if serves_stdio {
        let (sender, receiver) = mpsc::unbounded_channel();
        let enabled = Arc::new(AtomicBool::new(false));

        let stderr_enabled = enabled.clone();
        let stderr_filter = filter_fn(move |metadata| {
            metadata.is_span()
                || *metadata.level() == Level::INFO
                || !stderr_enabled.load(std::sync::atomic::Ordering::Acquire)
        });
        registry
            .with(log_handler.clone().with_filter(stderr_filter))
            .with(LogMessageLayer::new(sender, enabled.clone()))
            .init();
        Some((receiver, enabled))
    } else {
        registry.with(log_handler.clone()).init();
        None
    };

    let factory = factory(log_handler);

    match args.command {
        None => run_server(args.http_port, factory, log_messages).await,
        Some(Commands::Capabilities) => {
            let backend_capabilities = capabilities::<T>().await?;
            eprintln!(
                "Supports {}: {}",
                pixi_build_types::procedures::conda_outputs::METHOD_NAME,
                backend_capabilities.provides_conda_outputs()
            );
            eprintln!(
                "Supports {}: {}",
                pixi_build_types::procedures::conda_build_v1::METHOD_NAME,
                backend_capabilities.provides_conda_build_v1()
            );
            Ok(())
        }
    }
}

fn log_to_tracing_level_filter(
    level: clap_verbosity_flag::log::LevelFilter,
) -> tracing_subscriber::filter::LevelFilter {
    use clap_verbosity_flag::log::LevelFilter as LogLevelFilter;
    use tracing_subscriber::filter::LevelFilter;
    match level {
        LogLevelFilter::Off => LevelFilter::OFF,
        LogLevelFilter::Error => LevelFilter::ERROR,
        LogLevelFilter::Warn => LevelFilter::WARN,
        LogLevelFilter::Info => LevelFilter::INFO,
        LogLevelFilter::Debug => LevelFilter::DEBUG,
        LogLevelFilter::Trace => LevelFilter::TRACE,
    }
}

/// The entry point for the CLI which should be called from the backends implementation.
pub async fn main<T: ProtocolInstantiator, F: FnOnce(LoggingOutputHandler) -> T>(
    factory: F,
) -> miette::Result<()> {
    let args = App::parse();
    main_impl(factory, args).await
}

/// The entry point for the CLI which should be called from the backends implementation.
pub async fn main_ext<T: ProtocolInstantiator, F: FnOnce(LoggingOutputHandler) -> T>(
    factory: F,
    args: Vec<String>,
) -> miette::Result<()> {
    let args = App::parse_from(args);
    main_impl(factory, args).await
}

/// Returns the capabilities of the backend.
async fn capabilities<Factory: ProtocolInstantiator>() -> miette::Result<BackendCapabilities> {
    let result = Factory::negotiate_capabilities(NegotiateCapabilitiesParams {
        capabilities: FrontendCapabilities::default(),
    })
    .await?;

    Ok(result.capabilities)
}
