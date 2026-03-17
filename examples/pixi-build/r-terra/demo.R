library(terrademo)

cat("=== Terra + Pixi Build Demo ===\n\n")

# Show terra and system library versions
cat("Terra version:", as.character(packageVersion("terra")), "\n")
cat("GDAL version:", terra::gdal(), "\n")
cat("GEOS version:", terra::geos(), "\n")
cat("PROJ version:", terra::proj(), "\n\n")

# Create and analyze a raster
cat("Creating demo raster...\n")
r <- create_demo_raster()
cat("Raster:", terra::nrow(r), "x", terra::ncol(r), "cells\n")

result <- analyze_raster(r)
cat("\nRaster statistics:\n")
print(result$stats)

# Create vector data and extract raster values at points
cat("\nCreating demo points and extracting raster values...\n")
pts <- create_demo_vector(10)
extracted <- terra::extract(r, pts)
cat("Extracted values at", nrow(extracted), "points:\n")
print(head(extracted))

cat("\nDone! All system dependencies (GDAL, GEOS, PROJ) resolved by pixi.\n")
