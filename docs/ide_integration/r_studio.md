# Developing R scripts in RStudio

You can use `pixi` to manage your R dependencies. The conda-forge channel contains a wide range of R packages that can be installed using `pixi`.

## Installing R packages

R packages are usually prefixed with `r-` in the conda-forge channel. To install an R package, you can use the following command:

```bash
pixi add r-<package-name>
# for example
pixi add r-ggplot2
```

## Using R packages in RStudio

To use the R packages installed by `pixi` in RStudio, you need to set the R interpreter to the one installed by `pixi`. You can do this by setting the `RSTUDIO_WHICH_R` environment variable to the path of the R interpreter installed by `pixi`.

```toml
[tasks]
rstudio = "RSTUDIO_WHICH_R=$CONDA_PREFIX/bin/R rstudio"
```

Now you can run `pixi run rstudio` in your project, and it will launch with the proper R interpreter and the dependencies managed by pixi.

## Full example

Here is an example of a `pixi.toml` file that sets up an RStudio task:

```toml
[project]
name = "r"
version = "0.1.0"
description = "Add a short description here"
authors = ["Wolf Vollprecht <wolf@prefix.dev>"]
channels = ["conda-forge"]
platforms = ["linux-64", "osx-64", "osx-arm64"]

[target.linux.tasks]
rstudio = "RSTUDIO_WHICH_R=$CONDA_PREFIX/bin/R rstudio"

[target.macos.tasks]
rstudio = "RSTUDIO_WHICH_R=$CONDA_PREFIX/bin/R /Applications/RStudio.app/Contents/MacOS/RStudio"

[dependencies]
r = ">=4.3,<5"
r-ggplot2 = ">=3.5.0,<3.6"
```

!!! Note
    This example assumes that you have installed RStudio system-wide. We are working on updating RStudio as well as the R interpreter builds on Windows for maximum compatibility with `pixi`.