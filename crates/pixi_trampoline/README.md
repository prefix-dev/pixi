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
It will be built automatically when you run `cargo build`  on pixi by using `build.rs`.
