#' Create a demo raster
#'
#' Creates a small example raster with random elevation-like data.
#'
#' @return A SpatRaster object
#' @export
create_demo_raster <- function() {
    r <- terra::rast(nrows = 50, ncols = 50, xmin = 0, xmax = 10, ymin = 0, ymax = 10)
    terra::values(r) <- runif(terra::ncell(r), min = 0, max = 1000)
    names(r) <- "elevation"
    r
}

#' Analyze a raster
#'
#' Computes basic statistics and applies a focal mean filter.
#'
#' @param r A SpatRaster object
#' @return A list with summary stats and the smoothed raster
#' @export
analyze_raster <- function(r) {
    stats <- terra::global(r, fun = c("mean", "min", "max", "sd"))
    smoothed <- terra::focal(r, w = 3, fun = "mean", na.rm = TRUE)
    list(stats = stats, smoothed = smoothed)
}

#' Create a demo vector
#'
#' Creates example point geometries with random coordinates.
#'
#' @param n Number of points
#' @return A SpatVector of points
#' @export
create_demo_vector <- function(n = 20) {
    coords <- cbind(
        x = runif(n, min = 0, max = 10),
        y = runif(n, min = 0, max = 10)
    )
    terra::vect(coords, crs = "EPSG:4326")
}
