use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "macos")]
pub const TRAMPOLINE_BIN: &[u8] =
    include_bytes!("../../crates/pixi_trampoline/trampolines/pixi_trampoline_debug");

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "windows")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-aarch64-pc-windows-msvc.exe"
);

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "linux")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-aarch64-unknown-linux-musl"
);

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "macos")]
const TRAMPOLINE_BIN: &[u8] =
    include_bytes!("../../crates/pixi_trampoline/trampolines/pixi-trampoline-x86_64-apple-darwin");

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "windows")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-x86_64-pc-windows-msvc.exe"
);

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "linux")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-x86_64-unknown-linux-musl"
);

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct ManifestMetadata {
    pub exe: PathBuf,
    pub path: String,
    pub env: HashMap<String, String>,
}

#[allow(dead_code)]
pub struct Trampoline {
    binary_data: &'static [u8],
}

#[allow(dead_code)]
impl Trampoline {
    pub fn new() -> Self {
        let binary_data = TRAMPOLINE_BIN;
        Trampoline { binary_data }
    }

    pub fn get_binary_size(&self) -> usize {
        self.binary_data.len()
    }

    // Add more methods as needed for your specific use case
}

#[cfg(test)]
mod tests {
    use super::*;
    use which::which;

    #[test]
    fn test_trampoline_creation() {
        let trampoline = Trampoline::new();
        assert!(
            trampoline.get_binary_size() > 0,
            "Binary should not be empty"
        );
    }

    #[test]
    fn test_python_execution_using_trampoline() {
        let python_script = format!(
            r##"
# -*- coding: utf-8 -*-
import os
import sys

def main():
    # Get the environment variable
    env_var_value = os.getenv('TRAMPOLINE_TEST_ENV')

    # Check if it's set to 'teapot'
    assert env_var_value == "teapot"
if __name__ == "__main__":
    main()
"##
        );
        // Locate an arbitrary python installation from PATH
        let python_executable_path = which("python").unwrap();

        let trampoline = Trampoline::new();

        let env_dir = std::env::temp_dir();
        let trampoline_test_script_path = env_dir.join("trampoline_test.py");

        let trampoline_path = env_dir.join("trampoline");
        // trampoline_script_path.push("trampoline_test.py");

        std::fs::write(&trampoline_test_script_path, python_script)
            .expect("Failed to write trampoline script");
        std::fs::write(&trampoline_path, TRAMPOLINE_BIN)
            .expect("Failed to write trampoline script");

        // set permission
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(&trampoline_path).expect("Failed to get metadata");
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o755); // rwxr-xr-x
            std::fs::set_permissions(&trampoline_path, permissions)
                .expect("Failed to set permissions");
        }

        let mut current_env = std::env::vars().collect::<HashMap<String, String>>();
        current_env.insert("TRAMPOLINE_TEST_ENV".to_string(), "teapot".to_string());

        let metadata = ManifestMetadata {
            exe: python_executable_path.clone(),
            path: python_executable_path
                .parent()
                .unwrap()
                .to_string_lossy()
                .to_string(),
            env: current_env,
        };

        let json_path = env_dir.join("trampoline.json");
        // serde deser into json
        let file = std::fs::File::create(&json_path).expect("Failed to create json file");
        serde_json::to_writer(file, &metadata).expect("Failed to write metadata to json file");

        // start the trampoline and pass an argument

        let mut command = std::process::Command::new(trampoline_path);
        command.arg(trampoline_test_script_path);
        let status = command.status().expect("Failed to execute command");
        eprintln!("status is {:?}", status);
    }
}
