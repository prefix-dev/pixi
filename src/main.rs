pub fn main() -> miette::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed building the Runtime");

    // Box the large main future to avoid stack overflows.
    let result = runtime.block_on(Box::pin(pixi::cli::execute()));

    // Avoid waiting for pending tasks to complete.
    runtime.shutdown_background();

    result
}
