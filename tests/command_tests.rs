use crate::common::PixiControl;

mod common;

#[tokio::test]
pub async fn add_command() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Simple command
    pixi.command()
        .add("test")
        .with_commands(&["echo hello"])
        .execute()
        .unwrap();

    let project = pixi.project().unwrap();
    let cmd = project.manifest.commands.get("test").unwrap();
    assert!(matches!(cmd, pixi::command::Command::Plain(s) if s == "echo hello"));

    // Remove the command
    pixi.command().remove("test").await.unwrap();
    assert_eq!(pixi.project().unwrap().manifest.commands.len(), 0);

    // Add a command with dependencies
    pixi.command()
        .add("test")
        .with_commands(&["echo hello"])
        .with_depends_on(&["something_else"])
        .execute()
        .unwrap();
    pixi.command()
        .add("test2")
        .with_commands(&["echo hello", "Bonjour"])
        .execute()
        .unwrap();

    let project = pixi.project().unwrap();
    let cmd = project.manifest.commands.get("test").unwrap();
    assert!(matches!(cmd, pixi::command::Command::Process(cmd) if cmd.depends_on.len() > 0));
    assert!(
        matches!(cmd, pixi::command::Command::Process(cmd) if matches!(cmd.cmd, pixi::command::CmdArgs::Single(_)))
    );
    let cmd = project.manifest.commands.get("test2").unwrap();
    assert!(
        matches!(cmd, pixi::command::Command::Process(cmd) if matches!(cmd.cmd, pixi::command::CmdArgs::Multiple(_)))
    );

    // Create an alias
    pixi.command()
        .alias("testing")
        .with_depends_on(&["test"])
        .execute()
        .unwrap();
    let project = pixi.project().unwrap();
    let cmd = project.manifest.commands.get("testing").unwrap();
    assert!(matches!(cmd, pixi::command::Command::Alias(a) if a.depends_on.len() > 0));
}
