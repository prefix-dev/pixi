# Example use-cases
`pax` is a versatile package manager that can be utilized for a wide range of applications.
Although it can be employed in various scenarios, here are a few notable examples where `pax` is particularly effective.

## Global package installation in isolation
Similar to other tools like pipx and condax, pax can be used to install software binaries along with their dependencies into an isolated environment.
This strategy helps prevent cluttering system dependencies.

The idea is that you install a tool with all its own dependencies into its own environment and don't depend on system dependencies at all.
Except for very low level drivers like Cuda and platform libraries.

Examples of such installations, which automatically fetch the tools from the `conda-forge` channel, are:
```shell
pax install starship
pax install ruff
```
After running the above commands (and adding the binary folder to your path) the tools are directly available from the command line.

If you wish to install packages from a different channel, the `--channel` or `-c` option can be used:
```shell
pax install --channel conda-forge --channel bioconda trackplot
# Or in a more concise form
pax install -c conda-forge -c bioconda trackplot
```

The `install` command in pax can take a matchspec, providing you with the flexibility to specify the exact version of a package you want to install.
You can fine-tune the version down to the build:
```shell
pax install python=3.9.*
pax install "python [version="3.11.0", build_number=1]"
pax install "python [version="3.11.0", build=he550d4f_1_cpython]"
pax install python=3.11.0=h10a6764_1_cpython
```
