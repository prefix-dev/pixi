use miette::Diagnostic;
use pixi_core::{
    Workspace,
    workspace::{Environment, virtual_packages::verify_current_platform_can_run_environment},
};
use pixi_manifest::{Task, TaskName};
use rattler_conda_types::Platform;
use thiserror::Error;

use crate::error::{AmbiguousTaskError, MissingTaskError};

/// The separator used between member path segments and the task name in
/// qualified task addresses, e.g. `member_a::build` or `a::c::test`.
pub const MEMBER_TASK_SEPARATOR: &str = "::";

/// Splits a task name on [`MEMBER_TASK_SEPARATOR`]. Returns `Some((member_path,
/// task_name))` when the input contains at least one separator, otherwise
/// `None`.
///
/// Examples:
/// - `"build"` → `None`
/// - `"a::build"` → `Some((vec!["a"], "build"))`
/// - `"a::c::test"` → `Some((vec!["a", "c"], "test"))`
pub fn parse_qualified_task_name(s: &str) -> Option<(Vec<&str>, &str)> {
    if !s.contains(MEMBER_TASK_SEPARATOR) {
        return None;
    }
    let mut parts: Vec<&str> = s.split(MEMBER_TASK_SEPARATOR).collect();
    if parts.len() < 2 {
        return None;
    }
    let task = parts.pop().expect("checked len >= 2");
    Some((parts, task))
}

/// Defines where the task was defined when looking for a task.
#[derive(Debug, Clone)]
pub enum FindTaskSource<'p> {
    CmdArgs,
    DependsOn(TaskName, &'p Task),
}

pub type TaskAndEnvironment<'p> = (Environment<'p>, &'p Task);

pub trait TaskDisambiguation<'p> {
    fn disambiguate(&self, task: &AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>>;
}

#[derive(Default)]
pub struct NoDisambiguation;
pub struct DisambiguateFn<Fn>(Fn);

