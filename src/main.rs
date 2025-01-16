// This forces the crate to be compiled even though the crate is not used in the project.
// https://github.com/rust-lang/rust/issues/64402
#[cfg(feature = "pixi_allocator")]
extern crate pixi_allocator;

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
