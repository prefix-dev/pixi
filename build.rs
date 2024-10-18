use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

static PIXI_TRAMPOLINE: &str = "pixi_trampoline";
static TRAMPOLINES_FOLDER: &str = "trampolines";

fn main() {
    // Construct trampoline crate path
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let trampoline_crate_path = format!("{}/crates/{}", manifest_dir, PIXI_TRAMPOLINE);
    let trampoline_crate = Path::new(&trampoline_crate_path);
    let trampolines_dir = trampoline_crate.join(TRAMPOLINES_FOLDER);

    // Get the target triplet, which will be used to name the trampoline binary
    let target_triplet = env::var("TARGET").expect("TARGET env variable should be set by cargo");

    // Build the trampoline binary
    // we always use --release profile for this
    Command::new("cargo")
        .current_dir(&trampoline_crate)
        .args(&[
            "build",
            "--release",
            "--target",
            target_triplet.as_str(),
            "--target-dir",
            ".pixi/target",
        ])
        .status()
        .expect("Failed to build trampoline crate");

    // Create trampolines directory if it doesn't exist
    fs::create_dir_all(&trampolines_dir).expect("Failed to create trampolines directory");

    // let cargo_target_dir =
    //     env::var("CARGO_TARGET_DIR").expect("CARGO_TARGET_DIR should be set by cargo");

    let cargo_target_dir = format!(".pixi/target");

    // Construct the path to the built binary
    let target_path = PathBuf::from_iter([
        cargo_target_dir.as_str(),
        target_triplet.as_str(),
        "release",
        PIXI_TRAMPOLINE,
    ]);

    let built_binary_path = trampoline_crate.join(target_path);
    let dest_path = trampolines_dir.join(format!("pixi_trampoline-{}", target_triplet));

    // Copy the built binary to the trampolines directory
    fs::copy(&built_binary_path, &dest_path).expect("Failed to copy second crate binary");

    // Tell cargo to re-run this build script if the trampoline binary code changes
    println!("cargo:rerun-if-changed=crates/pixi_trampoline");
    println!(
        "cargo:rustc-env=TRAMPOLINE_PATH={}",
        dest_path.display().to_string()
    );
}
