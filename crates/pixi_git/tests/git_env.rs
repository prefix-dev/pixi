//! Integration test for running `GitSource::fetch` from inside a git hook,
//! where git exports repository-selecting variables such as `GIT_DIR` and
//! `GIT_INDEX_FILE` (#7090). This lives in its own test binary because it
//! mutates the process environment.

use std::path::Path;
use std::process::Command;

use pixi_git::{GitUrl, source::GitSource};
use pixi_test_utils::GitRepoFixture;
use rattler_networking::LazyClient;
use reqwest_middleware::ClientWithMiddleware;

/// Variables that make git operate on another repository than its working
/// directory.
const REPOSITORY_ENV_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
];

/// LazyClient that panics if HTTP is touched. file:// URLs never trigger it.
fn panic_client() -> LazyClient {
    LazyClient::new(|| -> ClientWithMiddleware {
        panic!("network should not be used in git env tests")
    })
}

/// Runs `git` in `dir` without inheriting any [`REPOSITORY_ENV_VARS`], so it
/// is safe to use while the test has them set.
fn git(dir: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    for var in REPOSITORY_ENV_VARS {
        cmd.env_remove(var);
    }
    let output = cmd
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn `git {}`: {err}", args.join(" ")));
    assert!(
        output.status.success(),
        "`git {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// Fetches `fixture` with the environment a hook running in a linked
/// worktree of an unrelated repository sees, and checks that the fetch
/// succeeds without touching that repository.
fn fetch_from_linked_worktree_hook(fixture: &GitRepoFixture, set_work_tree: bool) {
    let cache = tempfile::tempdir().unwrap();

    // The repository the hook runs in, with a linked worktree.
    let victim = tempfile::tempdir().unwrap();
    let main = victim.path().join("main");
    let worktree = victim.path().join("wt");
    fs_err::create_dir_all(&main).unwrap();
    git(&main, &["init", "-b", "main"]);
    fs_err::write(main.join("file.txt"), "content").unwrap();
    git(&main, &["add", "file.txt"]);
    git(
        &main,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@test.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--message",
            "initial",
        ],
    );
    git(&main, &["worktree", "add", worktree.to_str().unwrap()]);

    let shared_config = main.join(".git").join("config");
    let worktree_git_dir = main.join(".git").join("worktrees").join("wt");
    let worktree_index = worktree_git_dir.join("index");
    let config_before = fs_err::read_to_string(&shared_config).unwrap();
    let index_before = fs_err::read(&worktree_index).unwrap();
    let head_before = git(&worktree, &["rev-parse", "HEAD"]);

    // SAFETY: this test binary runs a single test, so no other thread reads
    // or writes the environment concurrently.
    unsafe {
        std::env::set_var("GIT_DIR", &worktree_git_dir);
        std::env::set_var("GIT_INDEX_FILE", &worktree_index);
        if set_work_tree {
            std::env::set_var("GIT_WORK_TREE", &worktree);
        }
    }

    let git_url = GitUrl::try_from(fixture.base_url.clone()).unwrap();
    let result = GitSource::new(git_url, panic_client(), cache.path()).fetch();

    // SAFETY: see above.
    unsafe {
        for var in REPOSITORY_ENV_VARS {
            std::env::remove_var(var);
        }
    }

    let config_after = fs_err::read_to_string(&shared_config).unwrap();
    assert!(
        !config_after.contains("bare = true"),
        "fetch must not mark the repository in GIT_DIR as bare:\n{config_after}"
    );
    assert_eq!(
        config_after, config_before,
        "fetch must not modify the config of the repository in GIT_DIR"
    );
    assert!(
        fs_err::read(&worktree_index).unwrap() == index_before,
        "fetch must not modify the index in GIT_INDEX_FILE"
    );
    assert_eq!(
        git(&worktree, &["rev-parse", "HEAD"]),
        head_before,
        "fetch must not move HEAD of the repository in GIT_DIR"
    );

    let fetch = result.expect("fetch should succeed");
    assert_eq!(fetch.commit().to_string(), fixture.latest_commit());
    assert!(
        fetch.path().join("pyproject.toml").is_file(),
        "checkout at {} should contain the fixture files",
        fetch.path().display()
    );
}

/// A fetch started from a git hook, as `pixi lock` in a `pre-commit` hook
/// is, succeeds and leaves the user's repository alone. Git exports
/// `GIT_DIR` and `GIT_INDEX_FILE` to hooks in a linked worktree; the second
/// case also sets `GIT_WORK_TREE`, as `git --work-tree` would.
#[test]
fn fetch_ignores_inherited_git_repository_env() {
    // Created before the environment is modified: the fixture helpers run
    // git with the inherited environment.
    let fixture = GitRepoFixture::new("minimal-pypi-package");

    fetch_from_linked_worktree_hook(&fixture, false);
    fetch_from_linked_worktree_hook(&fixture, true);
}
