//! Error-quality coverage for native `pin-compatible` support in
//! package manifests (issue #6114).
//!
//! The actionable hint must live in the error message itself: the error
//! reaches the user nested behind
//! `SolvePixiEnvironmentError::ResolveSourcePackage` via
//! `#[diagnostic_source]`, and miette's graphical handler renders a
//! `#[help]` only for the top-level diagnostic.

use pixi_build_backend_passthrough::PassthroughBackend;
use pixi_build_frontend::BackendOverride;
use pixi_command_dispatcher::{
    BuildEnvironment, CommandDispatcher, Executor, keys::SolvePixiEnvironmentSpec,
};
use pixi_spec::PathSpec;
use pixi_spec_containers::DependencyMap;
use pixi_test_utils::format_diagnostic;
use rattler_conda_types::Platform;

use crate::{
    default_cache_dirs, empty_pixi_env_spec, env_ref_of, run_pixi_solve, test_tempdir, to_abs_dir,
    tool_platform, workspaces_dir,
};

/// A `pin-compatible` run dependency that names a package which is in
/// neither the build nor the host environment must fail the solve with
/// the dedicated error and its actionable help text.
#[tokio::test]
pub async fn test_pin_compatible_missing_package_reports_helpful_error() {
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let root_dir = workspaces_dir().join("pin-adv-missing");
    let tempdir = test_tempdir();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages)
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();

    let error = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package_a".parse().unwrap(),
                PathSpec {
                    path: "package_a".into(),
                }
                .into(),
            )]),
            env_ref: env_ref_of(vec![], BuildEnvironment::simple(Platform::Linux64, vec![])),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .expect_err("a pin against a package missing from the previous env must fail");

    let rendered = format_diagnostic(&error);
    assert!(
        rendered.contains(
            "Could not apply pin_compatible. Package 'some-lib' is not in the compatibility \
             environment"
        ),
        "the error must name the missing package, got:\n{rendered}"
    );
    assert!(
        rendered.contains("add 'some-lib' to `[package.host-dependencies]`"),
        "the error must carry the actionable hint, got:\n{rendered}"
    );
}
