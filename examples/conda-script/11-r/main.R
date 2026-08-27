# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "Rscript ${SCRIPT}"
#
# [dependencies]
# r-base = "*"
# r-jsonlite = "*"
# /// end-conda-script
library(jsonlite)

document <- list(
  name = "conda-script",
  languages = c("r", "python"),
  count = 2
)
writeLines(toJSON(document, auto_unbox = TRUE, pretty = TRUE))
