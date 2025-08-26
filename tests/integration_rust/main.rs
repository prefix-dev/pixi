use std::sync::Once;

mod add_tests;
mod build_tests;
mod common;
mod init_tests;
mod install_filter_tests;
mod install_tests;
mod project_tests;
mod pypi_tests;
mod search_tests;
mod solve_group_tests;
mod task_tests;
mod test_activation;
mod update_tests;
mod upgrade_tests;

/// Setup tracing for the test suite.
/// This function initializes the tracing subscriber with the environment
/// filter, enabling detailed logging for tests. It uses a `Once` to ensure that
/// the setup is performed only once, even if called multiple times during the
/// test run.
pub fn setup_tracing() {
    static TRACING_INIT: Once = Once::new();
    TRACING_INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_line_number(true)
            .with_file(true)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .init();
    });
}
