#!/usr/bin/env node
// /// script
// [tool.conda]
// channels = ["conda-forge"]
// dependencies = ["nodejs 22.*"]
//
// [tool.pixi]
// entrypoint = "node"
// ///

// A simple Hello World Node.js script with inline metadata, using the
// `//` comment marker.
// Run with: pixi exec hello_node.js

console.log("=".repeat(60));
console.log("Hello from Node.js with inline script metadata!");
console.log("=".repeat(60));
console.log(`Node.js version: ${process.version}`);
console.log(`Platform: ${process.platform} ${process.arch}`);
console.log("=".repeat(60));

const numbers = Array.from({ length: 10 }, (_, i) => i + 1);
const sum = numbers.reduce((a, b) => a + b, 0);
const mean = sum / numbers.length;

console.log(`Sum of 1 to 10: ${sum}`);
console.log(`Mean of 1 to 10: ${mean.toFixed(2)}`);
