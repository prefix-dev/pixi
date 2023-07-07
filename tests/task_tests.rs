use crate::common::PixiControl;
use pixi::task::{CmdArgs, Task};

mod common;

#[tokio::test]
pub async fn add_remove_task() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Simple task
    pixi.tasks()
        .add("test")
        .with_commands(["echo hello"])
        .execute()
        .unwrap();

    let project = pixi.project().unwrap();
    let task = project.manifest.tasks.get("test").unwrap();
    assert!(matches!(task, Task::Plain(s) if s == "echo hello"));

    // Remove the task
    pixi.tasks().remove("test").await.unwrap();
    assert_eq!(pixi.project().unwrap().manifest.tasks.len(), 0);
}

#[tokio::test]
pub async fn add_command_types() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Add a command with dependencies
    pixi.tasks()
        .add("test")
        .with_commands(["echo hello"])
        .execute()
        .unwrap();
    pixi.tasks()
        .add("test2")
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
        .alias("testing")
        .with_depends_on(["test"])
        .execute()
        .unwrap();
    let project = pixi.project().unwrap();
    let task = project.manifest.tasks.get("testing").unwrap();
    assert!(matches!(task, Task::Alias(a) if a.depends_on.get(0).unwrap() == "test"));
}
