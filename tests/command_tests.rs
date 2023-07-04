use crate::common::PixiControl;

mod common;

#[tokio::test]
pub async fn add_remove_command() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Simple command
    pixi.command()
        .add("test")
        .with_commands(["echo hello"])
        .execute()
        .unwrap();

    let project = pixi.project().unwrap();
    let cmd = project.manifest.commands.get("test").unwrap();
    assert!(matches!(cmd, pixi::command::Command::Plain(s) if s == "echo hello"));

    // Remove the command
    pixi.command().remove("test").await.unwrap();
    assert_eq!(pixi.project().unwrap().manifest.commands.len(), 0);
}

#[tokio::test]
pub async fn add_command_types() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Add a command with dependencies
    pixi.command()
        .add("test")
        .with_commands(["echo hello"])
        .execute()
        .unwrap();
    pixi.command()
        .add("test2")
        .with_commands(["echo hello", "echo bonjour"])
        .with_depends_on(["test"])
        .execute()
        .unwrap();

    let project = pixi.project().unwrap();
    let cmd = project.manifest.commands.get("test2").unwrap();
    assert!(
        matches!(cmd, pixi::command::Command::Process(cmd) if matches!(cmd.cmd, pixi::command::CmdArgs::Multiple(ref vec) if vec.len() == 2))
    );
    assert!(matches!(cmd, pixi::command::Command::Process(cmd) if !cmd.depends_on.is_empty()));

    // Create an alias
    pixi.command()
        .alias("testing")
        .with_depends_on(["test"])
        .execute()
        .unwrap();
    let project = pixi.project().unwrap();
    let cmd = project.manifest.commands.get("testing").unwrap();
    assert!(
        matches!(cmd, pixi::command::Command::Alias(a) if a.depends_on.get(0).unwrap() == "test")
    );
}
