use clap::{Parser, Subcommand};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use miette::IntoDiagnostic;
use pixi_build_types::{
    BackendCapabilities, FrontendCapabilities, log::BACKEND_LOG_SOCKET_ENV,
    procedures::negotiate_capabilities::NegotiateCapabilitiesParams,
};
use rattler_build_core::console_utils::{LoggingOutputHandler, get_default_env_filter};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    json_log_layer::JsonLogLayer, log_channel::connect_and_spawn,
    protocol::ProtocolInstantiator, server::Server,
};

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

/// Run the sever on the specified port or over stdin/stdout.
async fn run_server<T: ProtocolInstantiator>(port: Option<u16>, protocol: T) -> miette::Result<()> {
    let server = Server::new(protocol);
    if let Some(port) = port {
        server.run_over_http(port)
    } else {
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

    // `get_default_env_filter` only enables `rattler_build` and friends, which
    // silently drops events from the backend crates themselves (e.g. the
    // "`pypi-conda-map` is set but the mapping is disabled" warning). Add a
    // default directive so warnings from any target are surfaced.
    let registry = tracing_subscriber::registry().with(
        get_default_env_filter(args.verbose.log_level_filter())
            .into_diagnostic()?
            .add_directive(tracing_subscriber::filter::LevelFilter::WARN.into()),
    );

    // If the frontend set PIXI_BUILD_BACKEND_LOG_SOCKET, connect to it and
    // route structured tracing data (non-INFO events plus span lifecycle)
    // through that channel. INFO events stay on stderr via the
    // LoggingOutputHandler — rattler-build uses INFO as its plaintext
    // build-output channel, and the frontend reads it as raw stderr.
    //
    // Standalone invocations (no env var, no socket) keep the unfiltered
    // human-readable handler so dev runs behave as before.
    let log_socket = std::env::var(BACKEND_LOG_SOCKET_ENV).ok();
    if let Some(addr) = log_socket {
        let sender = connect_and_spawn(&addr).await;
        match sender {
            Some(sender) => {
                let info_only = tracing_subscriber::filter::filter_fn(|m| {
                    *m.level() == tracing::Level::INFO
                });
                registry
                    .with(log_handler.clone().with_filter(info_only))
                    .with(JsonLogLayer::new(sender))
                    .init();
            }
            None => {
                // Couldn't connect — fall back to the standalone layout so
                // we still produce useful output to stderr.
                registry.with(log_handler.clone()).init();
            }
        }
    } else {
        registry.with(log_handler.clone()).init();
    }

    let factory = factory(log_handler);

    match args.command {
        None => run_server(args.http_port, factory).await,
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
        capabilities: FrontendCapabilities {},
    })
    .await?;

    Ok(result.capabilities)
}
