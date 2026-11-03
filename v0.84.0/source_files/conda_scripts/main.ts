// /// conda-script
// channels = ["https://prefix.dev/conda-forge"]
// entrypoint = "deno run ${SCRIPT}"
//
// [dependencies]
// deno = "*"
// /// end-conda-script

import { chunk } from "npm:lodash-es@4";

const pairs: string[][] = chunk(["a", "b", "c", "d"], 2);
console.log(JSON.stringify(pairs));
