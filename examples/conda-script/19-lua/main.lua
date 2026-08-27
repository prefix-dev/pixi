-- /// conda-script
-- channels = ["conda-forge"]
-- entrypoint = "lua ${SCRIPT}"
--
-- [dependencies]
-- lua = "*"
-- lua-luafilesystem = "*"
-- /// end-conda-script
local lfs = require("lfs")

print("script is a " .. lfs.attributes(arg[0], "mode"))
print("currentdir is a " .. lfs.attributes(lfs.currentdir(), "mode"))