impl<'p> TaskDisambiguation<'p> for NoDisambiguation {
    fn disambiguate(&self, _task: &AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>> {
        None
    }
}

impl<'p, F: Fn(&AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>>> TaskDisambiguation<'p>
    for DisambiguateFn<F>
{
    fn disambiguate(&self, task: &AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>> {
        self.0(task)
    }
}

/// An object to help with searching for tasks.
pub struct SearchEnvironments<'p, D: TaskDisambiguation<'p> = NoDisambiguation> {
    pub project: &'p Workspace,
    pub explicit_environment: Option<Environment<'p>>,
    pub platform: Option<Platform>,
    pub disambiguate: D,
}

/// Information about an task that was found when searching for a task
pub struct AmbiguousTask<'p> {
    pub task_name: TaskName,
    pub depended_on_by: Option<(TaskName, &'p Task)>,
    pub environments: Vec<TaskAndEnvironment<'p>>,
}

impl<'p> From<AmbiguousTask<'p>> for AmbiguousTaskError {
    fn from(value: AmbiguousTask<'p>) -> Self {
        Self {
            task_name: value.task_name,
            environments: value
                .environments
                .into_iter()
                .map(|env| env.0.name().clone())
                .collect(),
        }
    }
}

#[derive(Debug, Diagnostic, Error)]
pub enum FindTaskError {
    #[error(transparent)]
    MissingTask(MissingTaskError),

    #[error(transparent)]
    AmbiguousTask(AmbiguousTaskError),
}

impl<'p> SearchEnvironments<'p, NoDisambiguation> {
    // Determine which environments we are allowed to check for tasks.
    //
    // If the user specified an environment, look for tasks in the main environment
    // and the user specified environment.
    //
    // If the user did not specify an environment, look for tasks in any
    // environment.
    pub fn from_opt_env(
        project: &'p Workspace,
        explicit_environment: Option<Environment<'p>>,
        platform: Option<Platform>,
    ) -> Self {
        Self {
            project,
            explicit_environment,
            platform,
            disambiguate: NoDisambiguation,
        }
    }
}

impl<'p, D: TaskDisambiguation<'p>> SearchEnvironments<'p, D> {
    /// Returns a new `SearchEnvironments` with the given disambiguation
    /// function.
    pub fn with_disambiguate_fn<F: Fn(&AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>>>(
        self,
        func: F,
    ) -> SearchEnvironments<'p, DisambiguateFn<F>> {
        SearchEnvironments {
            project: self.project,
            explicit_environment: self.explicit_environment,
            platform: self.platform,
            disambiguate: DisambiguateFn(func),
        }
    }

    /// Finds the task with the given name or returns an error that explains why
    /// the task could not be found.
    pub(crate) fn find_task(
        &self,
        name: TaskName,
        source: FindTaskSource<'p>,
        task_specific_environment: Option<Environment<'p>>,
    ) -> Result<TaskAndEnvironment<'p>, FindTaskError> {
        // `member::task` / `a::b::task` addressing for the hierarchical-tasks
        // preview feature (Model 2 — federated member workspaces).
        //
        // If the name contains `::` and the first segment matches a known
        // top-level member, we resolve the member path to the member's
        // standalone Workspace and dispatch the lookup into that member's
        // **own** default environment. The returned `Environment` carries
        // a reference to the member workspace — so downstream task
        // execution (activation, lockfile, install dir) naturally targets
        // the member, not the root.
        //
        // Names without `::`, or with a first segment that isn't a
        // member, fall through to the normal task search below —
        // preserving backwards compatibility for any task name that
        // happens to contain `::`.
        if let Some((member_path, task_name)) = parse_qualified_task_name(name.as_str())
            && self.project.members().contains_key(member_path[0])
            && let Some(member_ws) = self.project.resolve_member(member_path.iter().copied())
        {
            let member_env = member_ws.default_environment();
            let task_name_lookup = TaskName::from(task_name);
            match member_env.task(&task_name_lookup, self.platform) {
                Ok(task) => return Ok((member_env, task)),
                Err(_) => {
                    // Member path resolves but no task with that name.
                    // Surface a MissingTask error tied to the
                    // fully-qualified address so the user sees exactly
                    // what we searched for.
                    return Err(FindTaskError::MissingTask(MissingTaskError {
                        task_name: name,
                    }));
                }
            }
        }

        // If no explicit environment was specified
        if self.explicit_environment.is_none() && task_specific_environment.is_none() {
            let default_env = self.project.default_environment();
            // If the default environment has the task
            if let Ok(default_env_task) = default_env.task(&name, self.platform) {
                // If the task in the default environment declares a `default-environment`
                // and that environment exists and can run on this platform, prefer that
                // environment instead of returning the default environment.
                if let Some(default_env_name) = default_env_task.default_environment()
                    && let Some(env) = self
                        .project
                        .environments()
                        .into_iter()
                        .find(|e| e.name() == default_env_name)
                    && verify_current_platform_can_run_environment(&env, None).is_ok()
                    && let Ok(task_in_env) = env.task(&name, self.platform)
                {
                    return Ok((env.clone(), task_in_env));
                }
                // If no other environment has the task name but a different task, return the
                // default environment
                if !self
                    .project
                    .environments()
                    .iter()
                    // Filter out default environment
                    .filter(|env| !env.name().is_default())
                    // Filter out environments that can not run on this machine.
                    .filter(|env| verify_current_platform_can_run_environment(env, None).is_ok())
                    .any(|env| {
                        if let Ok(task) = env.task(&name, self.platform) {
                            // If the task exists in the environment but it is not the reference to
                            // the same task, return true to make it ambiguous
                            !std::ptr::eq(task, default_env_task)
                        } else {
                            // If the task does not exist in the environment, return false
                            false
                        }
                    })
                {
                    return Ok((self.project.default_environment(), default_env_task));
                }
            }
        }

        let environments = match (task_specific_environment, &self.explicit_environment) {
            (Some(task_specific_environment), _) => {
                // If a specific environment was specified in the dependency, only look for tasks in that
                // environment.
                vec![task_specific_environment]
            }
            (None, Some(explicit_environment)) => {
                // If an explicit environment was specified, only look for tasks in that
                // environment and the default environment.
                Vec::from([explicit_environment.clone()])
            }
            _ => {
                // If no specific environment was specified, look for tasks in all environments.
                self.project.environments()
            }
        };

        // Find all the task and environment combinations
        let mut tasks = Vec::new();
        for env in environments.iter() {
            if let Some(task) = env
                .tasks(self.platform)
                .ok()
                .and_then(|tasks| tasks.get(&name).copied())
            {
                tasks.push((env.clone(), task));
            }
        }

        match tasks.len() {
            0 => Err(FindTaskError::MissingTask(MissingTaskError {
                task_name: name,
            })),
            1 => {
                let (env, task) = tasks.remove(0);
                Ok((env.clone(), task))
            }
            _ => {
                let ambiguous_task = AmbiguousTask {
                    task_name: name,
                    depended_on_by: match source {
                        FindTaskSource::DependsOn(dep, task) => Some((dep, task)),
                        _ => None,
                    },
                    environments: tasks,
                };

                match self.disambiguate.disambiguate(&ambiguous_task) {
                    Some(env) => Ok(env),
                    None => Err(FindTaskError::AmbiguousTask(ambiguous_task.into())),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pixi_core::workspace::HasWorkspaceRef;

    use super::*;

    #[test]
    fn test_find_task_default_defined() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "win-64", "osx-64"]

            [tasks]
            test = "cargo test"
            [feature.test.dependencies]
            pytest = "*"
            [environments]
            test = ["test"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let env = project.default_environment();
        let search = SearchEnvironments::from_opt_env(&project, None, Some(env.best_platform()));
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(result.is_ok());
        assert!(result.unwrap().0.name().is_default());
    }

    #[test]
    fn test_find_task_dual_defined() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64", "linux-riscv64"]

            [tasks]
            test = "cargo test"

            [feature.test.tasks]
            test = "cargo test --all-features"

            [environments]
            test = ["test"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));
    }

    #[test]
    fn test_find_task_explicit_defined() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64", "linux-riscv64"]

            [tasks]
            test = "pytest"
            [feature.test.tasks]
            test = "pytest -s"
            [feature.prod.tasks]
            run = "python start.py"

            [environments]
            default = ["test"]
            test = ["test"]
            prod = ["prod"]

            [system-requirements]
            macos = "10.6"
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));

        // With explicit environment
        let search =
            SearchEnvironments::from_opt_env(&project, Some(project.default_environment()), None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(result.unwrap().0.name().is_default());
    }

    #[test]
    fn test_find_non_default_feature_task() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64"]

            [tasks]

            [feature.test.tasks]
            test = "pytest -s"
            [feature.prod.tasks]
            run = "python start.py"

            [environments]
            default = ["test"]
            test = ["test"]
            prod = ["prod"]

            [system-requirements]
            macos = "10.6"
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(result.unwrap().0.name().is_default());

        // With explicit environment
        let search = SearchEnvironments::from_opt_env(
            &project,
            Some(project.environment("prod").unwrap()),
            None,
        );
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(matches!(result, Err(FindTaskError::MissingTask(_))));
    }

    #[test]
    fn test_find_ambiguous_task() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64", "linux-riscv64"]

            [tasks]
            bla = "echo foo"

            [feature.other.tasks]
            bla = "echo foo"

            [environments]
            other = ["other"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("bla".into(), FindTaskSource::CmdArgs, None);
        // Ambiguous task because it is the same name and code but it is defined in
        // different environments
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));
    }

    #[test]
    fn test_default_environment_preferred_when_multiple_envs() {
        let manifest_str = r#"
            [workspace]
            channels = []
            platforms = ["linux-64", "win-64", "osx-64", "osx-arm64", "linux-aarch64"]

            [tasks]
            test = "echo test"
            test2 = "echo test2"
            dep.depends-on = ["test3", "test6"]

            [feature.test.tasks]
            test3 = { cmd = "echo test3", default-environment = "three" }
            test4 = "echo test4"

            [feature.test2.tasks]
            test5 = "echo test5"
            test6 = { cmd = "echo test6", default-environment = "four" }

            [environments]
            one = []
            two = ["test"]
            three = ["test2", "test"]
            four = ["test2"]
            five = ["test", "test2"]
            six = { features = ["test"], no-default-feature = true }
            seven = { features = ["test2"], no-default-feature = true }
        "#;

        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();

        // Build a SearchEnvironments that will prefer a candidate environment
        // whose task declares a `default-environment` matching the env name.
        let search = SearchEnvironments::from_opt_env(&project, None, None).with_disambiguate_fn(
            |amb: &AmbiguousTask| {
                amb.environments
                    .iter()
                    .find(|(env, task)| {
                        if let Some(default_env_name) = task.default_environment() {
                            default_env_name == env.name()
                        } else {
                            false
                        }
                    })
                    .cloned()
            },
        );

        // When resolving `test3` we expect the it to pick `three`.
        let result = search
            .find_task("test3".into(), FindTaskSource::CmdArgs, None)
            .expect("should pick default environment");
        assert_eq!(result.0.name().as_str(), "three");
    }

    #[test]
    fn test_explicit_environment_overrides_task_default_environment() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = []
            platforms = ["linux-64", "win-64", "osx-64", "osx-arm64", "linux-aarch64"]

            [feature.test.tasks]
            test3 = { cmd = "echo test3", default-environment = "three" }

            [feature.test2.tasks]
            test3 = "echo other"

            [environments]
            default = ["test"]
            two = ["test2"]
            three = ["test"]
        "#;

        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();

        // If the user explicitly requests `two`, that should be preferred even
        // though the task has a `default-environment` set to `three`.
        let explicit_env = project.environment("two").unwrap();
        let search = SearchEnvironments::from_opt_env(&project, Some(explicit_env), None);
        let result = search.find_task("test3".into(), FindTaskSource::CmdArgs, None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0.name().as_str(), "two");
    }

    #[test]
    fn test_top_level_task_default_environment_is_used() {
        let manifest_str = r#"
            [workspace]
            channels = []
            platforms = ["linux-64", "win-64", "osx-64", "osx-arm64", "linux-aarch64"]

            [tasks]
            test = { cmd = "echo test", default-environment = "test" }

            [feature.test.dependencies]

            [environments]
            test = ["test"]
        "#;

        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();

        // Build a SearchEnvironments that will apply default behavior.
        let search = SearchEnvironments::from_opt_env(&project, None, None);

        // Resolve `test` task; since the task declares `default-environment = "test"`
        // we expect the resolved environment to be `test` rather than the default.
        let result = search
            .find_task("test".into(), FindTaskSource::CmdArgs, None)
            .expect("should resolve to an environment");
        assert_eq!(result.0.name().as_str(), "test");
    }

    // ---- Hierarchical-tasks (`a::b::task`) end-to-end routing tests ----

    /// Writes a workspace root + member layout used by the hierarchical-tasks
    /// integration tests and returns the tempdir guard (drop = cleanup).
    ///
    /// Under Model 2 every member has its own `[workspace]` block — each
    /// is a fully standalone pixi project. The root's role is purely to
    /// aggregate so `a::c::test` resolves through the member tree.
    fn build_hierarchical_fixture(preview_on: bool) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let preview_line = if preview_on {
            "preview = [\"hierarchical-tasks\"]"
        } else {
            "preview = []"
        };
        std::fs::write(
            tmp.path().join("pixi.toml"),
            format!(
                "[workspace]\nname = \"ht-root\"\nchannels = []\nplatforms = [\"linux-64\", \"osx-64\", \"osx-arm64\", \"win-64\"]\n{preview_line}\n\n[tasks]\ngreet = \"echo hi\"\nall_tests = {{ depends-on = [\"a::test\", \"a::c::test\", \"b::test\"] }}\n"
            ),
        )
        .unwrap();

        for (rel, name, task) in [
            ("a", "a", "echo a"),
            ("b", "b", "echo b"),
            ("a/c", "c", "echo c"),
        ] {
            let dir = tmp.path().join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("pixi.toml"),
                format!(
                    "[workspace]\nname = \"{name}\"\nchannels = []\nplatforms = [\"linux-64\", \"osx-64\", \"osx-arm64\", \"win-64\"]\n\n[tasks]\ntest = \"{task}\"\n"
                ),
            )
            .unwrap();
        }
        tmp
    }

    fn locate_workspace(root: &Path) -> Workspace {
        pixi_core::WorkspaceLocator::for_cli()
            .with_consider_environment(false)
            .with_emit_warnings(false)
            .with_search_start(pixi_core::workspace::DiscoveryStart::SearchRoot(
                root.to_path_buf(),
            ))
            .locate()
            .expect("workspace should locate")
    }

    #[test]
    fn qualified_task_routes_to_member() {
        let tmp = build_hierarchical_fixture(true);
        let project = locate_workspace(tmp.path());
        let search = SearchEnvironments::from_opt_env(&project, None, None);

        let (env, task) = search
            .find_task("a::test".into(), FindTaskSource::CmdArgs, None)
            .expect("a::test must resolve");
        // Model 2: the returned Environment belongs to the **member's**
        // workspace, not the root. Verify by comparing workspace roots.
        let expected_root = std::fs::canonicalize(tmp.path().join("a")).unwrap();
        assert_eq!(
            env.workspace().root(),
            expected_root,
            "member tasks must run in the member's own workspace"
        );
        assert!(
            env.name().is_default(),
            "member task should run in the member's default env"
        );
        // Confirm we routed to the member's task, not the workspace's.
        // The member's `test` command is "echo a"; the root has no `test`.
        use pixi_manifest::task::CmdArgs;
        match task.as_command() {
            Some(CmdArgs::Single(s)) => assert_eq!(s.source(), "echo a"),
            other => panic!("expected `echo a`, got {other:?}"),
        }
    }

    #[test]
    fn nested_qualified_task_routes_to_grandchild() {
        let tmp = build_hierarchical_fixture(true);
        let project = locate_workspace(tmp.path());
        let search = SearchEnvironments::from_opt_env(&project, None, None);

        let (env, task) = search
            .find_task("a::c::test".into(), FindTaskSource::CmdArgs, None)
            .expect("a::c::test must resolve");
        // The returned env must belong to the inner member `a/c`, not to
        // `a` or the root.
        let expected_root = std::fs::canonicalize(tmp.path().join("a/c")).unwrap();
        assert_eq!(env.workspace().root(), expected_root);
        use pixi_manifest::task::CmdArgs;
        match task.as_command() {
            Some(CmdArgs::Single(s)) => assert_eq!(s.source(), "echo c"),
            other => panic!("expected `echo c`, got {other:?}"),
        }
    }

    #[test]
    fn qualified_task_with_unknown_leaf_returns_missing() {
        let tmp = build_hierarchical_fixture(true);
        let project = locate_workspace(tmp.path());
        let search = SearchEnvironments::from_opt_env(&project, None, None);

        let err = search
            .find_task("a::does_not_exist".into(), FindTaskSource::CmdArgs, None)
            .expect_err("unknown leaf task must fail");
        assert!(matches!(err, FindTaskError::MissingTask(_)));
    }

    #[test]
    fn unqualified_task_still_works_with_preview_on() {
        let tmp = build_hierarchical_fixture(true);
        let project = locate_workspace(tmp.path());
        let search = SearchEnvironments::from_opt_env(&project, None, None);

        // Root has `greet`; should resolve against the workspace as normal.
        let (env, _task) = search
            .find_task("greet".into(), FindTaskSource::CmdArgs, None)
            .expect("root task must still resolve");
        assert!(env.name().is_default());
    }

    #[test]
    fn qualified_task_is_unknown_when_preview_off() {
        let tmp = build_hierarchical_fixture(false);
        let project = locate_workspace(tmp.path());
        let search = SearchEnvironments::from_opt_env(&project, None, None);

        // Preview off → member tree is empty, so `a::test` falls through
        // to the normal task search and reports a missing task (there is
        // no root task literally named `a::test`).
        let err = search
            .find_task("a::test".into(), FindTaskSource::CmdArgs, None)
            .expect_err("preview-off must not resolve member tasks");
        assert!(matches!(err, FindTaskError::MissingTask(_)));
    }

    #[test]
    fn parse_qualified_task_name_edge_cases() {
        assert_eq!(parse_qualified_task_name("build"), None);
        assert_eq!(
            parse_qualified_task_name("a::build"),
            Some((vec!["a"], "build"))
        );
        assert_eq!(
            parse_qualified_task_name("a::b::c::t"),
            Some((vec!["a", "b", "c"], "t"))
        );
        // `::foo` splits to ["", "foo"]: degenerate but handled — caller
        // will fail at member-resolve time since `""` isn't a member.
        assert_eq!(
            parse_qualified_task_name("::foo"),
            Some((vec![""], "foo"))
        );
    }
}
