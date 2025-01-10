// This forces the crate to be compiled even though the crate is not used in the project.
// https://github.com/rust-lang/rust/issues/64402
#[cfg(feature = "pixi_allocator")]
extern crate pixi_allocator;

#[tokio::main]
pub async fn main() -> miette::Result<()> {
    pixi::cli::execute().await
}
