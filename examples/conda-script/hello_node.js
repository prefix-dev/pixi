#!/usr/bin/env node
// /// conda-script
// [dependencies]
// nodejs = "20.*"
// [script]
// channels = ["conda-forge"]
// entrypoint = "node"
// /// end-conda-script

// A simple Hello World Node.js script demonstrating conda-script metadata
// Run with: pixi exec hello_node.js

console.log("=".repeat(60));
console.log("Hello from Node.js with conda-script!");
console.log("=".repeat(60));
console.log(`Node.js version: ${process.version}`);
console.log(`Platform: ${process.platform} ${process.arch}`);
console.log("=".repeat(60));

// Simple JavaScript example
const numbers = Array.from({ length: 10 }, (_, i) => i + 1);
const sum = numbers.reduce((a, b) => a + b, 0);
const mean = sum / numbers.length;

console.log(`Sum of 1 to 10: ${sum}`);
console.log(`Mean of 1 to 10: ${mean.toFixed(2)}`);
