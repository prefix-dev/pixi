# Building Mojo CPU and AMD Packages

Build two `mojo-fractal` packages from one Mojo source: an x86-64-v3 CPU executable and a `gfx1103` AMD GPU executable.
Both have the same name and version.
A separate consumer installs the compatible binary from a local channel, without a source dependency or a build backend.

This example needs Linux x86-64 with glibc 2.34 or newer and an x86-64-v3-capable CPU, including for the AMD build.
The AMD path also needs a usable `gfx1103` GPU, driver, and permission to access the device.
Other machines use the CPU package.
See the [backend reference](backends/pixi-build-mojo.md#accelerator-variants) for the supported variants and their runtime requirements.

!!! warning "Use the matching checkout"
    Accelerator variants are not assumed to be available in a published backend release.
    Use Pixi from this checkout and build its matching backend.
    The commands below use Nushell and start at the repository root:

    ```nu
    let repo = $env.PWD
    pixi run -e rust-test cargo build --workspace --bin pixi --bin pixi-build-mojo
    $env.PATH = ($env.PATH | prepend $"($repo)/target/pixi/debug")
    $env.PIXI_BUILD_BACKEND_OVERRIDE = $"pixi-build-mojo=($repo)/target/pixi/debug/pixi-build-mojo"
    ```

    This path is the output of the checkout's `rust-test` environment, which sets `CARGO_TARGET_DIR` to `target/pixi`.
    Keep the override set while publishing.
    It selects the local executable instead of installing a published backend; it does not affect the consumer.

## Build and publish

The example is in `docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators`.
Its `package/` directory is a standalone producer workspace.
It has no local-channel dependency, so the first publish can run before that channel exists.

```toml title="package/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/package/pixi.toml"
```

`mojo_accelerator` opts in to the two canonical variants.
The backend supplies the compile-time define, hides devices for CPU compilation, and sets the shared x86-64-v3 host baseline for both builds.
The source branches at compile time; the AMD executable does real device work, with no CPU fallback inside it.
The compiler pin in `[package.build-dependencies]` and the runtime provider pin in `[package.host-dependencies]` make this example reproducible.
`--fp-mode contract=off` keeps floating-point contraction disabled in both variants.
The backend provides the conda-forge C and C++ compiler requirements and infers the glibc sysroot, so no compiler metapackages or explicit sysroot dependencies are needed.

Publish from the producer directory, with an explicit package manifest:

```nu
cd docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/package
pixi publish --path ./pixi.toml --target-channel ../channel
cd ..
```

`--target-channel` creates an indexed local channel containing both builds.
Do not use `--target-dir`, which only copies package archives without indexing them.

## Consume the binaries

The root workspace is an ordinary binary consumer:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/pixi.toml"
```

The two [rich platforms](../workspace/multi_platform_configuration.md#declaring-virtual-packages-per-platform) describe separate solves for the same conda subdir.
AMD comes first, so Pixi selects it automatically on a compatible machine; otherwise it selects `cpu`.
Both platforms declare the same `archspec = "x86_64_v3"` minimum because the prebuilt [Modular runtime requires it](https://mojolang.org/docs/requirements/#hardware).
The backend uses matching CPU flags and records `_x86_64-microarch-level >=3`; the platform declaration alone does not set compiler flags.
`amdgpu = "gfx1103"` means that exact ISA, not `gfx1103` or newer.
Within a solve where both builds are compatible, the CPU build's `track_features` entry makes it the lower-priority fallback.

From the example root, run the automatically selected binary:

```nu
pixi run fractal
pixi list '^mojo-fractal$' --fields name,version,build,track-features
```

Force the CPU platform and assert that the executable contains the CPU path:

```nu
pixi run --platform cpu fractal 1024 1024 512 5 cpu
pixi list --platform cpu '^mojo-fractal$' --fields name,version,build,track-features
```

On a compatible AMD machine, select and check the GPU path:

```nu
pixi run --platform amd fractal 1024 1024 512 5 gfx1103
pixi list --platform amd '^mojo-fractal$' --fields name,version,build,track-features
```

Compare the `Build` columns: the package name and version stay the same, but the build hashes differ.
The CPU build has `mojo-fractal-p-0` in `Track Features`; the AMD build does not.
The printed `path` and optional fifth argument check the compiled implementation, not just the solver's choice.
Forcing `amd` does not make an unsupported GPU compatible.

## Correctness and timings

The workload computes deterministic float32 Mandelbrot escape counts.
The CPU implementation uses parallel rows and portable SIMD; the GPU implementation uses a device kernel.
Every output element is compared exactly with an independent scalar reference, with fixed known escape counts checked separately.
Validation happens outside the timed regions, and a mismatch fails the command.
A checksum alone is not the correctness check.

The four numeric arguments are width, height, maximum iterations, and measured repetitions.
Without arguments, the defaults are `1024 1024 512 5`.
The CPU implementation schedules eight row workers and reports the runtime's available parallelism.
Each run prints its configuration and raw `kernel_ns` and `e2e_ns` for every repetition:

- `kernel_ns` measures computation with resources already prepared, including synchronization before the timer stops.
- `e2e_ns` measures a complete image: per-image context/setup, allocation, computation, synchronization, and transfer into owned, usable host output. It is not just kernel launch time.
- Two warmups exclude one-time runtime startup and offload compilation from the measured samples. Reference calculation, validation, and printing are also outside the measurements.

Use identical dimensions, iteration limits, and repetitions when comparing paths.
Keep CPU thread count, power profile, AC/battery state, and background load fixed.
An integrated GPU shares memory bandwidth, thermal limits, and a power budget with the CPU, so kernel and end-to-end results answer different questions.
Record the configuration alongside the raw samples rather than treating one speedup as a hardware-independent result.

## Source

The same source is compiled for both packages:

```mojo title="package/main.mojo"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/package/main.mojo"
```
