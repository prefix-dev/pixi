# Building Mojo CPU and AMD GPU Packages

In this tutorial, we will build CPU and AMD GPU variants of a Mojo package with Pixi.
For background on building packages, see the [Getting Started](./getting_started.md) guide and the [Mojo backend documentation](backends/pixi-build-mojo.md).

!!! warning
    `pixi-build` is a preview feature and may change before it is stabilized.

## Why is This Useful?

Build variants let us distribute CPU and GPU implementations of the same package without changing its name or version.
Pixi can then select the AMD GPU build on compatible hardware and use the CPU build otherwise.
We will publish both builds to a local channel and install them in a separate workspace, without a source dependency or build backend in the consumer.

## Let's Get Started

We will create a `mojo-matmul` package that multiplies two matrices.
This example needs Linux x86-64, glibc 2.34 or newer, and an x86-64-v3-capable CPU for both builds.
The GPU build targets `gfx1103`, such as the Radeon 780M.
See [platform configuration](../workspace/multi_platform_configuration.md#declaring-virtual-packages-per-platform) for hardware declarations, including other AMD targets.

Create these files in a new directory:

```text
.
├── package/
│   ├── main.mojo
│   └── pixi.toml
└── pixi.toml
```

First copy the matrix multiplication program.

```mojo title="package/main.mojo"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/package/main.mojo"
```

There is a lot of math involved, but you don't have to understand all of it to profit from this tutorial.
The important bit is that different code is executed if `ACCELERATOR == "cpu"` than if `ACCELERATOR == "gfx1103"`.

The CPU branch uses a plain, single-threaded loop with normal compiler optimizations.
The GPU branch copies the inputs to device buffers, launches one thread per output cell, copies the result back, and synchronizes before reading it.
Both branches multiply the same matrices, so we expect the same result but different performance.

Next, define the producer workspace and package:

```toml title="package/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/package/pixi.toml"
```

The [`mojo_accelerator`](backends/pixi-build-mojo.md#accelerator-variants) build variant produces two packages with the same name and version but different build hashes.

You can create a local conda channel like this:

```nu
pixi publish --path ./package/pixi.toml --target-channel channel
```

`--target-channel` creates an indexed local channel containing both builds.
The producer has no dependency on that channel, so publishing works before it exists.

## Install and run

Create the consumer manifest beside `package/`:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/mojo_accelerators/pixi.toml"
```

The two platforms describe separate solves for the same `linux-64` conda subdir.
`amd` comes first, so Pixi tries it before `cpu` when its built-in declarations match the machine.
Both require `archspec = "x86_64_v3"` because the prebuilt Modular runtime needs that CPU baseline.
`amdgpu = "gfx1103"` solves `amd` for a GPU with the gfx1103 [ISA](https://gpuopen.com/amd-gpu-architecture-programming-documentation/).
The backend adds the run dependency `__amdgpu_arch ==11.0.3` to the AMD build.
That locked requirement enforces the exact ISA match; a different ISA makes Pixi skip `amd` and try `cpu`.
See [Which platform gets installed?](../workspace/multi_platform_configuration.md#which-platform-gets-installed) for the details.
If the GPU build is suitable, typically the CPU is suitable as well.
To prioritize the GPU build in that case, `pixi-build-mojo` makes sure to [down prioritize](https://rattler-build.prefix.dev/latest/reference/recipe_file/#variant-configuration) the CPU build.

Run from the consumer directory:

```shell
pixi run matmul
```

On a compatible Radeon 780M, this prints:

```text
path=gfx1103 side=1024 validation=ok
```

To run the CPU build explicitly:

```nu
pixi run --platform cpu matmul
```

```text
path=cpu side=1024 validation=ok
```

## Optional: measure whole-program time

On a compatible AMD machine, install Hyperfine and measure each implementation:

```shell
pixi global install hyperfine
pixi run --platform cpu hyperfine --warmup 1 --runs 5 matmul
pixi run --platform amd hyperfine --warmup 1 --runs 5 matmul
```

On the machine the tutorial author used, the GPU version was 4.8 times faster than the CPU version.