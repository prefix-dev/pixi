mod common;

use common::{LockFileExt, PixiControl};

/// Should add a python version to the environment and lock file that matches the specified version
#[tokio::test]
async fn add_python() {
    let mut pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    pixi.add(["python==3.11.0"]).await.unwrap();
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec("python==3.11.0"));
    pixi.run(["python", "--version"]).await.unwrap();
}
