use std::path::PathBuf;

use pixi::{
    cli::{cli_config::WorkspaceConfig, run::Args},
    task::TaskName,
};
use pixi_manifest::{task::CmdArgs, FeatureName, Task};
use rattler_conda_types::Platform;

use crate::common::PixiControl;

#[tokio::test]
pub async fn add_remove_task() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Simple task
    pixi.tasks()
        .add("test".into(), None, FeatureName::Default)
        .with_commands(["echo hello"])
        .execute()
        .await
        .unwrap();

    let project = pixi.workspace().unwrap();
    let tasks = project.default_environment().tasks(None).unwrap();
    let task = tasks.get(&<TaskName>::from("test")).unwrap();
    assert!(matches!(task, Task::Plain(s) if s == "echo hello"));

    // Remove the task
    pixi.tasks()
        .remove("test".into(), None, None)
        .await
        .unwrap();
    assert_eq!(
        pixi.workspace()
            .unwrap()
            .default_environment()
            .tasks(None)
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
pub async fn add_command_types() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Add a command with dependencies
    pixi.tasks()
        .add("test".into(), None, FeatureName::Default)
        .with_commands(["echo hello"])
        .execute()
        .await
        .unwrap();
    pixi.tasks()
        .add("test2".into(), None, FeatureName::Default)
        .with_commands(["echo hello", "echo bonjour"])
        .with_depends_on(vec!["test".into()])
        .execute()
        .await
        .unwrap();

    let project = pixi.workspace().unwrap();
    let tasks = project.default_environment().tasks(None).unwrap();
    let task2 = tasks.get(&<TaskName>::from("test2")).unwrap();
    let task = tasks.get(&<TaskName>::from("test")).unwrap();
    assert!(matches!(task2, Task::Execute(cmd) if matches!(cmd.cmd, CmdArgs::Single(_))));
    assert!(matches!(task2, Task::Execute(cmd) if !cmd.depends_on.is_empty()));

    assert_eq!(task.as_single_command().as_deref(), Some("echo hello"));
    assert_eq!(
        task2.as_single_command().as_deref(),
        Some("\"echo hello\" \"echo bonjour\"")
    );

    // Create an alias
    pixi.tasks()
        .alias("testing".into(), None)
        .with_depends_on(vec!["test".into(), "test3".into()])
        .execute()
        .await
        .unwrap();
    let project = pixi.workspace().unwrap();
    let tasks = project.default_environment().tasks(None).unwrap();
    let task = tasks.get(&<TaskName>::from("testing")).unwrap();
    assert!(matches!(task, Task::Alias(a) if a.depends_on.first().unwrap().as_str() == "test"));
}

#[tokio::test]
async fn test_alias() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().without_channels().await.unwrap();

    pixi.tasks()
        .add("hello".into(), None, FeatureName::Default)
        .with_commands(["echo hello"])
        .execute()
        .await
        .unwrap();

    pixi.tasks()
        .add("world".into(), None, FeatureName::Default)
        .with_commands(["echo world"])
        .execute()
        .await
        .unwrap();

    pixi.tasks()
        .add("helloworld".into(), None, FeatureName::Default)
        .with_depends_on(vec!["hello".into(), "world".into()])
        .execute()
        .await
        .unwrap();

    let result = pixi
        .run(Args {
            task: vec!["helloworld".to_string()],
            workspace_config: WorkspaceConfig {
                manifest_path: None,
            },
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "hello\nworld\n");
}

#[tokio::test]
pub async fn add_remove_target_specific_task() {
    let pixi = PixiControl::new().unwrap();
    pixi.init_with_platforms(vec!["win-64".to_string()])
        .await
        .unwrap();

    // Simple task
    pixi.tasks()
        .add("test".into(), Some(Platform::Win64), FeatureName::Default)
        .with_commands(["echo only_on_windows"])
        .execute()
        .await
        .unwrap();

    let project = pixi.workspace().unwrap();
    let task = *project
        .default_environment()
        .tasks(Some(Platform::Win64))
        .unwrap()
        .get(&<TaskName>::from("test"))
        .unwrap();
    assert!(matches!(task, Task::Plain(s) if s == "echo only_on_windows"));

    // Simple task
    pixi.tasks()
        .add("test".into(), None, FeatureName::Default)
        .with_commands(["echo hello"])
        .execute()
        .await
        .unwrap();

    // Remove the task
    pixi.tasks()
        .remove("test".into(), Some(Platform::Win64), None)
        .await
        .unwrap();
    assert_eq!(
        project
            .default_environment()
            .tasks(Some(Platform::Win64))
            .unwrap()
            .len(),
        // The default task is still there
        1
    );
}

#[tokio::test]
async fn test_cwd() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().without_channels().await.unwrap();

    // Create test dir
    fs_err::create_dir(pixi.workspace_path().join("test")).unwrap();

    pixi.tasks()
        .add("pwd-test".into(), None, FeatureName::Default)
        .with_commands(["pwd"])
        .with_cwd(PathBuf::from("test"))
        .execute()
        .await
        .unwrap();

    let result = pixi
        .run(Args {
            task: vec!["pwd-test".to_string()],
            workspace_config: WorkspaceConfig {
                manifest_path: None,
            },
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("test"));

    // Test that an unknown cwd gives an error
    pixi.tasks()
        .add("unknown-cwd".into(), None, FeatureName::Default)
        .with_commands(["pwd"])
        .with_cwd(PathBuf::from("tests"))
        .execute()
        .await
        .unwrap();

    assert!(pixi
        .run(Args {
            task: vec!["unknown-cwd".to_string()],
            workspace_config: WorkspaceConfig {
                manifest_path: None
            },
            ..Default::default()
        })
        .await
        .is_err());
}

#[tokio::test]
async fn test_task_with_env() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().without_channels().await.unwrap();

    pixi.tasks()
        .add("env-test".into(), None, FeatureName::Default)
        .with_commands(["echo From a $HELLO_WORLD"])
        .with_env(vec![(
            String::from("HELLO_WORLD"),
            String::from("world with spaces"),
        )])
        .execute()
        .await
        .unwrap();

    let result = pixi
        .run(Args {
            task: vec!["env-test".to_string()],
            workspace_config: WorkspaceConfig {
                manifest_path: None,
            },
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "From a world with spaces\n");
}

#[tokio::test]
async fn test_clean_env() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().without_channels().await.unwrap();

    std::env::set_var("HELLO", "world from env");
    pixi.tasks()
        .add("env-test".into(), None, FeatureName::Default)
        .with_commands(["echo Hello is: $HELLO"])
        .execute()
        .await
        .unwrap();

    let run = pixi.run(Args {
        task: vec!["env-test".to_string()],
        workspace_config: WorkspaceConfig {
            manifest_path: None,
        },
        clean_env: true,
        ..Default::default()
    });

    if cfg!(windows) {
        // Clean env running not supported on windows.
        run.await.unwrap_err();
    } else {
        let result = run.await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "Hello is:\n");
    }

    let result = pixi
        .run(Args {
            task: vec!["env-test".to_string()],
            workspace_config: WorkspaceConfig {
                manifest_path: None,
            },
            clean_env: false,
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "Hello is: world from env\n");
}

// When adding another test with an environment variable, please choose a unique
// name to avoid collisions
