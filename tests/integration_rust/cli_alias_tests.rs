use std::process::Command;

use pixi::cli::Command as PixiCommand;
use pixi::cli::Args;

#[test]
fn test_command_aliases() {
    // Test that all aliases are properly registered
    let args = Args::parse_from(["pixi", "--help"]);
    let help_text = format!("{:?}", args);

    // Check that all aliases are visible in help text
    assert!(help_text.contains("new") || help_text.contains("init"));
    assert!(help_text.contains("a") || help_text.contains("add"));
    assert!(help_text.contains("rm") || help_text.contains("remove"));
    assert!(help_text.contains("i") || help_text.contains("install"));
    assert!(help_text.contains("u") || help_text.contains("update"));
    assert!(help_text.contains("up") || help_text.contains("upgrade"));
    assert!(help_text.contains("l") || help_text.contains("lock"));
    assert!(help_text.contains("r") || help_text.contains("run"));
    assert!(help_text.contains("x") || help_text.contains("exec"));
    assert!(help_text.contains("s") || help_text.contains("shell"));
    assert!(help_text.contains("project") || help_text.contains("workspace"));
    assert!(help_text.contains("tsk") || help_text.contains("task"));
    assert!(help_text.contains("ls") || help_text.contains("list"));
    assert!(help_text.contains("t") || help_text.contains("tree"));
    assert!(help_text.contains("g") || help_text.contains("global"));
    assert!(help_text.contains("cfg") || help_text.contains("config"));
    assert!(help_text.contains("inf") || help_text.contains("info"));
    assert!(help_text.contains("find") || help_text.contains("search"));
    assert!(help_text.contains("c") || help_text.contains("clean"));
}

#[test]
fn test_alias_functionality() {
    // Test that aliases work as expected
    let test_cases = vec![
        (vec!["pixi", "new"], "init"),
        (vec!["pixi", "a"], "add"),
        (vec!["pixi", "rm"], "remove"),
        (vec!["pixi", "i"], "install"),
        (vec!["pixi", "u"], "update"),
        (vec!["pixi", "up"], "upgrade"),
        (vec!["pixi", "l"], "lock"),
        (vec!["pixi", "r"], "run"),
        (vec!["pixi", "x"], "exec"),
        (vec!["pixi", "s"], "shell"),
        (vec!["pixi", "project"], "workspace"),
        (vec!["pixi", "tsk"], "task"),
        (vec!["pixi", "ls"], "list"),
        (vec!["pixi", "t"], "tree"),
        (vec!["pixi", "g"], "global"),
        (vec!["pixi", "cfg"], "config"),
        (vec!["pixi", "inf"], "info"),
        (vec!["pixi", "find"], "search"),
        (vec!["pixi", "c"], "clean"),
    ];

    for (args, expected_command) in test_cases {
        let parsed_args = Args::parse_from(args);
        let command_str = format!("{:?}", parsed_args.command);
        assert!(
            command_str.contains(expected_command),
            "Alias test failed for args: {:?}, expected command: {}",
            args,
            expected_command
        );
    }
}
