#!/usr/bin/env Rscript
# /// conda-script
# [dependencies]
# r-base = "4.3.*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "Rscript"
# /// end-conda-script

# A simple Hello World R script demonstrating conda-script metadata
# Run with: pixi exec hello_r.R

cat("============================================================\n")
cat("Hello from R with conda-script!\n")
cat("============================================================\n")
cat(sprintf("R version: %s\n", R.version$version.string))
cat(sprintf("Platform: %s\n", R.version$platform))
cat(sprintf("System: %s\n", Sys.info()["sysname"]))
cat("============================================================\n")

# Simple R example
numbers <- 1:10
cat(sprintf("Sum of 1 to 10: %d\n", sum(numbers)))
cat(sprintf("Mean of 1 to 10: %.2f\n", mean(numbers)))
