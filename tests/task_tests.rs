use crate::common::PixiControl;
use pixi::cli::run::Args;
use pixi::task::{CmdArgs, Task};
use rattler_conda_types::Platform;
use std::fs;
use std::path::PathBuf;

mod common;

#[tokio::test]
pub async fn add_remove_task() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Simple task
    pixi.tasks()
        .add("test", None)
        .with_commands(["echo hello"])
        .execute()
        .unwrap();

    let project = pixi.project().unwrap();
    let tasks = project.tasks(None);
    let task = tasks.get("test").unwrap();
    assert!(matches!(task, Task::Plain(s) if s == "echo hello"));

    // Remove the task
    pixi.tasks().remove("test", None).await.unwrap();
    assert_eq!(pixi.project().unwrap().tasks(None).len(), 0);
}

#[tokio::test]
pub async fn add_command_types() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Add a command with dependencies
    pixi.tasks()
        .add("test", None)
        .with_commands(["echo hello"])
        .execute()
        .unwrap();
    pixi.tasks()
        .add("test2", None)
        .with_commands(["echo hello", "echo bonjour"])
        .with_depends_on(["test"])
        .execute()
        .unwrap();

    let project = pixi.project().unwrap();
    let tasks = project.tasks(None);
    let task2 = tasks.get("test2").unwrap();
    let task = tasks.get("test").unwrap();
    assert!(matches!(task2, Task::Execute(cmd) if matches!(cmd.cmd, CmdArgs::Single(_))));
    assert!(matches!(task2, Task::Execute(cmd) if !cmd.depends_on.is_empty()));

    assert_eq!(task.as_single_command().as_deref(), Some("echo hello"));
    assert_eq!(
        task2.as_single_command().as_deref(),
        Some("\"echo hello\" \"echo bonjour\"")
    );

    // Create an alias
    pixi.tasks()
        .alias("testing", None)
        .with_depends_on(["test"])
        .execute()
        .unwrap();
    let project = pixi.project().unwrap();
    let tasks = project.tasks(None);
    let task = tasks.get("testing").unwrap();
    assert!(matches!(task, Task::Alias(a) if a.depends_on.get(0).unwrap() == "test"));
}

#[tokio::test]
async fn test_alias() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().without_channels().await.unwrap();

    pixi.tasks()
        .add("hello", None)
        .with_commands(["echo hello"])
        .execute()
        .unwrap();

    pixi.tasks()
        .add("world", None)
        .with_commands(["echo world"])
        .execute()
        .unwrap();

    pixi.tasks()
        .add("helloworld", None)
        .with_depends_on(["hello", "world"])
        .execute()
        .unwrap();

    let result = pixi
        .run(Args {
            task: vec!["helloworld".to_string()],
            manifest_path: None,
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
        .add("test", Some(Platform::Win64))
        .with_commands(["echo only_on_windows"])
        .execute()
        .unwrap();

    let project = pixi.project().unwrap();
    let task = *project.tasks(Some(Platform::Win64)).get("test").unwrap();
    assert!(matches!(task, Task::Plain(s) if s == "echo only_on_windows"));

    // Simple task
    pixi.tasks()
        .add("test", None)
        .with_commands(["echo hello"])
        .execute()
        .unwrap();

    // Remove the task
    pixi.tasks()
        .remove("test", Some(Platform::Win64))
        .await
        .unwrap();
    assert_eq!(
        project.tasks(Some(Platform::Win64)).len(),
        // The default task is still there
        1
    );
}

#[tokio::test]
async fn test_cwd() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().without_channels().await.unwrap();

    // Create test dir
    fs::create_dir(pixi.project_path().join("test")).unwrap();

    pixi.tasks()
        .add("pwd-test", None)
        .with_commands(["pwd"])
        .with_cwd(PathBuf::from("test"))
        .execute()
        .unwrap();

    let result = pixi
        .run(Args {
            task: vec!["pwd-test".to_string()],
            manifest_path: None,
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("test"));

    // Test that an unknown cwd gives an error
    pixi.tasks()
        .add("unknown-cwd", None)
        .with_commands(["pwd"])
        .with_cwd(PathBuf::from("tests"))
        .execute()
        .unwrap();

    assert!(pixi
        .run(Args {
            task: vec!["unknown-cwd".to_string()],
            manifest_path: None,
            ..Default::default()
        })
        .await
        .is_err());
}
