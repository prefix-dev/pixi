# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "julia ${SCRIPT}"
#
# [dependencies]
# julia = "*"
# /// end-conda-script
import Pkg
Pkg.activate("conda-script"; shared=true)
Pkg.add("JSON")
using JSON

data = (name = "conda-script", languages = ["julia", "python"], count = 2)
println(JSON.json(data))
