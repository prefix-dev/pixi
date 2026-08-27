// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "dotnet run ${SCRIPT}"
//
// [dependencies]
// dotnet = "*"
// /// end-conda-script

#:package Newtonsoft.Json@13.*

using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

var payload = new JObject { ["item"] = "answer", ["value"] = 42 };
Console.WriteLine(payload.ToString(Formatting.None));
