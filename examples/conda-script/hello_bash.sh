#!/bin/bash
# /// conda-script
# [dependencies]
# bash = "5.*"
# curl = "*"
# jq = "*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "bash"
# /// end-conda-script

# A simple Hello World bash script demonstrating conda-script metadata
# Run with: pixi exec hello_bash.sh

echo "========================================"
echo "Hello from Bash with conda-script!"
echo "========================================"
echo "Bash version: $BASH_VERSION"
echo "Platform: $(uname -s) $(uname -m)"
echo ""

# Demonstrate installed tools
echo "Installed tools:"
echo "- curl version: $(curl --version | head -n1)"
echo "  - installed at: $(which curl)"
echo "- jq version: $(jq --version)"
echo "  - installed at: $(which jq)"
echo "========================================"
