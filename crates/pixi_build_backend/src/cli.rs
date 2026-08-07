use clap::{Parser, Subcommand};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use miette::IntoDiagnostic;
use pixi_build_types::{
    BackendCapabilities, FrontendCapabilities,
    procedures::negotiate_capabilities::NegotiateCapabilitiesParams,
};
use rattler_build_core::console_utils::{LoggingOutputHandler, get_default_env_filter};
use tracing_subscriber::{
    Layer, filter::dynamic_filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};

use crate::{logging::LogForwarder, protocol::ProtocolInstantiator, server::Server, stdio};

#[allow(missing_docs)]
#[derive(Parser)]
pub struct App {
    /// The subcommand to run.
    #[clap(subcommand)]
    command: Option<Commands>,

    /// Enable verbose logging.
    #[command(flatten)]
    verbose: Verbosity<InfoLevel>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Get the capabilities of the backend.
    Capabilities,
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

    // The outgoing side of the connection has to exist before the subscriber is
    // installed, because the forwarding layer writes into it.
    let (sender, incoming) = stdio::channel();
    let (log_forwarder, log_forwarding) = LogForwarder::new(sender);

    // Exactly one of these two layers is live: the frontend either receives log
    // events as notifications or scrapes them off stderr, never both. The
    // switch flips during capability negotiation, so the stderr side needs a
    // filter that is re-evaluated per event rather than cached per callsite.
    let stderr_logging = log_forwarding.clone();
    registry
        .with(log_forwarder)
        .with(
            log_handler
                .clone()
                .with_filter(dynamic_filter_fn(move |_metadata, _ctx| {
                    !stderr_logging.is_enabled()
                })),
        )
        .init();

    let factory = factory(log_handler);

    match args.command {
        None => Server::new(factory, log_forwarding).run(incoming).await,
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
        capabilities: FrontendCapabilities::default(),
    })
    .await?;

    Ok(result.capabilities)
}
