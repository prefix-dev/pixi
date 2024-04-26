All logic regarding the decision which dependencies can be installed from which channel is done by the instruction we give the solver.

The actual code regarding this is in the [`rattler_solve`](https://github.com/mamba-org/rattler/blob/02e68c9539c6009cc1370fbf46dc69ca5361d12d/crates/rattler_solve/src/resolvo/mod.rs) crate.
This might however be hard to read.
Therefore, this document will continue with simplified flow charts.

# Channel specific dependencies
When a user defines a channel per dependency, the solver needs to know the other channels are unusable for this dependency.
```toml
[project]
channels = ["conda-forge", "my-channel"]

[dependencies]
packgex = { version = "*", channel = "my-channel" }
```
In the `packagex` example, the solver will understand that the package is only available in `my-channel` and will not look for it in `conda-forge`.

The flowchart of the logic that excludes all other channels:

``` mermaid
flowchart TD
    A[Start] --> B[Given a Dependency]
    B --> C{Channel Specific Dependency?}
    C -->|Yes| D[Exclude All Other Channels for This Package]
    C -->|No| E{Any Other Dependencies?}
    E -->|Yes| B
    E -->|No| F[End]
    D --> E
```

# Channel priority
Channel priority is dictated by the order in the `project.channels` array, where the first channel is the highest priority.
For instance:
```toml
[project]
channels = ["conda-forge", "my-channel", "your-channel"]
```
If the package is found in `conda-forge` the solver will not look for it in `my-channel` and `your-channel`, because it tells the solver they are excluded.
If the package is not found in `conda-forge` the solver will look for it in `my-channel` and if it **is** found there it will tell the solver to exclude `your-channel` for this package.
This diagram explains the logic:
``` mermaid
flowchart TD
    A[Start] --> B[Given a Dependency]
    B --> C{Loop Over Channels}
    C --> D{Package in This Channel?}
    D -->|No| C
    D -->|Yes| E{"This the first channel
     for this package?"}
    E -->|Yes| F[Include Package in Candidates]
    E -->|No| G[Exclude Package from Candidates]
    F --> H{Any Other Channels?}
    G --> H
    H -->|Yes| C
    H -->|No| I{Any Other Dependencies?}
    I -->|No| J[End]
    I -->|Yes| B
```

This method ensures the solver only adds a package to the candidates if it's found in the highest priority channel available.
If you have 10 channels and the package is found in the 5th channel it will exclude the next 5 channels from the candidates if they also contain the package.

# Use case: pytorch and nvidia with conda-forge
A common use case is to use `pytorch` with `nvidia` drivers, while also needing the `conda-forge` channel for the main dependencies.
```toml
[project]
channels = ["nvidia/label/cuda-11.8.0", "nvidia", "conda-forge", "pytorch"]
platforms = ["linux-64"]

[dependencies]
cuda = {version = "*", channel="nvidia/label/cuda-11.8.0"}
pytorch = {version = "2.0.1.*", channel="pytorch"}
torchvision = {version = "0.15.2.*", channel="pytorch"}
pytorch-cuda = {version = "11.8.*", channel="pytorch"}
python = "3.10.*"
```
What this will do is get as much as possible from the `nvidia/label/cuda-11.8.0` channel, which is actually only the `cuda` package.

Then it will get all packages from the `nvidia` channel, which is a little more and some packages overlap the `nvidia` and `conda-forge` channel.
Like the `cuda-cudart` package, which will now only be retrieved from the `nvidia` channel because of the priority logic.

Then it will get the packages from the `conda-forge` channel, which is the main channel for the dependencies.

But the user only wants the pytorch packages from the `pytorch` channel, which is why `pytorch` is added last and the dependencies are added as channel specific dependencies.

We don't define the `pytorch` channel before `conda-forge` because we want to get as much as possible from the `conda-forge` as the pytorch channel is not always shipping the best versions of all packages.

For example, it also ships the `ffmpeg` package, but only an old version which doesn't work with the newer pytorch versions.
Thus breaking the installation if we would skip the `conda-forge` channel for `ffmpeg` with the priority logic.

## Force a specific channel priority
If you want to force a specific priority for a channel, you can use the `priority` (int) key in the channel definition.
The higher the number, the higher the priority.
Non specified priorities are set to 0 but the index in the array still counts as a priority, where the first in the list has the highest priority.

This priority definition is mostly important for [multiple environments](../features/multi_environment.md) with different channel priorities, as by default feature channels are prepended to the project channels.

```toml
[project]
name = "test_channel_priority"
platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]
channels = ["conda-forge"]

[feature.a]
channels = ["nvidia"]

[feature.b]
channels = [ "pytorch", {channel = "nvidia", priority = 1}]

[feature.c]
channels = [ "pytorch", {channel = "nvidia", priority = -1}]

[environments]
a = ["a"]
b = ["b"]
c = ["c"]
```
This example creates 4 environments, `a`, `b`, `c`, and the default environment.
Which will have the following channel order:

| Environment | Resulting Channels order           |
|-------------|------------------------------------|
| default     | `conda-forge`                      |
| a           | `nvidia`, `conda-forge`            |
| b           | `nvidia`, `pytorch`, `conda-forge` |
| c           | `pytorch`, `conda-forge`, `nvidia` |

??? tip "Check priority result with `pixi info`"
    Using `pixi info` you can check the priority of the channels in the environment.
    ```bash
    pixi info
    Environments
    ------------
           Environment: default
              Features: default
              Channels: conda-forge
    Dependency count: 0
    Target platforms: linux-64

           Environment: a
              Features: a, default
              Channels: nvidia, conda-forge
    Dependency count: 0
    Target platforms: linux-64

           Environment: b
              Features: b, default
              Channels: nvidia, pytorch, conda-forge
    Dependency count: 0
    Target platforms: linux-64

           Environment: c
              Features: c, default
              Channels: pytorch, conda-forge, nvidia
    Dependency count: 0
    Target platforms: linux-64
    ```
