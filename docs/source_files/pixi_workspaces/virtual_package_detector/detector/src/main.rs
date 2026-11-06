use serde_json::json;
use std::{env, process::ExitCode};

fn main() -> ExitCode {
    let username = ["LOGNAME", "USER", "LNAME", "USERNAME"]
        .into_iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.is_empty()));
    let Some(username) = username else {
        eprintln!("username is not available in LOGNAME, USER, LNAME, or USERNAME");
        return ExitCode::FAILURE;
    };

    let report = json!({
        "version": 1,
        "virtual_packages": {
            "__username_length": {
                "version": username.chars().count().to_string()
            }
        },
        "cache": {
            "watch_env": ["LOGNAME", "USER", "LNAME", "USERNAME"]
        }
    });
    println!("{report}");
    ExitCode::SUCCESS
}
