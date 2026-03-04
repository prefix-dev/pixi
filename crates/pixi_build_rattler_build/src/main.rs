mod config;
mod protocol;
mod rattler_build;

use protocol::RattlerBuildBackendInstantiator;

#[tokio::main]
pub async fn main() {
    if let Err(err) = pixi_build_backend::cli::main(RattlerBuildBackendInstantiator::new).await {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}
