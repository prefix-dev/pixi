/// Reads an environment variable that has both a current `WORKSPACE` name and a
/// legacy `PROJECT` name (e.g. `PIXI_WORKSPACE_ROOT` / `PIXI_PROJECT_ROOT`).
///
/// The `WORKSPACE` variant takes precedence. If both are set to different
/// values, a warning is emitted and the `WORKSPACE` value is used.
pub fn workspace_or_project_env(workspace_var: &str, project_var: &str) -> Option<String> {
    let workspace = std::env::var(workspace_var).ok();
    let project = std::env::var(project_var).ok();

    if let (Some(workspace), Some(project)) = (&workspace, &project)
        && workspace != project
    {
        tracing::warn!(
            "Both `{workspace_var}` and `{project_var}` are set to different values; using `{workspace_var}`"
        );
    }

    workspace.or(project)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn prefers_workspace_when_both_set_differently() {
        temp_env::with_vars(
            [
                ("PIXI_TEST_WORKSPACE_ROOT", Some("/ws")),
                ("PIXI_TEST_PROJECT_ROOT", Some("/proj")),
            ],
            || {
                assert_eq!(
                    workspace_or_project_env("PIXI_TEST_WORKSPACE_ROOT", "PIXI_TEST_PROJECT_ROOT"),
                    Some("/ws".to_string())
                );
            },
        );
    }

    #[test]
    fn falls_back_to_project() {
        temp_env::with_vars(
            [
                ("PIXI_TEST_WORKSPACE_ROOT", None::<&str>),
                ("PIXI_TEST_PROJECT_ROOT", Some("/proj")),
            ],
            || {
                assert_eq!(
                    workspace_or_project_env("PIXI_TEST_WORKSPACE_ROOT", "PIXI_TEST_PROJECT_ROOT"),
                    Some("/proj".to_string())
                );
            },
        );
    }
}
