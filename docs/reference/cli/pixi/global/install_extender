--8<-- [start:example]

## Examples

```shell
pixi global install ruff
# Multiple packages can be installed at once
pixi global install starship rattler-build
# Specify the channel(s)
pixi global install --channel conda-forge --channel bioconda trackplot
# Or in a more concise form
pixi global install -c conda-forge -c bioconda trackplot

# Support full conda matchspec
pixi global install python=3.9.*
pixi global install "python [version='3.11.0', build_number=1]"
pixi global install "python [version='3.11.0', build=he550d4f_1_cpython]"
pixi global install python=3.11.0=h10a6764_1_cpython

# Install for a specific platform, only useful on osx-arm64
pixi global install --platform osx-64 ruff

# Install a package with all its executables exposed, together with additional packages that don't expose anything
pixi global install ipython --with numpy --with scipy

# Install into a specific environment name and expose all executables
pixi global install --environment data-science ipython jupyterlab numpy matplotlib

# Expose the binary under a different name
pixi global install --expose "py39=python3.9" "python=3.9.*"
```
--8<-- [end:example]

--8<-- [start:description]

!!! tip
Running `osx-64` on Apple Silicon will install the Intel binary but run it using [Rosetta](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment)
```
pixi global install --platform osx-64 ruff
```

After using global install, you can use the package you installed anywhere on your system.
--8<-- [end:description]
