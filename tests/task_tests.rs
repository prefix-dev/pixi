use crate::common::PixiControl;
use pixi::task::{CmdArgs, Task};
use rattler_conda_types::Platform;

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
    let task = project.manifest.tasks.get("test").unwrap();
    assert!(matches!(task, Task::Plain(s) if s == "echo hello"));

    // Remove the task
    pixi.tasks().remove("test", None).await.unwrap();
    assert_eq!(pixi.project().unwrap().manifest.tasks.len(), 0);
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
    let task = project.manifest.tasks.get("test2").unwrap();
    assert!(matches!(task, Task::Execute(cmd) if matches!(cmd.cmd, CmdArgs::Single(_))));
    assert!(matches!(task, Task::Execute(cmd) if !cmd.depends_on.is_empty()));

    // Create an alias
    pixi.tasks()
        .alias("testing", None)
        .with_depends_on(["test"])
        .execute()
        .unwrap();
    let project = pixi.project().unwrap();
    let task = project.manifest.tasks.get("testing").unwrap();
    assert!(matches!(task, Task::Alias(a) if a.depends_on.get(0).unwrap() == "test"));
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
        pixi.project()
            .unwrap()
            .target_specific_tasks(Platform::Win64)
            .len(),
        0
    );
}
