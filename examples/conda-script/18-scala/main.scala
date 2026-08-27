// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "scala run ${SCRIPT} --workspace ${CACHE}"
//
// [dependencies]
// scala3 = "*"
// /// end-conda-script
//> using scala 3.7.4
//> using dep "com.lihaoyi::upickle:4.1.0"

import upickle.default.{ReadWriter, write}

case class Language(name: String, year: Int) derives ReadWriter

@main def app(): Unit =
  println(write(Language("scala", 2004)))
