import subprocess


def main():
    # List of manifest paths
    manifest_paths = ["./Cargo.toml", "pixi_docs/Cargo.toml"]

    # Update lockfiles
    for manifest_path in manifest_paths:
        subprocess.run(
            ["cargo", "tree", "--manifest-path", manifest_path], stdout=subprocess.DEVNULL
        )


if __name__ == "__main__":
    main()
