All logic regarding the decision which dependencies can be installed from which channel is done by the instruction we give the solver.

The actual code regarding this is in the [`rattler_solve`](https://github.com/mamba-org/rattler/blob/02e68c9539c6009cc1370fbf46dc69ca5361d12d/crates/rattler_solve/src/resolvo/mod.rs) crate.
This might however be hard to read.
Therefore, this document will continue with pseudocode.

# Channel specific dependencies
When a user defines a channel per dependency, the solver needs to know the other channels are unusable for this dependency.
```toml
[project]
channels = ["conda-forge", "my-channel"]

[dependencies]
packgex = { version = "*", channel = "my-channel" }
```
This will ensure you will only get that package from that channel.
The pseudocode of the logic that excludes all other channels looks like this:
```rust
// Given a set of requirements and channels
let requirements = vec![packgex];
let channels = vec![conda_forge, my_channel];

// Check each channel
for channel in channels {
    // Search for the required package in the channel
    for requirement in requirements {
        let package = channel.packages.find(requirement.name);
        if package.is_some() && requirement.channel != channel {
            // Exclude packages from other channels
            candidates.excluded.push(package);
        }
    }
}
```
This ensures that if a dependency's channel is specified, the solver only considers that channel for the dependency.

# Channel priority
Channel priority is dictated by the order in the `project.channels` array, where the first channel is the highest priority.
For instance:
```toml
[project]
channels = ["conda-forge", "my-channel"]
```
If the package is found in `conda-forge` the solver will not look for it in `my-channel`, because we tell the solver they are excluded.
Here is the pseudocode for that logic:
```rust
// Given a set of requirements and channels
let requirements = vec![packgex];
let channels = vec![conda_forge, my_channel];

for requirement in requirements{
    let mut first_channel = None;

    for channel in channels{
        if channel.packages.find(requirement.name) != None {
            if first_channel.is_none() || channel.name == first_channel {
                first_channel = Some(channel.name);
                candidates.push(package);
            } else {
                // If the package is found in a different channel, add it to the excluded candidates
                candidates.excluded.push(package);
            }
        }
    }
}
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
