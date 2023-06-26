mod common;

use common::{LockFileExt, PixiControl};

/// Should add a python version to the environment and lock file that matches the specified version
/// and run it
#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_run_python() {
    let mut pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    pixi.add(["python==3.11.0"]).await.unwrap();

    // Check if lock has python version
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec("python==3.11.0"));

    // Check if python is installed and can be run
    let result = pixi.run(["python", "--version"]).await.unwrap();
    assert!(result.success());
    assert_eq!(result.stdout().trim(), "Python 3.11.0");
}
