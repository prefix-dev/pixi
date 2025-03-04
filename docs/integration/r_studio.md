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

To use the R packages installed by `pixi` in RStudio, you need to run `rstudio` from an activated environment. This can be achieved by running RStudio from `pixi shell` or from a task in the `pixi.toml` file.

## Full example

The full example can be found here: [RStudio example](https://github.com/prefix-dev/pixi/tree/main/examples/r).
Here is an example of a `pixi.toml` file that sets up an RStudio task:

```toml
[project]
name = "r"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-64", "osx-arm64"]

[target.linux.tasks]
rstudio = "rstudio"

[target.osx.tasks]
rstudio = "open -a rstudio"
# or alternatively with the full path:
# rstudio = "/Applications/RStudio.app/Contents/MacOS/RStudio"

[dependencies]
r = ">=4.3,<5"
r-ggplot2 = ">=3.5.0,<3.6"
```

Once RStudio has loaded, you can execute the following R code that uses the `ggplot2` package:

```R
# Load the ggplot2 package
library(ggplot2)

# Load the built-in 'mtcars' dataset
data <- mtcars

# Create a scatterplot of 'mpg' vs 'wt'
ggplot(data, aes(x = wt, y = mpg)) +
  geom_point() +
  labs(x = "Weight (1000 lbs)", y = "Miles per Gallon") +
  ggtitle("Fuel Efficiency vs. Weight")
```

!!! Note
    This example assumes that you have installed RStudio system-wide.
    We are working on updating RStudio as well as the R interpreter builds on Windows for maximum compatibility with `pixi`.
