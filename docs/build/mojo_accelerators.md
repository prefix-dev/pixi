# Building Mojo CPU and AMD Packages

Build two `mojo-matmul` packages from one source: a single-threaded CPU executable and an AMD GPU executable, selected at compile time.
Publish both to a local channel, then install the compatible binary in a separate workspace without a source dependency or build backend.

This example needs Linux x86-64, glibc 2.34 or newer, and an x86-64-v3-capable CPU for both builds.
The GPU build targets `gfx1103`, such as the Radeon 780M, and needs a working driver and permission to access the device.
See the [Mojo backend reference](backends/pixi-build-mojo.md#accelerator-variants) for implementation limits and runtime requirements, and [rich platforms](../workspace/multi_platform_configuration.md#declaring-virtual-packages-per-platform) for hardware declarations, including other AMD targets.
`pixi-build` is a preview feature.

!!! warning "Backend prerequisite"
    This tutorial requires Pixi and `pixi-build-mojo` with managed accelerator variants.
    Until that support is released, use the checkout setup in [PR #7432](https://github.com/prefix-dev/pixi/pull/7432).

## Create the producer

Create these files in a new directory:

```text
.
├── package/
│   ├── main.mojo
│   └── pixi.toml
└── pixi.toml
```

Start with the matrix multiplication program:

```mojo title="package/main.mojo"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/package/main.mojo"
```

The CPU branch uses a plain, single-threaded loop with normal compiler optimizations, without explicit SIMD or threading.
The GPU branch copies the inputs to device buffers, launches one thread per output cell, copies the result back, and synchronizes before reading it.
Both use the same dot product. There is no runtime CPU fallback in the GPU binary.

The left matrix contains ones, with twos on its diagonal; the right contains twos, with threes on its diagonal.
Their product contains `2 * SIDE + 3` off the diagonal and `2 * SIDE + 4` on it.
All values are exactly representable float32 integers at this size.
Both branches check every output cell against that result and fail on a mismatch.

Next, define the producer workspace and package:

```toml title="package/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/package/pixi.toml"
```

The `mojo_accelerator` build variant produces two packages with the same name and version but different build hashes.
The backend supplies the compile-time define and compiler flags for each implementation, with a shared x86-64-v3 host baseline.
The compiler and runtime pins keep this example on a matching toolchain.
The backend supplies C/C++ compiler requirements, derives the glibc sysroot from the platform, and records the runtime and hardware requirements in each package.

Publish from `package/`:

```nu
cd package
pixi publish --path ./pixi.toml --target-channel ../channel
cd ..
```

`--target-channel` creates an indexed local channel containing both builds.
The producer has no dependency on that channel, so publishing works before it exists.

## Install and run

Create the consumer manifest beside `package/`:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/pixi.toml"
```

The two rich platforms describe separate solves for the same `linux-64` subdir.
AMD comes first, so Pixi selects it automatically on compatible hardware; otherwise it selects `cpu`.
Both require `archspec = "x86_64_v3"` because the prebuilt Modular runtime needs that CPU baseline.
`amdgpu = "gfx1103"` requires that exact ISA, not `gfx1103` or newer.
Within a solve where both packages are compatible, the CPU package's `track_features` entry makes it the lower-priority fallback.

Run from the consumer directory:

```nu
pixi run matmul
```

On a compatible Radeon 780M, this prints:

```text
path=gfx1103 side=1024 validation=ok
```

Select each platform explicitly and inspect the installed package:

```nu
pixi run --platform cpu matmul
pixi list --platform cpu '^mojo-matmul$' --fields name,version,build,track-features
pixi run --platform amd matmul
pixi list --platform amd '^mojo-matmul$' --fields name,version,build,track-features
```

The CPU executable prints `path=cpu`; the AMD executable prints `path=gfx1103`.
Forcing `amd` does not make an unsupported GPU compatible.
The installed packages have distinct builds:

```text
Name         Version  Build       Track Features
mojo-matmul  0.1.0    hdd18115_0  mojo-matmul-p-0
mojo-matmul  0.1.0    h5d698f7_0
```

The first row is CPU and the second is AMD. Build hashes can change with build inputs.

## Optional: measure whole-program time

On a compatible AMD machine with an external `time` command, use Nushell to warm up both installed binaries and repeat under the same conditions:

```nu
let timer = (which time | first).path
for platform in [cpu amd] {
    pixi run --platform $platform matmul
}
for sample in 1..5 {
    for platform in [cpu amd] {
        print $"sample=($sample) platform=($platform)"
        pixi run --platform $platform $timer -p matmul
    }
}
```

Pixi prepares each platform's environment and resolves its installed executable on `PATH`.
`time` runs inside that environment and measures only `matmul`, not Pixi's installation or task-runner overhead.
Use `real` for whole-program elapsed time, including process startup, input initialization, allocation, computation, device transfers, synchronization, and validation. This is not kernel time.
Keep power profile, AC/battery state, and background load fixed; record hardware, toolchain, matrix size, and sample variability.
Any CPU/GPU ratio compares the GPU against this single-threaded CPU implementation, not an optimized full-CPU implementation.
