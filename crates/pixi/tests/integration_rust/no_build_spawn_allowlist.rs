//! Fails if a new subprocess spawn appears on the lockfile build path
//! outside the files that check [`pixi_compute_engine::BuildExecutionPermit`].
use std::path::Path;

const ROOTS: &[&str] = &[
    "crates/pixi_build_frontend/src",
    "crates/pixi_command_dispatcher/src",
    "crates/pixi_core/src/lock_file",
    "crates/pixi_git/src",
];

/// Files allowed to construct or spawn a process. New backends must call
/// these helpers instead of `Command::new` directly.
const ALLOWLIST: &[&str] = &[
    "crates/pixi_build_frontend/src/backend/json_rpc.rs",
    "crates/pixi_build_frontend/src/tool.rs",
    "crates/pixi_command_dispatcher/src/file_fingerprint.rs",
    "crates/pixi_git/src/git.rs",
];

#[test]
fn build_spawn_sites_are_allowlisted() {
    let workspace = Path::new(env!("CARGO_WORKSPACE_DIR"));
    let mut unexpected = Vec::new();
    for root in ROOTS {
        let dir = workspace.join(root);
        visit(&dir, workspace, &mut unexpected);
    }
    assert!(
        unexpected.is_empty(),
        "new process spawn outside the build-execution gate:\n{}",
        unexpected.join("\n")
    );
}

fn visit(dir: &Path, workspace: &Path, unexpected: &mut Vec<String>) {
    let entries = match fs_err::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, workspace, unexpected);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let relative = path
            .strip_prefix(workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWLIST.iter().any(|allowed| relative == *allowed) {
            continue;
        }
        let contents = fs_err::read_to_string(&path).unwrap_or_default();
        if contents.contains("Command::new") || contents.contains(".spawn()") {
            unexpected.push(relative);
        }
    }
}
