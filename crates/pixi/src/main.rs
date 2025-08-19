// This forces the crate to be compiled even though the crate is not used in the
// project. https://github.com/rust-lang/rust/issues/64402
#[cfg(feature = "pixi_allocator")]
extern crate pixi_allocator;

pub fn main() -> miette::Result<()> {
    // We often run out of stack space when running the CLI. This is especially an
    // issue for debug builds.
    //
    // Non-main threads should all have 2MB, as Rust forces platform consistency.
    // The default can be overridden with the RUST_MIN_STACK environment variable if
    // you need more.
    //
    // However, the real issue is the main thread. There is a large variety here
    // across platforms and it's harder to control (which is why Rust doesn't
    // normalize is by default). Notably on macOS and Linux you will typically get
    // 8MB for the main thread, while on Windows we only get 1MB, which is tiny in
    // comparison:
    // https://learn.microsoft.com/en-us/cpp/build/reference/stack-stack-allocations?view=msvc-170
    //
    // To normalize this we spawn an additional thread called `main2` with a size we
    // can set ourselves. 2MB tends to be too small (especially for debug builds).
    // 4MB seems fine. The code also tries to respect RUST_MIN_STACK if it is set,
    // which allows overriding the defaults. We don't allow stack sizes smaller than
    // 4MB to avoid misconfiguration since we know we use quite a bit of stack
    // space.
    let main_stack_size = std::env::var("RUST_MIN_STACK")
        .ok()
        .and_then(|var| var.parse::<usize>().ok())
        .unwrap_or(0)
        .max(4 * 1024 * 1024);

    let main2 = move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed building the Runtime");

        // Box the large main future to avoid stack overflows.
        runtime.block_on(Box::pin(pixi_cli::execute()))
    };

    std::thread::Builder::new()
        .name("main2".to_string())
        .stack_size(main_stack_size)
        .spawn(main2)
        .expect("Tokio executor failed, was there a panic?")
        .join()
        .expect("Tokio executor failed, was there a panic?")
}
