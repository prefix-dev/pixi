# A small trampoline binary that allow to run executables installed by pixi global install.


This is the configuration used by trampoline to set the env variables, and run the executable.

```js
{
    // Full path to the executable
    "exe": "/Users/wolfv/.pixi/envs/conda-smithy/bin/conda-smithy",
    // One or more path segments to prepend to the PATH environment variable
    "path": "/Users/wolfv/.pixi/envs/conda-smithy/bin",
    // One or more environment variables to set
    "env": {
        "CONDA_PREFIX": "/Users/wolfv/.pixi/envs/conda-smithy"
    }
}
```

# How to build it?
You can use `trampoline.yaml` workflow to build the binary for all the platforms and architectures supported by pixi.
In case of building it manually, you can use the following command, after executing the `cargo build --release`, you need to compress it using `zstd`.
If running it manually or triggered by changes in `crates/pixi_trampoline` from the main repo, they will be automatically committed to the branch.
