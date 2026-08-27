// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "kotlin ${SCRIPT}"
//
// [dependencies]
// kotlin = "*"
// /// end-conda-script
@file:DependsOn("com.google.code.gson:gson:2.13.1")

import com.google.gson.GsonBuilder

data class Language(val name: String, val year: Int)

val gson = GsonBuilder().setPrettyPrinting().create()
println(gson.toJson(Language("kotlin", 2011)))
